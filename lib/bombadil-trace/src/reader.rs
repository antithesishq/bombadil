use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;

use crate::{
    ENVELOPE_LEN, RECORD_HEADER_LEN, TRACE_FILE_NAME, TraceDriver,
    envelope::read_envelope,
    record::{RawRecord, RecordHeader, RecordReader, read_full},
    tag,
};

/// One state read back from a trace.
#[derive(Debug)]
pub struct State<M> {
    /// Position of the state in the trace, from 0.
    pub index: usize,
    pub metadata: M,
    /// The state's driver-defined bytes, if it has any and they weren't
    /// skipped (see [`StateReader::without_payloads`]).
    pub payload: Option<Vec<u8>>,
    /// Byte offset of the payload in the trace file, for reading it again
    /// later with [`read_payload_at`] without holding it in memory.
    pub payload_offset: Option<u64>,
    /// Size of the payload in bytes, even if it was skipped.
    pub payload_length: Option<usize>,
}

/// Reads the states of a trace, in the order they were written, with
/// their metadata deserialized as `M`.
///
/// A bad record, or a state missing its payload, ends the states with an
/// error, after every complete state before it. An incomplete last
/// record, or a last state whose payload the trace ends before (from a
/// run killed while writing), ends them with a logged warning instead,
/// and without that state. A trace without a [`tag::END`] record also
/// ends with a logged warning; see [`finished`](Self::finished).
pub struct StateReader<R, M> {
    pub driver: TraceDriver,
    records: RecordReader<R>,
    states_read: usize,
    // A state whose payload hasn't been read yet.
    pending: Option<PendingState<M>>,
    // Whether iteration ended, with or without an error.
    ended: bool,
    // Whether the end record was read.
    finished: bool,
    keep_payloads: bool,
}

struct PendingState<M> {
    metadata: M,
    // The index of the record holding the metadata.
    record: usize,
}

/// Opens a trace file, or the trace in a run directory.
pub fn open_trace<M: DeserializeOwned>(
    path: &Path,
) -> Result<StateReader<BufReader<File>, M>> {
    let path = trace_file(path);
    let file = File::open(&path).with_context(|| {
        format!("failed to open trace file {}", path.display())
    })?;
    StateReader::new(BufReader::new(file))
        .with_context(|| format!("failed to read {}", path.display()))
}

fn trace_file(path: &Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.join(TRACE_FILE_NAME)
    } else {
        path.to_path_buf()
    }
}

/// Reads the payload at `offset` (a [`State::payload_offset`]) of the
/// trace file at `path`, or of the trace in a run directory, as with
/// [`open_trace`].
///
/// Checks that the file is a bombadil trace in the supported format
/// version, and that an intact [`tag::STATE_PAYLOAD`] record (both of its
/// checksums match) holds the payload at `offset`. It doesn't check that
/// this is the payload of the state the offset was read for: if the file
/// was replaced by another trace since, this may fail or return another
/// state's payload. To read the payloads of the file that was read,
/// whatever the path names since, keep it open and use
/// [`read_payload_from`].
pub fn read_payload_at(path: &Path, offset: u64) -> Result<Vec<u8>> {
    let path = trace_file(path);
    File::open(&path)
        .map_err(anyhow::Error::from)
        .and_then(|file| read_payload(&file, offset))
        .with_context(|| {
            format!(
                "failed to read the state payload at offset {offset} of {}",
                path.display()
            )
        })
}

/// [`read_payload_at`] of the open trace file `file`, with the same
/// checks, so the payload is from that file even if its path has been
/// replaced since.
///
/// On Unix and Windows, concurrent calls on one handle always read the
/// right bytes, as each read names its offset. On Unix the cursor isn't
/// used at all, so this can also run alongside a reader of the same
/// handle (e.g. a [`StateReader`] of a [`File::try_clone`] of it), even
/// concurrently. On Windows each read moves the cursor and it is put
/// back after, so a single call leaves it where it was, but concurrent
/// calls can interleave their saving and restoring and leave it
/// anywhere: don't mix concurrent calls with cursor-based reads of the
/// same handle. On other platforms reads seek first, so don't call this
/// concurrently on one handle at all.
pub fn read_payload_from(file: &File, offset: u64) -> Result<Vec<u8>> {
    read_payload(file, offset).with_context(|| {
        format!("failed to read the state payload at offset {offset}")
    })
}

