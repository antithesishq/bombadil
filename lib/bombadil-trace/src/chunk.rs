use std::io::{self, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::{RECORD_HEADER_LEN, RECORD_PAYLOAD_BYTES_MAX, tag, write_record};

const TARGET_BYTES: usize = 1024 * 1024;
const STATES_MAX: usize = 32;
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) fn decode(payload: &[u8]) -> Result<Vec<u8>> {
    let (length, compressed) = payload
        .split_at_checked(4)
        .context("compressed chunk has no decoded length")?;
    let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
    if length == 0 || length > RECORD_PAYLOAD_BYTES_MAX {
        bail!("compressed chunk decoded length out of range");
    }
    let decoded = zstd::bulk::decompress(compressed, length)
        .context("invalid compressed chunk")?;
    if decoded.len() != length {
        bail!("compressed chunk decoded length mismatch");
    }
    Ok(decoded)
}

pub(crate) struct ChunkWriter<W> {
    writer: W,
    compressor: zstd::bulk::Compressor<'static>,
    buffer: Vec<u8>,
    compressed: Vec<u8>,
    states: usize,
    started: Option<Instant>,
}

impl<W: Write> ChunkWriter<W> {
    pub fn new(writer: W) -> io::Result<Self> {
        Ok(Self {
            writer,
            compressor: zstd::bulk::Compressor::new(3)?,
            buffer: Vec::with_capacity(TARGET_BYTES),
            compressed: Vec::with_capacity(zstd::zstd_safe::compress_bound(
                TARGET_BYTES,
            )),
            states: 0,
            started: None,
        })
    }

    fn remaining(&self, now: Instant) -> Option<Duration> {
        self.started.map(|started| {
            FLUSH_INTERVAL
                .saturating_sub(now.saturating_duration_since(started))
        })
    }

    pub fn recv<T>(&mut self, receiver: &Receiver<T>) -> io::Result<Option<T>> {
        match receiver.try_recv() {
            Ok(message) => return Ok(Some(message)),
            Err(TryRecvError::Disconnected) => return Ok(None),
            Err(TryRecvError::Empty) => {}
        }
        let Some(remaining) = self.remaining(Instant::now()) else {
            self.flush()?;
            return Ok(receiver.recv().ok());
        };
        match receiver.recv_timeout(remaining) {
            Ok(message) => Ok(Some(message)),
            Err(RecvTimeoutError::Disconnected) => Ok(None),
            Err(RecvTimeoutError::Timeout) => {
                self.flush()?;
                Ok(receiver.recv().ok())
            }
        }
    }

    pub fn state(
        &mut self,
        metadata: &[u8],
        payload: Option<&[u8]>,
        now: Instant,
    ) -> io::Result<()> {
        let length = RECORD_HEADER_LEN
            + metadata.len()
            + payload.map_or(0, |p| RECORD_HEADER_LEN + p.len());
        if self.buffer.len() + length > RECORD_PAYLOAD_BYTES_MAX {
            self.flush()?;
        }
        let state_tag = if payload.is_some() {
            tag::STATE_WITH_PAYLOAD
        } else {
            tag::STATE
        };
        if length > RECORD_PAYLOAD_BYTES_MAX {
            write_record(&mut self.writer, state_tag, &[metadata])?;
            if let Some(payload) = payload {
                write_record(&mut self.writer, tag::STATE_PAYLOAD, &[payload])?;
            }
            return self.writer.flush();
        }
        self.buffer.reserve(length);
        self.started.get_or_insert(now);
        write_record(&mut self.buffer, state_tag, &[metadata])?;
        if let Some(payload) = payload {
            write_record(&mut self.buffer, tag::STATE_PAYLOAD, &[payload])?;
        }
        self.states += 1;
        if self.states >= STATES_MAX
            || self.buffer.len() >= TARGET_BYTES
            || self.remaining(now) == Some(Duration::ZERO)
        {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return self.writer.flush();
        }
        self.compressed.clear();
        self.compressed
            .reserve(zstd::zstd_safe::compress_bound(self.buffer.len()));
        self.compressor
            .compress_to_buffer(&self.buffer, &mut self.compressed)?;
        if self.compressed.len() + 4 <= RECORD_PAYLOAD_BYTES_MAX {
            let length = (self.buffer.len() as u32).to_le_bytes();
            write_record(
                &mut self.writer,
                tag::CHUNK,
                &[&length, &self.compressed],
            )?;
        } else {
            self.writer.write_all(&self.buffer)?;
        }
        self.buffer.clear();
        self.states = 0;
        self.started = None;
        self.writer.flush()
    }

