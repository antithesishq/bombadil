use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
};

use zerocopy::{FromBytes, IntoBytes};

use crate::{
    ENVELOPE_LEN, MAGIC, RECORD_HEADER_LEN, record::RecordHeader, tag,
};

/// Whether `file` ends with an end record's bytes, as a finished trace
/// does. Only a hint: the rest isn't read. Leaves `file` at its start.
pub fn ends_with_end_record(file: &mut File) -> io::Result<bool> {
    let end = RecordHeader::new(tag::END, 0, crc32fast::hash(&[]));
    let mut last = [0; RECORD_HEADER_LEN];
    let length = file.metadata()?.len();
    if length < (ENVELOPE_LEN + RECORD_HEADER_LEN) as u64 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-(RECORD_HEADER_LEN as i64)))?;
    let read = file.read_exact(&mut last);
    file.seek(SeekFrom::Start(0))?;
    read?;
    Ok(last == end.as_bytes())
}

/// Reads a trace file that may still be written, like `tail -f`: at the
/// end of the file it calls `wait` with the bytes read so far and reads
/// on, until the end record was read or `wait` returns `false`.
pub struct FollowReader<W> {
    file: File,
    wait: W,
    following: bool,
    position: u64,
    // Records are only framed here, to find the end record.
    header: [u8; RECORD_HEADER_LEN],
    header_filled: usize,
    // Bytes left of the envelope or payload being read.
    skip: u64,
}

impl<W: FnMut(u64) -> bool> FollowReader<W> {
    pub fn new(file: File, wait: W) -> Self {
        FollowReader {
            file,
            wait,
            following: true,
            position: 0,
            header: [0; RECORD_HEADER_LEN],
            header_filled: 0,
            skip: ENVELOPE_LEN as u64,
        }
    }

    /// Stops following at the end record, or where none can be found.
    fn frame(&mut self, mut bytes: &[u8]) {
        let magic = MAGIC.iter().skip(self.position as usize);
        if magic.zip(bytes).any(|(expected, byte)| expected != byte) {
            self.following = false;
        }
        self.position += bytes.len() as u64;
        while self.following && !bytes.is_empty() {
            if self.skip > 0 {
                let skipped = self.skip.min(bytes.len() as u64);
                self.skip -= skipped;
                bytes = &bytes[skipped as usize..];
                continue;
            }
            let taken = bytes.len().min(RECORD_HEADER_LEN - self.header_filled);
            self.header[self.header_filled..][..taken]
                .copy_from_slice(&bytes[..taken]);
            self.header_filled += taken;
            bytes = &bytes[taken..];
            if self.header_filled == RECORD_HEADER_LEN {
                self.header_filled = 0;
                let header = RecordHeader::read_from_bytes(&self.header)
                    .expect("the buffer is a header long");
                self.following =
                    header.checksum_matches() && header.tag != tag::END;
                self.skip = header.length() as u64;
            }
        }
    }
}