/// Reads `file` from `position` on, as [`read_payload_from`] says.
struct PositionalReader<'a> {
    file: &'a File,
    position: u64,
}

impl Read for PositionalReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(unix)]
        let read =
            std::os::unix::fs::FileExt::read_at(self.file, buf, self.position)?;
        // Elsewhere reads move the cursor, which is put back. Concurrent
        // calls can interleave saving and restoring it, so they may leave
        // it anywhere (see `read_payload_from`).
        #[cfg(not(unix))]
        let read = {
            use std::io::{Seek, SeekFrom};
            let mut file = self.file;
            let cursor = file.stream_position()?;
            #[cfg(windows)]
            let read = std::os::windows::fs::FileExt::seek_read(
                self.file,
                buf,
                self.position,
            );
            #[cfg(not(windows))]
            let read = file
                .seek(SeekFrom::Start(self.position))
                .and_then(|_| file.read(buf));
            file.seek(SeekFrom::Start(cursor))?;
            read?
        };
        self.position += read as u64;
        Ok(read)
    }
}

fn read_payload(file: &File, offset: u64) -> Result<Vec<u8>> {
    read_envelope(&mut PositionalReader { file, position: 0 })?;
    let header_offset = offset
        .checked_sub(RECORD_HEADER_LEN as u64)
        .filter(|&header| header >= ENVELOPE_LEN as u64)
        .context("not a payload offset")?;
    let file_length = file.metadata()?.len();
    if offset > file_length {
        bail!("past the end of the trace ({file_length} bytes)");
    }
    let mut file = PositionalReader {
        file,
        position: header_offset,
    };
    let mut header = [0; RECORD_HEADER_LEN];
    if read_full(&mut file, &mut header)? < header.len() {
        bail!("no state payload there");
    }
    // The offset was given by the caller, so a mismatch more likely
    // means it isn't a payload offset than that the file is damaged.
    if !RecordHeader::checksum_matches(&header) {
        bail!(
            "no state payload at offset {offset} (no record header \
             before it)"
        );
    }
    let header = RecordHeader::parse(header)?;
    if header.tag != tag::STATE_PAYLOAD {
        bail!("no state payload there (record tag {})", header.tag);
    }
    match header.read_payload(&mut file, true)? {
        Some(Some(payload)) => Ok(payload),
        _ => bail!("truncated state payload"),
    }
}

impl<R: Read, M: DeserializeOwned> StateReader<R, M> {
    /// Reads the envelope from `reader`; states are read on iteration.
    pub fn new(mut reader: R) -> Result<Self> {
        let driver = read_envelope(&mut reader)?;
        Ok(StateReader {
            driver,
            records: RecordReader::new(reader),
            states_read: 0,
            pending: None,
            ended: false,
            finished: false,
            keep_payloads: true,
        })
    }

    /// Whether the trace ended with its [`tag::END`] record, as a trace
    /// does when the writer finished it. Known once the states are read.
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// Fails unless the trace was written by `driver`.
    pub fn expect_driver(self, driver: TraceDriver) -> Result<Self> {
        if self.driver != driver {
            bail!(
                "not a {driver} trace (written by the {} driver)",
                self.driver
            );
        }
        Ok(self)
    }

    /// Checks payloads but doesn't keep them, for readers that only need
    /// the metadata, or the payloads' offsets to read them later.
    pub fn without_payloads(mut self) -> Self {
        self.keep_payloads = false;
        self
    }

