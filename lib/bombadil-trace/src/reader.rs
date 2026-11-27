use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, Cursor, Read},
    path::Path,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use crate::{
    ENVELOPE_LEN, PayloadLocation, RECORD_HEADER_LEN, StepError, TraceDriver,
    envelope::read_envelope,
    record::{RawRecord, RecordHeader, RecordReader},
    tag,
};

/// One state read back from a trace.
#[derive(Debug)]
pub struct State<M> {
    /// Position of the state in the trace, from 0.
    pub index: usize,
    pub metadata: M,
    /// The state's driver-defined bytes, if it has any and they weren't
    /// skipped (see [`TraceReader::without_payloads`]).
    pub payload: Option<Vec<u8>>,
    pub payload_location: Option<PayloadLocation>,
    /// Size of the payload in bytes, even if it was skipped.
    pub payload_length: Option<usize>,
    action: Option<ActionJson>,
}

/// One step of a run.
#[derive(Debug)]
pub enum Record<M, A> {
    /// A state and the action that led to it (none for the first).
    State { action: Option<A>, state: State<M> },
    /// The actions the trace has no state after (one, unless the run was
    /// killed), and why the last has none. Only ever the last record.
    Error {
        /// The index the first missing state would have.
        index: usize,
        actions: Vec<A>,
        error: StepError,
    },
}

/// Reads a trace as its [`Record`]s: each state's metadata as `M`, each
/// action as `A` from the JSON object `{"action": …, "action_taken": …}`.
/// A bad record ends them with an error; a cut tail only logs a warning.
pub struct TraceReader<R, M, A> {
    pub driver: TraceDriver,
    states: StateReader<R, M>,
    failed: bool,
    action: std::marker::PhantomData<A>,
}

impl<R: Read, M: DeserializeOwned, A: DeserializeOwned> TraceReader<R, M, A> {
    /// Reads the envelope from `reader`; records are read on iteration.
    pub fn new(reader: R) -> Result<Self> {
        let states = StateReader::new(reader)?;
        Ok(TraceReader {
            driver: states.driver,
            states,
            failed: false,
            action: std::marker::PhantomData,
        })
    }

    /// Whether the trace ended with its [`tag::END`] record.
    pub fn finished(&self) -> bool {
        self.states.finished()
    }

    /// Fails unless the trace was written by `driver`.
    pub fn expect_driver(mut self, driver: TraceDriver) -> Result<Self> {
        self.states = self.states.expect_driver(driver)?;
        Ok(self)
    }

    /// Checks payloads but doesn't keep them.
    pub fn without_payloads(mut self) -> Self {
        self.states = self.states.without_payloads();
        self
    }

    fn read_record(&mut self) -> Result<Option<Record<M, A>>> {
        let parse = |action: &[u8], index: usize| {
            serde_json::from_slice(action).with_context(|| {
                format!("invalid action of trace record {index}")
            })
        };
        if let Some(state) = self.states.next() {
            let mut state = state?;
            let action = state
                .action
                .take()
                .map(|action| parse(action.json(), state.index))
                .transpose()?;
            return Ok(Some(Record::State { action, state }));
        }
        let Some((index, error)) = self.states.error()? else {
            return Ok(None);
        };
        let actions = self
            .states
            .actions
            .drain(..)
            .map(|(_, record)| parse(&record[STATE_INDEX_LEN..], index))
            .collect::<Result<_>>()?;
        Ok(Some(Record::Error {
            index,
            actions,
            error,
        }))
    }
}

impl<R: Read, M: DeserializeOwned, A: DeserializeOwned> Iterator
    for TraceReader<R, M, A>
{
    type Item = Result<Record<M, A>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let result = self.read_record().transpose();
        self.failed = matches!(result, Some(Err(_)));
        result
    }
}

/// An action as recorded, at the end of `bytes`.
#[derive(Debug)]
struct ActionJson {
    bytes: Vec<u8>,
    start: usize,
}

impl ActionJson {
    fn json(&self) -> &[u8] {
        &self.bytes[self.start..]
    }