impl<W: FnMut(u64) -> bool> Read for FollowReader<W> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let read = self.file.read(buffer)?;
            self.frame(&buffer[..read]);
            if read > 0 || buffer.is_empty() || !self.following {
                return Ok(read);
            }
            // Read once more after the last wait.
            self.following = (self.wait)(self.position);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Record, StepError, TRACE_FILE_NAME, TraceDriver, TraceReader,
        chunk::ChunkWriter, write_envelope, write_record,
    };
    use hegel::{
        Generator, TestCase,
        generators::{booleans, integers, optional},
    };
    use std::{
        cell::{Cell, RefCell},
        io::{BufReader, Write},
        sync::mpsc,
        time::Instant,
    };

    /// State `n` has metadata `n`, a payload of the given length if any,
    /// ends a chunk if flagged, and has an action applied in it if flagged.
    struct Trace {
        states: Vec<(Option<usize>, bool, bool)>,
        /// Why no state followed the last action, if the run recorded it.
        error: Option<StepError>,
        finished: bool,
        bytes: Vec<u8>,
    }

    impl std::fmt::Debug for Trace {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Trace")
                .field("states", &self.states)
                .field("error", &self.error)
                .field("finished", &self.finished)
                .field("bytes", &self.bytes.len())
                .finish()
        }
    }

    hegel::pretty_print_as_debug!(Trace);

    #[hegel::composite]
    fn step_error(tc: &TestCase) -> StepError {
        let message = "it broke".to_string();
        match tc.draw(integers::<u8>().max_value(4)) {
            0 => StepError::Stalled,
            1 => StepError::Driver { message },
            2 => StepError::Failed { message },
            3 => StepError::Interrupted,
            _ => StepError::TimeLimit,
        }
    }

    #[hegel::composite]
    fn trace(tc: &TestCase, finished: Option<bool>) -> Trace {
        let count = tc.draw(integers::<usize>().max_value(40));
        let mut states = Vec::with_capacity(count);
        let mut bytes = Vec::new();
        write_envelope(&mut bytes, TraceDriver::Terminal).unwrap();
        let mut writer =
            ChunkWriter::new(&mut bytes, mpsc::channel::<()>().1).unwrap();
        let now = Instant::now();
        for n in 0..count {
            let payload =
                tc.draw(optional(integers::<usize>().max_value(20_000)));
            let bytes = payload.map(|length| {
                (0..length).map(|i| (i * 31 + n) as u8).collect::<Vec<_>>()
            });
            writer
                .write_state(n.to_string().as_bytes(), bytes.as_deref(), now)
                .unwrap();
            // As the writer does: straight to the file, ahead of the
            // chunk its state is in.
            let action = tc.draw(booleans());
            if action {
                let action = format!(r#"{{"action":{n},"action_taken":null}}"#);
                let parts = [&(n as u64).to_le_bytes()[..], action.as_bytes()];
                write_record(writer.writer_mut(), tag::ACTION, &parts).unwrap();
            }
            let flush = tc.draw(booleans());
            if flush {
                writer.flush().unwrap();
            }
            states.push((payload, flush, action));
        }
        let finished = finished.unwrap_or_else(|| tc.draw(booleans()));
        let pending = states.last().is_some_and(|&(_, _, action)| action);
        let error = (finished && pending && tc.draw(booleans()))
            .then(|| tc.draw(step_error().print_as_debug()));
        if let Some(error) = &error {
            let error = serde_json::to_vec(error).unwrap();
            writer.write_error(count as u64 - 1, &error).unwrap();
        }
        // A killed run may lose its last chunk.
        if finished {
            writer.finish().unwrap();
        } else if tc.draw(booleans()) {
            writer.flush().unwrap();
        }
        drop(writer);
        Trace {
            states,
            error,
            finished,
            bytes,
        }
    }

    /// The records, each passed to `seen` as it is read, then whether the
    /// trace was finished, or the error.
    fn read(
        reader: impl io::Read,
        mut seen: impl FnMut(&str),
    ) -> (Vec<String>, Result<bool, String>) {
        let mut records = Vec::new();
        let reader = TraceReader::<_, usize, serde_json::Value>::new(reader);
        let mut reader = match reader {
            Ok(reader) => reader,
            Err(error) => return (records, Err(format!("{error:#}"))),
        };
        for record in reader.by_ref() {
            match record {
                Ok(record) => {
                    let record = format!("{record:?}");
                    seen(&record);
                    records.push(record);
                }
                Err(error) => return (records, Err(format!("{error:#}"))),
            }
        }
        (records, Ok(reader.finished()))
    }

    /// How many of `bytes`' records are states.
    fn states_in(bytes: &[u8]) -> usize {
        let Ok(reader) = TraceReader::<_, usize, serde_json::Value>::new(bytes)
        else {
            return 0;
        };
        reader
            .filter(|record| matches!(record, Ok(Record::State { .. })))
            .count()
    }

    const STEPS_MAX: usize = 100;

    struct Writer {
        file: Option<File>,
        written: usize,
        steps: usize,
        left: bool,
    }

    /// The writer appends a few bytes at a time from inside `wait`, and
    /// may stop anywhere.
    #[hegel::test]
    fn follows_a_trace_as_reading_it_afterwards(tc: TestCase) {
        let mut trace = tc.draw(trace(None));
        let damaged = tc.draw(booleans());
        if damaged {
            let last = trace.bytes.len() - 1;
            let offset = tc.draw(integers::<usize>().max_value(last));
            trace.bytes[offset] ^= 1 << tc.draw(integers::<u8>().max_value(7));
        }
        let bytes = &trace.bytes;
        let cut = if tc.draw(booleans()) {
            bytes.len()
        } else {
            tc.draw(integers::<usize>().max_value(bytes.len()))
        };
        let complete = trace.finished && !damaged && cut == bytes.len();
        let expected = read(&bytes[..cut], |_| {});

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let writer = RefCell::new(Writer {
            file: Some(File::create(&path).unwrap()),
            written: 0,
            steps: 0,
            left: false,
        });
        let read_count = Cell::new(0);
        let wait = |position| {
            let mut writer = writer.borrow_mut();
            assert!(!writer.left, "waited after being told to stop");
            // Nothing readable is held back while waiting.
            assert_eq!(position, writer.written as u64);
            // Actions stay pending: no error is reported while waiting.
            assert_eq!(read_count.get(), states_in(&bytes[..writer.written]));
            let Some(mut file) = writer.file.take() else {
                assert!(!complete, "waited after the end record");
                writer.left = true;
                return false;
            };
            let rest = cut - writer.written;
            writer.steps += 1;
            let length = if writer.steps > STEPS_MAX {
                rest
            } else {
                tc.draw(integers::<usize>().max_value(rest))
            };
            file.write_all(&bytes[writer.written..][..length]).unwrap();
            writer.written += length;
            if writer.written < cut {
                writer.file = Some(file);
            } else if !complete {
                writer.left = tc.draw(booleans());
            }
            !writer.left
        };
        let capacity =
            tc.draw(integers::<usize>().min_value(1).max_value(8192));
        let follower = BufReader::with_capacity(
            capacity,
            FollowReader::new(File::open(&path).unwrap(), wait),
        );
        let followed = read(follower, |state: &str| {
            let expected = expected.0.get(read_count.get());
            assert_eq!(Some(state), expected.map(String::as_str));
            read_count.set(read_count.get() + 1);
        });
        assert_eq!(followed, expected);
        assert!(writer.borrow().written == cut || followed.1.is_err());
    }

    #[derive(Debug, Clone, Copy)]
    enum Ending {
        End { payload: usize },
        BadHeader,
        BadMagic { offset: usize },
    }

    hegel::pretty_print_as_debug!(Ending);

    #[hegel::composite]
    fn ending(tc: &TestCase) -> Ending {
        match tc.draw(integers::<u8>().max_value(2)) {
            0 => Ending::End {
                payload: tc.draw(integers().max_value(40)),
            },
            1 => Ending::BadHeader,
            _ => Ending::BadMagic {
                offset: tc.draw(integers().max_value(MAGIC.len() - 1)),
            },
        }
    }

    #[hegel::test]
    fn reads_on_without_waiting_when_no_end_is_to_come(tc: TestCase) {
        let mut bytes = tc.draw(trace(Some(false))).bytes;
        let ending = tc.draw(ending());
        match ending {
            Ending::End { payload } => {
                crate::write_record(&mut bytes, tag::END, &[&vec![7; payload]])
                    .unwrap();
            }
            Ending::BadHeader => {
                let mut header =
                    RecordHeader::new(tag::STATE, 3, 0).as_bytes().to_vec();
                let bit = tc.draw(integers::<usize>().max_value(12 * 8 - 1));
                header[bit / 8] ^= 1 << (bit % 8);
                bytes.extend(header);
            }
            Ending::BadMagic { offset } => {
                bytes[offset] ^= 1 << tc.draw(integers::<u8>().max_value(7));
                bytes.truncate(offset + 1);
            }
        }
        let rest = tc.draw(integers::<usize>().max_value(40));
        if !matches!(ending, Ending::BadMagic { .. }) {
            bytes.extend(std::iter::repeat_n(0xa5, rest));
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&path, &bytes).unwrap();
        let wait = |_| panic!("waited though no end record can come");
        let mut follower = FollowReader::new(File::open(&path).unwrap(), wait);
        let mut read = Vec::new();
        io::Read::read_to_end(&mut follower, &mut read).unwrap();
        assert_eq!(read, bytes);
    }

    #[hegel::test]
    fn tells_a_finished_trace_by_its_last_bytes(tc: TestCase) {
        let trace = tc.draw(trace(None));
        let cut = if tc.draw(booleans()) {
            trace.bytes.len()
        } else {
            tc.draw(integers::<usize>().max_value(trace.bytes.len()))
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&path, &trace.bytes[..cut]).unwrap();
        let mut file = File::open(&path).unwrap();
        assert_eq!(
            ends_with_end_record(&mut file).unwrap(),
            trace.finished && cut == trace.bytes.len()
        );
        let mut read = Vec::new();
        io::Read::read_to_end(&mut file, &mut read).unwrap();
        assert_eq!(read, &trace.bytes[..cut]);
    }
}