    fn state(&mut self, metadata: M, payload: Option<RawRecord>) -> State<M> {
        let index = self.states_read;
        self.states_read += 1;
        let (payload_offset, payload_length, payload) = match payload {
            Some(record) => (
                Some(record.payload_offset),
                Some(record.payload_length),
                record.payload,
            ),
            None => (None, None, None),
        };
        State {
            index,
            metadata,
            payload,
            payload_offset,
            payload_length,
        }
    }

    fn read_state(&mut self) -> Result<Option<State<M>>> {
        loop {
            let Some(record) = self.records.next_record(self.keep_payloads)
            else {
                if self.pending.take().is_some() {
                    log::warn!(
                        "trace ends before the payload of state {} (the run \
                         was probably killed while writing it); reading up \
                         to that state",
                        self.states_read
                    );
                }
                log::warn!(
                    "trace was not finished (it has no end record): the run \
                     may have been killed"
                );
                return Ok(None);
            };
            let record = record?;
            match record.tag {
                tag::END => {
                    if let Some(pending) = &self.pending {
                        bail!(
                            "state {} in trace record {} is missing its \
                             payload (trace record {} is the end)",
                            self.states_read,
                            pending.record,
                            record.index
                        );
                    }
                    if record.payload_length != 0 {
                        bail!(
                            "invalid end of trace in trace record {} (it \
                             has a payload)",
                            record.index
                        );
                    }
                    self.records.expect_end()?;
                    self.finished = true;
                    return Ok(None);
                }
                tag::STATE | tag::STATE_WITH_PAYLOAD => {
                    if let Some(pending) = &self.pending {
                        bail!(
                            "state {} in trace record {} is missing its \
                             payload (trace record {} is another state)",
                            self.states_read,
                            pending.record,
                            record.index
                        );
                    }
                    let metadata = serde_json::from_slice(
                        record.payload.as_deref().unwrap_or_default(),
                    )
                    .with_context(|| {
                        format!(
                            "invalid state in trace record {}",
                            record.index
                        )
                    })?;
                    if record.tag == tag::STATE {
                        return Ok(Some(self.state(metadata, None)));
                    }
                    self.pending = Some(PendingState {
                        metadata,
                        record: record.index,
                    });
                }
                tag::STATE_PAYLOAD => {
                    let Some(pending) = self.pending.take() else {
                        bail!(
                            "state payload without a state expecting one in \
                             trace record {}",
                            record.index
                        );
                    };
                    return Ok(Some(
                        self.state(pending.metadata, Some(record)),
                    ));
                }
                tag if tag & tag::OPTIONAL != 0 => {}
                tag => {
                    bail!(
                        "unsupported tag {tag} in trace record {}",
                        record.index
                    );
                }
            }
        }
    }
}