    /// The action in a state's metadata, where traces without action
    /// records hold it: `action`, with the `action_taken` of `state`.
    fn of_state(metadata: &[u8]) -> Option<Self> {
        #[derive(Deserialize)]
        struct Metadata<'a> {
            #[serde(borrow, default)]
            action: Option<&'a RawValue>,
            #[serde(borrow, default)]
            state: Option<&'a RawValue>,
        }
        #[derive(Deserialize)]
        struct State<'a> {
            #[serde(borrow, default)]
            action_taken: Option<&'a RawValue>,
        }
        if metadata.first() != Some(&b'{') {
            return None;
        }
        let metadata: Metadata = serde_json::from_slice(metadata).ok()?;
        let action = metadata.action?;
        let action_taken = metadata
            .state
            .filter(|state| state.get().starts_with('{'))
            .and_then(|state| serde_json::from_str::<State>(state.get()).ok())
            .and_then(|state| state.action_taken)
            .map_or("null", RawValue::get);
        Some(ActionJson {
            bytes: format!(
                r#"{{"action":{},"action_taken":{action_taken}}}"#,
                action.get()
            )
            .into_bytes(),
            start: 0,
        })
    }
}

// The index of a state, which starts an action or error record.
const STATE_INDEX_LEN: usize = size_of::<u64>();

/// Reads the states of a trace, in the order they were written, with
/// their metadata deserialized as `M`.
///
/// A bad record, or a state missing its payload, ends the states with an
/// error, after every complete state before it. An incomplete last
/// record, or a last state whose payload the trace ends before (from a
/// run killed while writing), ends them with a logged warning instead,
/// and without that state. A trace without a [`tag::END`] record also
/// ends with a logged warning; see [`finished`](Self::finished).
pub(crate) struct StateReader<R, M> {
    pub driver: TraceDriver,
    records: RecordReader<R>,
    states_read: usize,
    // A state whose payload hasn't been read yet.
    pending: Option<PendingState<M>>,
    // The actions whose state hasn't been read, by the state each was applied in.
    actions: VecDeque<(u64, Vec<u8>)>,
    error: Option<Vec<u8>>,
    failed: bool,
    // Whether iteration ended, with or without an error.
    ended: bool,
    // Whether the end record was read.
    finished: bool,
    keep_payloads: bool,
}

struct PendingState<M> {
    metadata: M,
    action: Option<ActionJson>,
    // The index of the record holding the metadata.
    record: usize,
}

/// Opens a trace file. Directories are not trace inputs.
pub fn open_trace<M: DeserializeOwned, A: DeserializeOwned>(
    path: &Path,
) -> Result<TraceReader<BufReader<File>, M, A>> {
    let file = File::open(path).with_context(|| {
        format!("failed to open trace file {}", path.display())
    })?;
    TraceReader::new(BufReader::new(file))
        .with_context(|| format!("failed to read {}", path.display()))
}

#[cfg(test)]
pub(crate) fn open_states<M: DeserializeOwned>(
    path: &Path,
) -> Result<StateReader<BufReader<File>, M>> {
    let file = File::open(path).with_context(|| {
        format!("failed to open trace file {}", path.display())
    })?;
    StateReader::new(BufReader::new(file))
        .with_context(|| format!("failed to read {}", path.display()))
}

