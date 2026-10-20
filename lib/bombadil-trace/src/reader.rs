use std::{
    fs::File,
    io::{BufReader, Read, Seek},
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;

use crate::{
    ENVELOPE_LEN, RECORD_HEADER_LEN, TRACE_FILE_NAME, TraceDriver,
    envelope::read_envelope,
    record::{PayloadRef, RawRecord, RecordHeader, RecordReader, read_full},
    snapshots::SnapshotRestorer,
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
    /// later with [`read_payload_at`] without holding it in memory. For a
    /// payload shared with an earlier state, that of the earlier state's.
    pub payload_offset: Option<u64>,
    /// Size of the payload in bytes, even if it was skipped.
    pub payload_length: Option<usize>,
    /// The state whose record holds the payload: this one, or an earlier
    /// state with the same payload (see [`tag::STATE_PAYLOAD_REF`]).
    pub payload_from: Option<usize>,
}

/// Where a state's payload is, to resolve references to it.
#[derive(Clone, Copy)]
struct PayloadLocation {
    offset: u64,
    length: usize,
    crc: u32,
    // The state whose record holds it.
    from: usize,
}

/// Reads the states of a trace, in the order they were written, with
/// their metadata deserialized as `M`.
///
/// A state whose payload is shared with an earlier state (see
/// [`tag::STATE_PAYLOAD_REF`]) gets the earlier state's offset and
/// length, and, unless payloads are skipped, its bytes, read again from
/// the record that holds them, once per reference: hence `R: Seek`. Read
/// from a pipe, such a state fails unless payloads are skipped.
///
/// Snapshot values written as references to an earlier state's (see the
/// [crate] docs) are put back in the metadata before it is deserialized,
/// so `M` reads the metadata as encoded by the writer. Metadata without
/// such references is deserialized as written.
///
/// A bad record, a state missing its payload, or a payload or snapshot
/// value reference that doesn't resolve ends the states with an error,
/// after every complete state before it. An incomplete last record, or a
/// last state whose payload the trace ends before (from a run killed
/// while writing), ends them with a logged warning instead, and without
/// that state. A trace without a [`tag::END`] record also ends with a
/// logged warning; see [`finished`](Self::finished).
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
    // Where the payloads in records of their own are, in the order of
    // their states (`PayloadLocation::from`), to resolve references to
    // them.
    payloads: Vec<PayloadLocation>,
    // Puts back the snapshot values written as references.
    snapshots: SnapshotRestorer,
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
pub(crate) struct PositionalReader<'a> {
    pub file: &'a File,
    pub position: u64,
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

impl<R: Read + Seek, M: DeserializeOwned> StateReader<R, M> {
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
            payloads: Vec::new(),
            snapshots: SnapshotRestorer::default(),
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

    /// The next state, with its payload (unless skipped) and where that
    /// is, if it has one.
    fn state(
        &mut self,
        metadata: M,
        payload: Option<(Option<Vec<u8>>, PayloadLocation)>,
    ) -> State<M> {
        let index = self.states_read;
        self.states_read += 1;
        let location = payload.as_ref().map(|(_, location)| *location);
        if let Some(location) = location
            && location.from == index
        {
            self.payloads.push(location);
        }
        State {
            index,
            metadata,
            payload: payload.and_then(|(payload, _)| payload),
            payload_offset: location.map(|location| location.offset),
            payload_length: location.map(|location| location.length),
            payload_from: location.map(|location| location.from),
        }
    }

    /// Where the payload that the [`tag::STATE_PAYLOAD_REF`] `record`
    /// refers to is, checked against the reference.
    fn resolve(&self, record: &RawRecord) -> Result<PayloadLocation> {
        let corrupt = |reason: String| {
            anyhow!(
                "corrupt payload reference in trace record {} ({reason})",
                record.index
            )
        };
        // The record reader keeps a reference's payload and checks its
        // length.
        let reference = record
            .payload
            .as_deref()
            .and_then(PayloadRef::parse)
            .expect("a payload reference record of the right length");
        let state = reference.state as usize;
        if state >= self.states_read {
            return Err(corrupt(format!(
                "to state {state}, which isn't before state {}",
                self.states_read
            )));
        }
        // Only to a payload in a record of its own, as the writer writes
        // them: not to a state without a payload, nor to a reference.
        let Ok(found) = self
            .payloads
            .binary_search_by_key(&state, |location| location.from)
        else {
            return Err(corrupt(format!(
                "to state {state}, which has no payload in a record of its \
                 own"
            )));
        };
        let location = self.payloads[found];
        if location.length != reference.length as usize
            || location.crc != reference.crc
        {
            return Err(corrupt(format!(
                "to state {state}, whose payload's length or checksum \
                 differs"
            )));
        }
        Ok(location)
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
                    // No state is pending, so this is the next one.
                    let metadata = self.snapshots.restore(
                        record.payload.as_deref().unwrap_or_default(),
                        self.states_read,
                        record.index,
                    )?;
                    let metadata = serde_json::from_slice(&metadata)
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
                tag::STATE_PAYLOAD | tag::STATE_PAYLOAD_REF => {
                    let Some(pending) = self.pending.take() else {
                        let what = if record.tag == tag::STATE_PAYLOAD {
                            "state payload"
                        } else {
                            "payload reference"
                        };
                        bail!(
                            "{what} without a state expecting one in trace \
                             record {}",
                            record.index
                        );
                    };
                    if record.tag == tag::STATE_PAYLOAD {
                        let location = PayloadLocation {
                            offset: record.payload_offset,
                            length: record.payload_length,
                            crc: record.payload_crc,
                            from: self.states_read,
                        };
                        return Ok(Some(self.state(
                            pending.metadata,
                            Some((record.payload, location)),
                        )));
                    }
                    let location = self.resolve(&record)?;
                    // Read from the record that holds it, and checked
                    // against the reference as well as that record.
                    let payload = if self.keep_payloads {
                        let payload = self
                            .records
                            .read_payload_again(
                                location.offset,
                                location.length,
                                location.crc,
                            )
                            .with_context(|| {
                                format!(
                                    "failed to read the payload of state {} \
                                     that trace record {} refers to",
                                    location.from, record.index
                                )
                            })?;
                        Some(payload)
                    } else {
                        None
                    };
                    return Ok(Some(
                        self.state(pending.metadata, Some((payload, location))),
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

impl<R: Read + Seek, M: DeserializeOwned> Iterator for StateReader<R, M> {
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
    use std::io::{Cursor, SeekFrom};

    use crate::{
        record::PayloadRef,
        test_log::{capture_warnings, warned},
        write_envelope, write_record,
    };
    use serde::Deserialize;
    use serde_json::value::RawValue;

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
        StateReader::new(Cursor::new(file))?.collect()
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
        bytes: Cursor<&'a [u8]>,
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

    impl Seek for FailingReader<'_> {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.bytes.seek(position)
        }
    }

    #[test]
    fn retries_interrupted_reads_and_stops_at_other_errors() {
        let file = three_states();
        let read = |fail_after, error| {
            StateReader::<_, Metadata>::new(FailingReader {
                bytes: Cursor::new(&file),
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
            let mut reader =
                StateReader::<_, Metadata>::new(Cursor::new(file)).unwrap();
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
        let results: Vec<_> =
            StateReader::<_, Metadata>::new(Cursor::new(&file[..]))
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
        let results: Vec<_> =
            StateReader::<_, Metadata>::new(Cursor::new(&file[..]))
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
        let results: Vec<_> =
            StateReader::<_, Metadata>::new(Cursor::new(&corrupt[..]))
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
        let mut reader =
            StateReader::<_, Metadata>::new(Cursor::new(&file[..])).unwrap();
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

    /// A payload reference record's payload: to `state`, whose payload is
    /// `payload`.
    fn reference(state: u32, payload: &[u8]) -> Vec<u8> {
        PayloadRef {
            state,
            length: payload.len() as u32,
            crc: crc32fast::hash(payload),
        }
        .encode()
        .to_vec()
    }

    /// States 1 and 3 share state 0's payload; state 2 has its own.
    fn shared_payloads() -> Vec<u8> {
        trace(&[
            (tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#),
            (tag::STATE_PAYLOAD, b"zero"),
            (tag::STATE_WITH_PAYLOAD, br#"{"n":1}"#),
            (tag::STATE_PAYLOAD_REF, &reference(0, b"zero")),
            (tag::STATE_WITH_PAYLOAD, br#"{"n":2}"#),
            (tag::STATE_PAYLOAD, b"two"),
            (tag::STATE, br#"{"n":3}"#),
            (tag::STATE_WITH_PAYLOAD, br#"{"n":4}"#),
            (tag::STATE_PAYLOAD_REF, &reference(0, b"zero")),
            (tag::END, b""),
        ])
    }

    // The payload of each state of `shared_payloads`.
    const SHARED_PAYLOADS: [Option<&[u8]>; 5] = [
        Some(b"zero"),
        Some(b"zero"),
        Some(b"two"),
        None,
        Some(b"zero"),
    ];

    #[test]
    fn resolves_payload_references() {
        let file = shared_payloads();
        let mut reader =
            StateReader::<_, Metadata>::new(Cursor::new(&file)).unwrap();
        let states = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert!(reader.finished());
        let payloads: Vec<_> = states
            .iter()
            .map(|state| state.payload.as_deref())
            .collect();
        assert_eq!(payloads, SHARED_PAYLOADS);
        let from: Vec<_> =
            states.iter().map(|state| state.payload_from).collect();
        assert_eq!(from, [Some(0), Some(0), Some(2), None, Some(0)]);
        for state in [&states[1], &states[4]] {
            assert_eq!(state.payload_offset, states[0].payload_offset);
            assert_eq!(state.payload_length, Some(4));
        }
        // Reading carries on where it left off after reading again.
        assert_eq!(states[4].metadata, Metadata { n: 4 });

        // Without payloads, the same locations, and nothing read again.
        let skipped = StateReader::<_, Metadata>::new(Cursor::new(&file))
            .unwrap()
            .without_payloads()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        for (skipped, state) in skipped.iter().zip(&states) {
            assert_eq!(skipped.payload, None);
            assert_eq!(skipped.payload_offset, state.payload_offset);
            assert_eq!(skipped.payload_length, state.payload_length);
            assert_eq!(skipped.payload_from, state.payload_from);
        }
    }

    /// A trace cut anywhere reads without an error, as every state before
    /// the cut with its own payload: a cut reference never passes for a
    /// state without a payload, nor resolves to another payload.
    #[test]
    fn reads_a_trace_with_references_cut_anywhere() {
        let file = shared_payloads();
        let mut previous = 0;
        for length in ENVELOPE_LEN..=file.len() {
            for keep in [true, false] {
                let mut reader = StateReader::<_, Metadata>::new(Cursor::new(
                    &file[..length],
                ))
                .unwrap();
                if !keep {
                    reader = reader.without_payloads();
                }
                let states = reader.collect::<Result<Vec<_>>>().unwrap_or_else(
                    |error| panic!("cut at {length}: {error:#}"),
                );
                assert!(states.len() >= previous, "cut at {length}");
                previous = states.len();
                for state in &states {
                    let expected = SHARED_PAYLOADS[state.index];
                    assert_eq!(
                        state.payload_length,
                        expected.map(<[u8]>::len),
                        "cut at {length}"
                    );
                    if keep {
                        assert_eq!(
                            state.payload.as_deref(),
                            expected,
                            "cut at {length}"
                        );
                    }
                }
            }
        }
        assert_eq!(previous, SHARED_PAYLOADS.len());
    }

    #[test]
    fn rejects_references_that_dont_resolve() {
        // State 2's payload refers to `reference`, after state 0 with a
        // payload and state 1 without.
        let error = |reference: &[u8], keep: bool| {
            let file = trace(&[
                (tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#),
                (tag::STATE_PAYLOAD, b"zero"),
                (tag::STATE, br#"{"n":1}"#),
                (tag::STATE_WITH_PAYLOAD, br#"{"n":2}"#),
                (tag::STATE_PAYLOAD_REF, reference),
                (tag::END, b""),
            ]);
            let mut reader =
                StateReader::<_, Metadata>::new(Cursor::new(file)).unwrap();
            if !keep {
                reader = reader.without_payloads();
            }
            let results: Vec<_> = reader.collect();
            // The states before it, then the error.
            assert_eq!(results.len(), 3);
            assert_eq!(
                results[1].as_ref().unwrap().metadata,
                Metadata { n: 1 }
            );
            let error = format!("{:#}", results[2].as_ref().unwrap_err());
            assert!(
                error.contains("corrupt payload reference in trace record 4"),
                "{error}"
            );
            error
        };
        for keep in [true, false] {
            for (reference, reason) in [
                (reference(1, b"zero"), "which has no payload"),
                (reference(2, b"zero"), "which isn't before state 2"),
                (reference(5, b"zero"), "which isn't before state 2"),
                (reference(u32::MAX, b"zero"), "which isn't before state 2"),
                (reference(0, b"zerO"), "checksum differs"),
                (reference(0, b"zero!"), "length or checksum differs"),
                // The right checksum, the wrong length.
                (
                    PayloadRef {
                        state: 0,
                        length: 5,
                        crc: crc32fast::hash(b"zero"),
                    }
                    .encode()
                    .to_vec(),
                    "length or checksum differs",
                ),
            ] {
                let error = error(&reference, keep);
                assert!(error.contains(reason), "{error}");
            }
        }

        // Of the wrong length: rejected by the record reader, as a bad
        // record, after the states before it.
        for (reference, length) in [
            (b"xy".to_vec(), 2),
            ([reference(0, b"zero"), vec![0]].concat(), 13),
        ] {
            let file = trace(&[
                (tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#),
                (tag::STATE_PAYLOAD, b"zero"),
                (tag::STATE_WITH_PAYLOAD, br#"{"n":1}"#),
                (tag::STATE_PAYLOAD_REF, &reference),
            ]);
            let results: Vec<_> =
                StateReader::<_, Metadata>::new(Cursor::new(file))
                    .unwrap()
                    .collect();
            assert_eq!(results.len(), 2);
            let error = format!("{:#}", results[1].as_ref().unwrap_err());
            assert!(
                error.contains(&format!(
                    "failed to read trace record 3: corrupt payload \
                     reference ({length} bytes, not 12)"
                )),
                "{error}"
            );
        }

        // Not to another reference, even one that resolves.
        for keep in [true, false] {
            let file = trace(&[
                (tag::STATE_WITH_PAYLOAD, br#"{"n":0}"#),
                (tag::STATE_PAYLOAD, b"zero"),
                (tag::STATE_WITH_PAYLOAD, br#"{"n":1}"#),
                (tag::STATE_PAYLOAD_REF, &reference(0, b"zero")),
                (tag::STATE_WITH_PAYLOAD, br#"{"n":2}"#),
                (tag::STATE_PAYLOAD_REF, &reference(1, b"zero")),
            ]);
            let mut reader =
                StateReader::<_, Metadata>::new(Cursor::new(file)).unwrap();
            if !keep {
                reader = reader.without_payloads();
            }
            let results: Vec<_> = reader.collect();
            assert_eq!(results.len(), 3);
            assert_eq!(results[1].as_ref().unwrap().payload_from, Some(0));
            let error = format!("{:#}", results[2].as_ref().unwrap_err());
            assert!(
                error.contains(
                    "corrupt payload reference in trace record 5 (to state \
                     1, which has no payload in a record of its own)"
                ),
                "{error}"
            );
        }

        let error =
            states(&trace(&[(tag::STATE_PAYLOAD_REF, &reference(0, b""))]))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("payload reference without a state expecting one"),
            "{error}"
        );
    }

    /// Becomes `changed` once seeking, as the reader does only to read a
    /// payload again: as if the file changed meanwhile.
    struct ChangedBeforeReadingAgain {
        bytes: Cursor<Vec<u8>>,
        changed: Option<Vec<u8>>,
    }

    impl Read for ChangedBeforeReadingAgain {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.bytes.read(buffer)
        }
    }

    impl Seek for ChangedBeforeReadingAgain {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            if let Some(changed) = self.changed.take() {
                *self.bytes.get_mut() = changed;
            }
            self.bytes.seek(position)
        }
    }

    #[test]
    fn checks_payloads_read_again() {
        let file = shared_payloads();
        let offset = states(&file).unwrap()[0].payload_offset.unwrap() as usize;
        let header = offset - RECORD_HEADER_LEN;
        let flipped = |position: usize| {
            let mut changed = file.clone();
            changed[position] ^= 1;
            changed
        };
        // State 0's record with another tag, and a valid header checksum.
        let retagged = {
            let mut changed = file.clone();
            let tag = (tag::OPTIONAL | 7).to_le_bytes();
            changed[header + crate::RECORD_TAG_OFFSET..][..2]
                .copy_from_slice(&tag);
            let crc = crc32fast::hash(
                &changed[header..header + crate::RECORD_HEADER_CRC_OFFSET],
            );
            changed[header + crate::RECORD_HEADER_CRC_OFFSET..][..4]
                .copy_from_slice(&crc.to_le_bytes());
            changed
        };
        // In state 0's payload, caught by its checksum; in its record's
        // payload CRC field, caught as it differs from the reference's;
        // and in its record's tag.
        for (changed, reason) in [
            (flipped(offset + 1), "corrupt record (checksum mismatch)"),
            (
                flipped(offset - 1),
                "not the state payload read there before",
            ),
            (retagged, "not the state payload read there before"),
        ] {
            let reader = ChangedBeforeReadingAgain {
                bytes: Cursor::new(file.clone()),
                changed: Some(changed),
            };
            let results: Vec<_> =
                StateReader::<_, Metadata>::new(reader).unwrap().collect();
            assert_eq!(results.len(), 2, "{reason}");
            assert_eq!(
                results[0].as_ref().unwrap().payload.as_deref(),
                Some(&b"zero"[..])
            );
            let error = format!("{:#}", results[1].as_ref().unwrap_err());
            assert!(
                error.contains(
                    "failed to read the payload of state 0 that trace record \
                     3 refers to"
                ) && error.contains(reason),
                "{error}"
            );
        }
    }

    /// Reads, but can't seek, as a pipe.
    struct Unseekable(Cursor<Vec<u8>>);

    impl Read for Unseekable {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buffer)
        }
    }

    impl Seek for Unseekable {
        fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
            Err(std::io::Error::other("illegal seek"))
        }
    }

    /// A shared payload can't be read from a pipe, which says so, but
    /// its location can.
    #[test]
    fn says_shared_payloads_need_a_seekable_trace() {
        let reader = || Unseekable(Cursor::new(shared_payloads()));
        let results: Vec<_> =
            StateReader::<_, Metadata>::new(reader()).unwrap().collect();
        assert_eq!(results.len(), 2);
        let error = format!("{:#}", results[1].as_ref().unwrap_err());
        assert!(error.contains("needs a seekable trace file"), "{error}");

        let states = StateReader::<_, Metadata>::new(reader())
            .unwrap()
            .without_payloads()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(states.len(), SHARED_PAYLOADS.len());
    }

    #[test]
    fn reads_shared_payloads_at_their_offsets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&path, shared_payloads()).unwrap();
        let states = open_trace::<Metadata>(&path)
            .unwrap()
            .without_payloads()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let file = File::open(&path).unwrap();
        for (state, expected) in states.iter().zip(SHARED_PAYLOADS) {
            let Some(offset) = state.payload_offset else {
                assert_eq!(expected, None);
                continue;
            };
            assert_eq!(
                read_payload_at(&path, offset).unwrap(),
                expected.unwrap()
            );
            assert_eq!(
                read_payload_from(&file, offset).unwrap(),
                expected.unwrap()
            );
        }
    }

    #[test]
    fn checks_the_driver() {
        let file = trace(&[]);
        let error =
            StateReader::<_, Metadata>::new(Cursor::new(file.as_slice()))
                .unwrap()
                .expect_driver(TraceDriver::Browser)
                .err()
                .unwrap();
        assert_eq!(
            error.to_string(),
            "not a browser trace (written by the terminal driver)"
        );
    }

    /// The metadata of a state with the snapshot `entries`.
    fn with_snapshots(n: u32, entries: &[&str]) -> String {
        format!(r#"{{"n":{n},"snapshots":[{}]}}"#, entries.join(","))
    }

    const LIST: &str = r#"{"index":0,"value":[1,2,3,4,5,6,7,8],"time":1}"#;
    const LIST_FROM_0: &str = r#"{"index":0,"value_from":0,"time":1}"#;
    const LIST_FROM_2: &str = r#"{"index":0,"value_from":2,"time":1}"#;
    const OTHER: &str = r#"{"index":0,"value":"another value","time":1}"#;

    /// The metadata of each state of `shared_snapshots`, as written by a
    /// writer that doesn't write values once.
    fn snapshots_read() -> Vec<String> {
        [&[LIST][..], &[LIST], &[OTHER], &[OTHER, LIST], &[OTHER]]
            .iter()
            .enumerate()
            .map(|(n, entries)| with_snapshots(n as u32, entries))
            .collect()
    }

    /// The states of `snapshots_read`, with repeated values written once,
    /// and payloads too.
    fn shared_snapshots() -> Vec<u8> {
        trace(&[
            (
                tag::STATE_WITH_PAYLOAD,
                with_snapshots(0, &[LIST]).as_bytes(),
            ),
            (tag::STATE_PAYLOAD, b"zero"),
            (
                tag::STATE_WITH_PAYLOAD,
                with_snapshots(1, &[LIST_FROM_0]).as_bytes(),
            ),
            (tag::STATE_PAYLOAD_REF, &reference(0, b"zero")),
            (tag::STATE, with_snapshots(2, &[OTHER]).as_bytes()),
            // Back to the list, written in full: only the last value in
            // full is referred to.
            (
                tag::STATE,
                with_snapshots(3, &[LIST_FROM_2, LIST]).as_bytes(),
            ),
            (tag::STATE, with_snapshots(4, &[OTHER]).as_bytes()),
            (tag::END, b""),
        ])
    }

    fn metadata(states: &[State<Box<RawValue>>]) -> Vec<&str> {
        states.iter().map(|state| state.metadata.get()).collect()
    }

    #[test]
    fn puts_back_snapshot_values_written_once() {
        let file = shared_snapshots();
        for keep in [true, false] {
            let mut reader =
                StateReader::<_, Box<RawValue>>::new(Cursor::new(&file))
                    .unwrap();
            if !keep {
                reader = reader.without_payloads();
            }
            let states = reader.collect::<Result<Vec<_>>>().unwrap();
            assert_eq!(metadata(&states), snapshots_read());
            assert_eq!(states[1].payload_from, Some(0));
        }
        // Into a type, after putting them back.
        #[derive(Deserialize)]
        struct Snapshots {
            snapshots: Vec<serde_json::Value>,
        }
        let states = StateReader::<_, Snapshots>::new(Cursor::new(&file))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(states[3].metadata.snapshots[0]["value"], "another value");
    }

    /// A trace cut anywhere reads without an error, every state as it was
    /// written, whether payloads are skipped or not.
    #[test]
    fn reads_a_trace_with_snapshot_references_cut_anywhere() {
        let file = shared_snapshots();
        let expected = snapshots_read();
        let mut previous = 0;
        for length in ENVELOPE_LEN..=file.len() {
            for keep in [true, false] {
                let mut reader = StateReader::<_, Box<RawValue>>::new(
                    Cursor::new(&file[..length]),
                )
                .unwrap();
                if !keep {
                    reader = reader.without_payloads();
                }
                let states = reader.collect::<Result<Vec<_>>>().unwrap_or_else(
                    |error| panic!("cut at {length}: {error:#}"),
                );
                assert!(states.len() >= previous, "cut at {length}");
                previous = states.len();
                assert_eq!(
                    metadata(&states),
                    expected[..states.len()],
                    "cut at {length}"
                );
            }
        }
        assert_eq!(previous, expected.len());
    }

    #[test]
    fn rejects_snapshot_references_that_dont_resolve() {
        // State 2 refers to `reference`, after state 0 with the list in
        // full and state 1 referring to it.
        for (reference, reason) in [
            (
                r#"{"index":0,"value_from":1}"#,
                "last value in full is state 0's",
            ),
            (r#"{"index":0,"value_from":2}"#, "isn't before state 2"),
            (r#"{"index":1,"value_from":0}"#, "no earlier value in full"),
            (r#"{"index":0,"value_from":null}"#, "not to a state index"),
        ] {
            let file = trace(&[
                (tag::STATE, with_snapshots(0, &[LIST]).as_bytes()),
                (
                    tag::STATE_WITH_PAYLOAD,
                    with_snapshots(1, &[LIST_FROM_0]).as_bytes(),
                ),
                (tag::STATE_PAYLOAD, b"one"),
                (tag::STATE, with_snapshots(2, &[reference]).as_bytes()),
                (tag::STATE, with_snapshots(3, &[LIST]).as_bytes()),
                (tag::END, b""),
            ]);
            let results: Vec<_> =
                StateReader::<_, Box<RawValue>>::new(Cursor::new(file))
                    .unwrap()
                    .collect();
            // The states before it, then the error.
            assert_eq!(results.len(), 3, "{reason}");
            assert_eq!(
                results[1].as_ref().unwrap().metadata.get(),
                snapshots_read()[1]
            );
            let error = format!("{:#}", results[2].as_ref().unwrap_err());
            assert!(
                error.contains(
                    "corrupt snapshot value reference in trace record 3"
                ) && error.contains(reason),
                "{error}"
            );
        }
    }
}