impl<R: Read, M: DeserializeOwned> Iterator for StateReader<R, M> {
    type Item = Result<State<M>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }
        let result = self.read_state().transpose();
        // Once ended, stay ended, so warnings aren't logged again and no
        // state is read after an error.
        self.ended = !matches!(result, Some(Ok(_)));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_log::{capture_warnings, warned},
        write_envelope, write_record,
    };
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Metadata {
        n: u32,
    }

    fn trace(records: &[(u16, &[u8])]) -> Vec<u8> {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        for (tag, payload) in records {
            write_record(&mut file, *tag, &[payload]).unwrap();
        }
        file
    }

    fn states(file: &[u8]) -> Result<Vec<State<Metadata>>> {
        StateReader::new(file)?.collect()
    }

    // Records start at 16, 37, 55, 76 and 97, and the file ends at 114.
    fn three_states() -> Vec<u8> {
        trace(&[
            (tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#),
            (tag::STATE_PAYLOAD, b"zero"),
            (tag::STATE, br#"{"n":1}"#),
            (tag::STATE_WITH_PAYLOAD, br#"{"n":2}"#),
            (tag::STATE_PAYLOAD, b"two"),
        ])
    }

    #[test]
    fn pairs_states_with_their_payloads() {
        let file = three_states();
        let states = states(&file).unwrap();
        assert_eq!(states.len(), 3);
        assert_eq!(states[0].metadata, Metadata { n: 0 });
        assert_eq!(states[0].payload.as_deref(), Some(&b"zero"[..]));
        assert_eq!(states[1].payload, None);
        assert_eq!(states[1].payload_length, None);
        assert_eq!(states[2].index, 2);
        assert_eq!(states[2].payload.as_deref(), Some(&b"two"[..]));

        let offset = states[2].payload_offset.unwrap() as usize;
        assert_eq!(&file[offset..offset + 3], b"two");
    }

    #[test]
    fn reads_payloads_at_their_offsets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&path, three_states()).unwrap();
        let states = open_trace::<Metadata>(&path)
            .unwrap()
            .without_payloads()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(states[2].payload, None);
        assert_eq!(states[2].payload_length, Some(3));
        let offset = states[2].payload_offset.unwrap();
        assert_eq!(read_payload_at(&path, offset).unwrap(), b"two");

        // Not a payload record: a state's metadata, the middle of a
        // payload, the envelope, or past the end.
        for wrong in [
            ENVELOPE_LEN as u64 + RECORD_HEADER_LEN as u64,
            offset + 1,
            0,
            offset + 4,
            u64::MAX,
        ] {
            let error = read_payload_at(&path, wrong).unwrap_err();
            let error = format!("{error:#}");
            assert!(error.contains(&format!("offset {wrong}")), "{error}");
        }
        let error =
            format!("{:#}", read_payload_at(&path, u64::MAX).unwrap_err());
        assert!(error.contains("past the end"), "{error}");
        // In the middle of a record: a wrong offset, not a damaged file.
        let error =
            format!("{:#}", read_payload_at(&path, offset + 1).unwrap_err());
        assert!(
            error.contains(&format!(
                "no state payload at offset {}",
                offset + 1
            )) && !error.contains("corrupt"),
            "{error}"
        );

        // A run directory reads its trace.bin, as with `open_trace`.
        assert_eq!(read_payload_at(directory.path(), offset).unwrap(), b"two");

        // An empty payload at the very end: its offset is the file length,
        // which is not past the end.
        let file = trace(&[
            (tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#),
            (tag::STATE_PAYLOAD, b""),
        ]);
        std::fs::write(&path, &file).unwrap();
        let file_len = file.len() as u64;
        assert_eq!(read_payload_at(&path, file_len).unwrap(), b"");
    }

    /// An open file's payloads are read from it, even after its path
    /// names another trace, at any offset, without moving its cursor.
    #[test]
    fn reads_payloads_from_an_open_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&path, three_states()).unwrap();
        let offset = open_trace::<Metadata>(&path)
            .unwrap()
            .nth(2)
            .unwrap()
            .unwrap()
            .payload_offset
            .unwrap();
        let file = File::open(&path).unwrap();
        // With its cursor moved, as a reader of it would.
        (&file).read_exact(&mut [0; 5]).unwrap();
        // Another trace at the path, with a payload at the same offset.
        std::fs::remove_file(&path).unwrap();
        let other = trace(&[
            (tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#),
            (tag::STATE_PAYLOAD, b"zero"),
            (tag::STATE, br#"{"n":1}"#),
            (tag::STATE_WITH_PAYLOAD, br#"{"n":2}"#),
            (tag::STATE_PAYLOAD, b"TWO"),
        ]);
        std::fs::write(&path, &other).unwrap();
        assert_eq!(read_payload_at(&path, offset).unwrap(), b"TWO");
        assert_eq!(read_payload_from(&file, offset).unwrap(), b"two");
        // The same checks as by path.
        let error = read_payload_from(&file, offset + 1).unwrap_err();
        assert!(
            format!("{error:#}").contains(&format!("offset {}", offset + 1)),
            "{error:#}"
        );
        // The cursor is where it was.
        use std::io::Seek;
        assert_eq!((&file).stream_position().unwrap(), 5);
    }

    /// Fails once with `error` after `fail_after` bytes, then reads on.
    /// Panics if called too often, so a retry loop can't spin forever.
    struct FailingReader<'a> {
        bytes: &'a [u8],
        fail_after: usize,
        error: Option<std::io::ErrorKind>,
        calls: usize,
    }

    impl Read for FailingReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            assert!(self.calls < 10_000, "read called too often");
            if self.error.is_none() {
                return self.bytes.read(buffer);
            }
            if self.fail_after == 0 {
                let kind = self.error.take().unwrap();
                return Err(std::io::Error::new(kind, "test error"));
            }
            let limit = self.fail_after.min(buffer.len());
            let read = self.bytes.read(&mut buffer[..limit])?;
            self.fail_after -= read;
            Ok(read)
        }
    }

    #[test]
    fn retries_interrupted_reads_and_stops_at_other_errors() {
        let file = three_states();
        let read = |fail_after, error| {
            StateReader::<_, Metadata>::new(FailingReader {
                bytes: &file,
                fail_after,
                error: Some(error),
                calls: 0,
            })
            .unwrap()
            .collect::<Vec<_>>()
        };

        // Interrupted before any data, or mid-record: every state.
        for fail_after in [0, 40] {
            let states = read(fail_after, std::io::ErrorKind::Interrupted);
            assert_eq!(states.len(), 3, "interrupted after {fail_after}");
            assert!(states.iter().all(Result::is_ok));
        }

        // A hard error mid-stream (in the second state's record) ends
        // the states with it.
        let states = read(60, std::io::ErrorKind::Other);
        assert_eq!(states.len(), 2);
        assert!(states[0].is_ok());
        let error = format!("{:#}", states[1].as_ref().unwrap_err());
        assert!(error.contains("test error"), "{error}");
    }

    #[test]
    fn reports_whether_the_trace_was_finished() {
        capture_warnings();
        let read = |file: &[u8]| {
            let mut reader = StateReader::<_, Metadata>::new(file).unwrap();
            let states = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
            assert!(reader.next().is_none());
            (states.len(), reader.finished())
        };

        let mut file = three_states();
        // Cut at a state boundary: every state, but not finished.
        assert_eq!(read(&file), (3, false));
        assert!(warned("trace was not finished"));

        write_record(&mut file, tag::END, &[]).unwrap();
        assert_eq!(read(&file), (3, true));
    }

    #[test]
    fn rejects_records_after_the_end() {
        let file = trace(&[
            (tag::STATE, br#"{"n":0}"#),
            (tag::END, b""),
            (tag::STATE, br#"{"n":1}"#),
        ]);
        let results: Vec<_> = StateReader::<_, Metadata>::new(&file[..])
            .unwrap()
            .collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap().metadata, Metadata { n: 0 });
        let error = format!("{:#}", results[1].as_ref().unwrap_err());
        assert!(error.contains("continues after its end"), "{error}");

        // Even a stray byte, or an optional record.
        let mut stray = trace(&[(tag::END, b"")]);
        stray.push(0);
        assert!(states(&stray).is_err());
        let optional = trace(&[(tag::END, b""), (tag::OPTIONAL | 7, b"")]);
        assert!(states(&optional).is_err());
    }

    #[test]
    fn rejects_a_misplaced_or_nonempty_end() {
        let file =
            trace(&[(tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#), (tag::END, b"")]);
        let error = format!("{:#}", states(&file).unwrap_err());
        assert!(error.contains("missing its payload"), "{error}");

        let file = trace(&[(tag::END, b"x")]);
        let error = format!("{:#}", states(&file).unwrap_err());
        assert!(error.contains("has a payload"), "{error}");
    }

    #[test]
    fn read_payload_at_rejects_other_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.jsonl");
        let line = "{\"timestamp\":0}\n";
        std::fs::write(&path, line.repeat(8)).unwrap();
        let offset = (ENVELOPE_LEN + RECORD_HEADER_LEN) as u64;
        let error =
            format!("{:#}", read_payload_at(&path, offset).unwrap_err());
        assert!(error.contains("not a bombadil trace"), "{error}");
    }

    #[test]
    fn drops_a_last_state_whose_payload_is_lost() {
        capture_warnings();
        let file = three_states();
        // Cut after state 2's metadata, at a record boundary, and inside
        // its payload record's header and payload.
        for length in [97, 100, 113] {
            let states = states(&file[..length]).unwrap();
            assert_eq!(states.len(), 2, "cut at {length}");
            assert_eq!(states[1].metadata, Metadata { n: 1 });
        }
        assert!(warned("trace ends before the payload of state 2"));
    }

    #[test]
    fn rejects_a_state_whose_payload_is_lost_mid_trace() {
        let file = trace(&[
            (tag::STATE, br#"{"n":0}"#),
            (tag::STATE_WITH_PAYLOAD, br#"{"n":1}"#),
            (tag::STATE, br#"{"n":2}"#),
        ]);
        let results: Vec<_> = StateReader::<_, Metadata>::new(&file[..])
            .unwrap()
            .collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap().metadata, Metadata { n: 0 });
        let error = format!("{:#}", results[1].as_ref().unwrap_err());
        assert!(
            error.contains("state 1 in trace record 1 is missing its payload"),
            "{error}"
        );
    }

    #[test]
    fn keeps_every_complete_state_before_a_bad_record() {
        let file = three_states();
        // State 2's metadata (bytes 76..97) cut short: states 0 and 1
        // are complete, and the trace ends without an error.
        let states = states(&file[..85]).unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[1].metadata, Metadata { n: 1 });

        // State 2's payload corrupted: states 0 and 1, then the error.
        // State 2 isn't complete without its payload.
        let mut corrupt = file.clone();
        corrupt[file.len() - 1] ^= 1;
        let results: Vec<_> = StateReader::<_, Metadata>::new(&corrupt[..])
            .unwrap()
            .collect();
        assert_eq!(results.len(), 3);
        assert_eq!(results[1].as_ref().unwrap().metadata, Metadata { n: 1 });
        let error = format!("{:#}", results[2].as_ref().unwrap_err());
        assert!(error.contains("checksum"), "{error}");
    }

    #[test]
    fn skips_unknown_optional_records() {
        let file = trace(&[
            (tag::OPTIONAL | 7, b"ignored"),
            (tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#),
            (tag::OPTIONAL | 7, b"ignored"),
            (tag::STATE_PAYLOAD, b"zero"),
        ]);
        let states = states(&file).unwrap();
        assert_eq!(states.len(), 1);
        assert!(states[0].payload.is_some());
    }

    #[test]
    fn rejects_unknown_required_and_misplaced_records() {
        let error = states(&trace(&[(7, b"unknown")])).unwrap_err();
        assert!(error.to_string().contains("unsupported tag 7"), "{error}");

        // Iteration stops at the error, without reading the states after
        // it.
        let file = trace(&[
            (tag::STATE, br#"{"n":0}"#),
            (7, b"unknown"),
            (tag::STATE, br#"{"n":1}"#),
            (tag::END, b""),
        ]);
        let mut reader = StateReader::<_, Metadata>::new(&file[..]).unwrap();
        assert_eq!(reader.next().unwrap().unwrap().metadata, Metadata { n: 0 });
        assert!(reader.next().unwrap().is_err());
        assert!(reader.next().is_none());
        assert!(reader.next().is_none());

        for records in [
            &[(tag::STATE_PAYLOAD, &b"x"[..])][..],
            &[(tag::STATE, br#"{"n":0}"#), (tag::STATE_PAYLOAD, b"x")],
        ] {
            let error = states(&trace(records)).unwrap_err();
            assert!(
                error.to_string().contains("state payload without a state"),
                "{error}"
            );
        }

        let error = states(&trace(&[(tag::STATE, b"not json")])).unwrap_err();
        assert!(error.to_string().contains("invalid state"), "{error}");
    }

    #[test]
    fn checks_the_driver() {
        let file = trace(&[]);
        let error = StateReader::<_, Metadata>::new(file.as_slice())
            .unwrap()
            .expect_driver(TraceDriver::Browser)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "not a browser trace (written by the terminal driver)"
        );
    }
}