/// Reads the payload at `offset` (a [`State::payload_location`]) of the
/// trace file at `path`, as with
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
pub fn read_payload_at(
    path: &Path,
    location: PayloadLocation,
) -> Result<Vec<u8>> {
    File::open(path)
        .map_err(anyhow::Error::from)
        .and_then(|file| read_payload(&file, location))
        .with_context(|| {
            format!(
                "failed to read the state payload at offset {} of {}",
                location.offset,
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
/// handle (e.g. a [`TraceReader`] of a [`File::try_clone`] of it), even
/// concurrently. On Windows each read moves the cursor and it is put
/// back after, so a single call leaves it where it was, but concurrent
/// calls can interleave their saving and restoring and leave it
/// anywhere: don't mix concurrent calls with cursor-based reads of the
/// same handle. On other platforms reads seek first, so don't call this
/// concurrently on one handle at all.
pub fn read_payload_from(
    file: &File,
    location: PayloadLocation,
) -> Result<Vec<u8>> {
    read_payload(file, location).with_context(|| {
        format!(
            "failed to read the state payload at offset {}",
            location.offset
        )
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

fn read_payload(file: &File, location: PayloadLocation) -> Result<Vec<u8>> {
    read_envelope(&mut PositionalReader { file, position: 0 })?;
    let offset = location.offset;
    if let Some(chunk) = location.chunk {
        if chunk < ENVELOPE_LEN as u64 || chunk > file.metadata()?.len() {
            bail!("not a chunk offset");
        }
        let mut reader = PositionalReader {
            file,
            position: chunk,
        };
        let (header, read) = RecordHeader::read(&mut reader)?;
        if read != RECORD_HEADER_LEN {
            bail!("truncated chunk header");
        }
        header.check()?;
        if header.tag != tag::CHUNK {
            bail!("no compressed chunk there");
        }
        let payload = header
            .read_payload(&mut reader, true)?
            .flatten()
            .context("truncated compressed chunk")?;
        let decoded = crate::chunk::decode(&payload)?;
        let header_offset = offset
            .checked_sub(RECORD_HEADER_LEN as u64)
            .filter(|&offset| offset < decoded.len() as u64)
            .context("not a chunk payload offset")?;
        let mut reader = Cursor::new(decoded);
        reader.set_position(header_offset);
        return read_payload_record(&mut reader, offset);
    }
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
    read_payload_record(&mut file, offset)
}

fn read_payload_record(reader: &mut impl Read, offset: u64) -> Result<Vec<u8>> {
    let (header, read) = RecordHeader::read(reader)?;
    if read < RECORD_HEADER_LEN {
        bail!("no state payload there");
    }
    // The offset was given by the caller, so a mismatch more likely
    // means it isn't a payload offset than that the file is damaged.
    if !header.checksum_matches() {
        bail!(
            "no state payload at offset {offset} (no record header \
             before it)"
        );
    }
    header.check()?;
    if header.tag != tag::STATE_PAYLOAD {
        bail!("no state payload there (record tag {})", header.tag);
    }
    match header.read_payload(reader, true)? {
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
            actions: VecDeque::new(),
            error: None,
            failed: false,
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

    /// Once the states are read: the index and error of [`Record::Error`].
    fn error(&mut self) -> Result<Option<(usize, StepError)>> {
        if !self.ended || self.failed {
            return Ok(None);
        }
        let Some((last, _)) = self.actions.back() else {
            return Ok(None);
        };
        let error = match &self.error {
            Some(error) => {
                let (state, error) = error.split_at(STATE_INDEX_LEN);
                if state != last.to_le_bytes() {
                    self.failed = true;
                    bail!("trace error is for another action than its last");
                }
                serde_json::from_slice(error)
                    .inspect_err(|_| self.failed = true)
                    .context("invalid trace error")?
            }
            None => StepError::Unknown,
        };
        Ok(Some((self.states_read, error)))
    }

    /// The action that led to the state: its record's, or its metadata's.
    fn action(&mut self, index: usize, metadata: &[u8]) -> Option<ActionJson> {
        while let Some((state, _)) = self.actions.front() {
            if *state as usize + 1 > index {
                break;
            }
            let (state, bytes) = self.actions.pop_front().unwrap();
            if state as usize + 1 == index {
                return Some(ActionJson {
                    bytes,
                    start: STATE_INDEX_LEN,
                });
            }
        }
        ActionJson::of_state(metadata)
    }

    fn state(
        &mut self,
        metadata: M,
        action: Option<ActionJson>,
        payload: Option<RawRecord>,
    ) -> State<M> {
        let index = self.states_read;
        self.states_read += 1;
        let (payload_location, payload_length, payload) = match payload {
            Some(record) => (
                Some(record.location),
                Some(record.payload_length),
                record.payload,
            ),
            None => (None, None, None),
        };
        State {
            index,
            metadata,
            payload,
            payload_location,
            payload_length,
            action,
        }
    }

    fn read_state(&mut self) -> Result<Option<State<M>>> {
        loop {
            let Some(record) = self.records.next_record(self.keep_payloads)
            else {
                if let Some(pending) = self.pending.take() {
                    // Its action is one no state follows, after all.
                    if let Some(action) = pending.action
                        && action.start > 0
                    {
                        let state = self.states_read as u64 - 1;
                        self.actions.push_front((state, action.bytes));
                    }
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
                    let encoded = record.payload.as_deref().unwrap_or_default();
                    let metadata = serde_json::from_slice(encoded)
                        .with_context(|| {
                            format!(
                                "invalid state in trace record {}",
                                record.index
                            )
                        })?;
                    let action = self.action(self.states_read, encoded);
                    if record.tag == tag::STATE {
                        return Ok(Some(self.state(metadata, action, None)));
                    }
                    self.pending = Some(PendingState {
                        metadata,
                        action,
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
                    return Ok(Some(self.state(
                        pending.metadata,
                        pending.action,
                        Some(record),
                    )));
                }
                tag::ACTION | tag::ERROR => {
                    let payload = record.payload.unwrap_or_default();
                    let Some(state) = payload.first_chunk::<STATE_INDEX_LEN>()
                    else {
                        bail!(
                            "invalid action or error in trace record {}",
                            record.index
                        );
                    };
                    if record.tag == tag::ACTION {
                        self.actions
                            .push_back((u64::from_le_bytes(*state), payload));
                    } else {
                        self.error = Some(payload);
                    }
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
        self.failed = matches!(result, Some(Err(_)));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        TRACE_FILE_NAME,
        test_log::{capture_warnings, warned},
        write_envelope, write_record,
    };
    use serde::Deserialize;
    use zerocopy::{FromBytes, IntoBytes};

    fn read_payload_at(path: &Path, offset: u64) -> Result<Vec<u8>> {
        super::read_payload_at(
            path,
            PayloadLocation {
                offset,
                chunk: None,
            },
        )
    }

    fn read_payload_from(file: &File, offset: u64) -> Result<Vec<u8>> {
        super::read_payload_from(
            file,
            PayloadLocation {
                offset,
                chunk: None,
            },
        )
    }

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

    // Records start at 16, 39, 59, 82 and 105, and the file ends at 124.
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
    fn reads_payloads_at_their_offsets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&path, three_states()).unwrap();
        let states = open_states::<Metadata>(&path)
            .unwrap()
            .without_payloads()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(states[2].payload, None);
        assert_eq!(states[2].payload_length, Some(3));
        let offset = states[2].payload_location.unwrap().offset;
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

        assert!(read_payload_at(directory.path(), offset).is_err());
        assert!(open_states::<Metadata>(directory.path()).is_err());

        // A header with a matching checksum is still checked.
        let mut file = three_states();
        let header = offset as usize - RECORD_HEADER_LEN;
        let mut record =
            RecordHeader::read_from_bytes(&file[header..offset as usize])
                .unwrap();
        record.set_reserved(1);
        file[header..offset as usize].copy_from_slice(record.as_bytes());
        std::fs::write(&path, &file).unwrap();
        let error =
            format!("{:#}", read_payload_at(&path, offset).unwrap_err());
        assert!(
            error.contains("unsupported record header (reserved field set)"),
            "{error}"
        );

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
        let offset = open_states::<Metadata>(&path)
            .unwrap()
            .nth(2)
            .unwrap()
            .unwrap()
            .payload_location
            .unwrap()
            .offset;
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

    /// An action record too short for the index of its state is an
    /// error, as other optional records are skipped, not this one.
    #[test]
    fn rejects_an_action_without_its_state() {
        let mut file = three_states();
        write_record(&mut file, tag::ACTION, &[&[0; 7]]).unwrap();
        let states: Vec<_> = StateReader::<_, Metadata>::new(&file[..])
            .unwrap()
            .collect();
        assert_eq!(states.len(), 4);
        let error = format!("{:#}", states[3].as_ref().unwrap_err());
        assert_eq!(error, "invalid action or error in trace record 5");
    }

    fn indexed(state: u64, json: &[u8]) -> Vec<u8> {
        [&state.to_le_bytes()[..], json].concat()
    }

    /// An error is for the last action; for another, the trace is bad.
    #[test]
    fn rejects_an_error_for_another_action() {
        let action = indexed(1, br#""a""#);
        let file = |error_state| {
            let error = indexed(error_state, br#"{"kind":"stalled"}"#);
            trace(&[
                (tag::STATE, br#"{"n":0}"#),
                (tag::STATE, br#"{"n":1}"#),
                (tag::ACTION, &action),
                (tag::ERROR, &error),
                (tag::END, b""),
            ])
        };
        let read = |file: Vec<u8>| {
            TraceReader::<_, Metadata, String>::new(&file[..])
                .unwrap()
                .collect::<Vec<_>>()
        };
        let records = read(file(1));
        assert!(matches!(
            &records[..],
            [Ok(_), Ok(_), Ok(Record::Error { index: 2, actions, error })]
                if actions[..] == ["a"] && *error == StepError::Stalled
        ));
        let records = read(file(0));
        assert_eq!(records.len(), 3);
        let error = records[2].as_ref().unwrap_err().to_string();
        assert_eq!(error, "trace error is for another action than its last");
    }

    /// A state whose payload the trace ends before isn't read, so the
    /// action that led to it has no state.
    #[test]
    fn keeps_the_action_of_a_state_cut_before_its_payload() {
        let action = indexed(0, br#""a""#);
        let file = trace(&[
            (tag::STATE, br#"{"n":0}"#),
            (tag::ACTION, &action),
            (tag::STATE_WITH_PAYLOAD, br#"{"n":1}"#),
        ]);
        let records = TraceReader::<_, Metadata, String>::new(&file[..])
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(matches!(
            &records[..],
            [
                Record::State { action: None, .. },
                Record::Error { index: 1, actions, error: StepError::Unknown },
            ] if actions[..] == ["a"]
        ));
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
        for length in [105, 108, 123] {
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
        // State 2's payload corrupted: states 0 and 1, then the error.
        // State 2 isn't complete without its payload.
        let mut corrupt = three_states();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
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