    pub fn finish(&mut self) -> io::Result<()> {
        self.flush()?;
        write_record(&mut self.writer, tag::END, &[])?;
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        StateReader, TraceDriver, TraceFileWriter, open_trace, read_payload_at,
        read_payload_from, write_envelope,
    };
    use hegel::{TestCase, generators::integers};
    use std::{
        fs::File,
        io::{Read, Seek},
        sync::Arc,
    };

    #[test]
    fn flushes_at_the_state_limit() {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        let mut writer = ChunkWriter::new(file).unwrap();
        let now = Instant::now();
        for index in 0..2 * STATES_MAX + 1 {
            writer
                .state(index.to_string().as_bytes(), None, now)
                .unwrap();
            let mut reader =
                StateReader::<_, usize>::new(&writer.writer[..]).unwrap();
            let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
            assert!(!reader.finished());
            let visible = (index + 1) / STATES_MAX * STATES_MAX;
            assert_eq!(
                read.iter().map(|state| state.metadata).collect::<Vec<_>>(),
                (0..visible).collect::<Vec<_>>()
            );
        }
    }

    #[hegel::test]
    fn flushes_at_the_byte_limit(tc: TestCase) {
        let overhead = 2 * RECORD_HEADER_LEN + 1;
        let first_length = tc.draw(
            integers::<usize>().max_value(TARGET_BYTES - 2 * overhead - 1),
        );
        let byte = tc.draw(integers::<u8>());
        let first = vec![byte; first_length];
        let now = Instant::now();
        for total in [TARGET_BYTES - 1, TARGET_BYTES, TARGET_BYTES + 1] {
            let second = vec![byte ^ 0xff; total - 2 * overhead - first_length];
            let mut file = Vec::new();
            write_envelope(&mut file, TraceDriver::Terminal).unwrap();
            let mut writer = ChunkWriter::new(file).unwrap();
            writer.state(b"0", Some(&first), now).unwrap();
            assert_eq!(writer.writer.len(), crate::ENVELOPE_LEN);
            writer.state(b"1", Some(&second), now).unwrap();
            let read = StateReader::<_, usize>::new(&writer.writer[..])
                .unwrap()
                .collect::<Result<Vec<_>>>()
                .unwrap();
            assert_eq!(read.len(), if total < TARGET_BYTES { 0 } else { 2 });
            for (index, (state, payload)) in
                read.iter().zip([&first, &second]).enumerate()
            {
                assert_eq!(state.metadata, index);
                assert_eq!(state.payload.as_ref(), Some(payload));
            }
            writer.state(b"2", None, now).unwrap();
            let read = StateReader::<_, usize>::new(&writer.writer[..])
                .unwrap()
                .collect::<Result<Vec<_>>>()
                .unwrap();
            let visible = if total < TARGET_BYTES { 3 } else { 2 };
            assert_eq!(
                read.iter().map(|state| state.metadata).collect::<Vec<_>>(),
                (0..visible).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn flushes_at_the_deadline_without_postponing_it() {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        let mut writer = ChunkWriter::new(file).unwrap();
        let now = Instant::now();
        for batch in 0..2 {
            let start = now + FLUSH_INTERVAL * batch;
            for (offset, elapsed) in [
                Duration::ZERO,
                FLUSH_INTERVAL - Duration::from_nanos(1),
                FLUSH_INTERVAL,
            ]
            .into_iter()
            .enumerate()
            {
                let index = batch as usize * 3 + offset;
                writer
                    .state(index.to_string().as_bytes(), None, start + elapsed)
                    .unwrap();
                let read = StateReader::<_, usize>::new(&writer.writer[..])
                    .unwrap()
                    .collect::<Result<Vec<_>>>()
                    .unwrap();
                let visible =
                    batch as usize * 3 + if offset == 2 { 3 } else { 0 };
                assert_eq!(
                    read.iter().map(|state| state.metadata).collect::<Vec<_>>(),
                    (0..visible).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn chunks_support_independent_payload_reads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.bin");
        let mut writer =
            TraceFileWriter::create(&path, TraceDriver::Browser, false)
                .unwrap();
        let payload = |n: u32| {
            if n != 33 {
                return n.to_le_bytes().repeat(257);
            }
            let mut bits = 0x9e37_79b1u32;
            (0..2 * TARGET_BYTES + 17)
                .map(|_| {
                    bits ^= bits << 13;
                    bits ^= bits >> 17;
                    bits ^= bits << 5;
                    bits as u8
                })
                .collect()
        };
        for n in 0..70 {
            writer.write_state(n, Some(Arc::new(payload(n)))).unwrap();
        }
        writer.finish().unwrap();
        let read = open_trace::<u32>(&path)
            .unwrap()
            .without_payloads()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(read.len(), 70);
        let first = read[0].payload_location.unwrap();
        let last = read[69].payload_location.unwrap();
        assert!(first.chunk.is_some() && last.chunk > first.chunk);
        let file = File::open(&path).unwrap();
        (&file).read_exact(&mut [0; 7]).unwrap();
        for state in read.iter().rev() {
            let location = state.payload_location.unwrap();
            assert_eq!(
                read_payload_from(&file, location).unwrap(),
                payload(state.metadata)
            );
        }
        assert_eq!((&file).stream_position().unwrap(), 7);
        let mut bytes = std::fs::read(&path).unwrap();
        let index = first.chunk.unwrap() as usize + RECORD_HEADER_LEN + 5;
        bytes[index] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(open_trace::<u32>(&path).unwrap().next().unwrap().is_err());
        assert_eq!(read_payload_at(&path, last).unwrap(), payload(69));
    }

    #[test]
    fn rejects_invalid_chunks_even_with_valid_outer_checksums() {
        let mut state = Vec::new();
        write_record(&mut state, tag::STATE, &[b"{}"]).unwrap();
        let encode = |bytes: &[u8], length: u32| {
            let mut result = length.to_le_bytes().to_vec();
            result.extend(zstd::bulk::compress(bytes, 3).unwrap());
            result
        };
        let mut nested = Vec::new();
        write_record(&mut nested, tag::CHUNK, &[b"anything"]).unwrap();
        let mut end = Vec::new();
        write_record(&mut end, tag::END, &[]).unwrap();
        let mut corrupt = state.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        let cases = [
            vec![],
            b"\x01\x00\x00\x00not zstd".to_vec(),
            encode(&state, 0),
            encode(&state, u32::MAX),
            encode(&state, state.len() as u32 + 1),
            encode(&state, state.len() as u32 - 1),
            encode(&state[..3], 3),
            encode(&state[..state.len() - 1], state.len() as u32 - 1),
            encode(&nested, nested.len() as u32),
            encode(&end, end.len() as u32),
            encode(&corrupt, corrupt.len() as u32),
        ];
        for payload in cases {
            let mut file = Vec::new();
            write_envelope(&mut file, TraceDriver::Terminal).unwrap();
            write_record(&mut file, tag::CHUNK, &[&payload]).unwrap();
            write_record(&mut file, tag::END, &[]).unwrap();
            assert!(
                StateReader::<_, serde_json::Value>::new(&file[..])
                    .unwrap()
                    .collect::<Result<Vec<_>>>()
                    .is_err()
            );
        }
    }
}
