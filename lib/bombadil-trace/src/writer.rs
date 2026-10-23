use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, TryRecvError},
    },
    thread::JoinHandle,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use crate::{
    ENVELOPE_LEN, PAYLOAD_REF_LEN, RECORD_HEADER_LEN, TRACE_FILE_NAME,
    TraceDriver, WRITER_FAILED_EARLIER,
    envelope::driver_code,
    reader::PositionalReader,
    record::{PayloadRef, check_payload_length, read_full},
    tag, write_envelope, write_record,
};

// Bounds the number of in-flight states to cap memory while letting the
// writer thread run behind the test loop.
const PENDING_STATES_MAX: usize = 32;

/// Writes the states of a trace on a dedicated thread, so that encoding
/// and disk I/O overlap with the test loop instead of stalling it. The
/// bounded channel provides backpressure if the writer can't keep up.
///
/// Whenever it has caught up, the thread flushes what it has written, so
/// if the process is killed while it is idle the file holds every state.
/// While it is behind, the queued states and those it has buffered (up
/// to 8 KiB) are lost. Call [`finish`](Self::finish) to write the rest
/// and the [`tag::END`] record, and learn whether everything was written.
/// Dropping the writer without finishing it writes the queued states but
/// no END, so the trace reads as unfinished, and can only log a failure
/// (unless a call already returned it).
///
/// A payload identical to an earlier state's is written as a
/// [`tag::STATE_PAYLOAD_REF`] record naming that state. The thread
/// compares the previous payload in memory, and older ones with the bytes
/// in the file, which it flushes first; it keeps only the CRC-32, length
/// and offset of each distinct payload, and of at most 4 distinct
/// payloads with the same CRC-32 and length. Payloads of 12 bytes or
/// fewer, no larger than a reference, are always written in full.
///
/// Once the thread has failed, every call fails: the first one with the
/// error that stopped it, later ones with a copy of its message.
pub struct TraceFileWriter<M> {
    // `None` once the worker stopped or the writer was finished.
    sender: Option<mpsc::SyncSender<Message<M>>>,
    worker: Option<JoinHandle<Result<()>>>,
    // The error that stopped the worker, once returned, so that every
    // later call reports it too.
    failure: Option<String>,
}

enum Message<M> {
    State(Box<PendingState<M>>),
    // Writes the END record after everything before it, and stops. If
    // the channel closes without it, the worker flushes and stops
    // without END, leaving the trace unfinished.
    Finish,
}

struct PendingState<M> {
    metadata: M,
    // Shared rather than copied, so it reaches the file without another
    // copy of its bytes.
    payload: Option<Arc<Vec<u8>>>,
}

impl<M: Serialize + Send + 'static> TraceFileWriter<M> {
    /// Creates `trace.bin` in `run_path`, and `run_path` if needed,
    /// written by `driver`. Replaces an existing trace only if
    /// `overwrite` is set.
    ///
    /// If this fails it leaves no new file behind, and an existing trace
    /// as it was, though it may have created `run_path`.
    pub fn create(
        run_path: &Path,
        driver: TraceDriver,
        overwrite: bool,
    ) -> Result<Self> {
        Self::create_with(run_path, driver, overwrite, |file| file)
    }

    /// [`create`](Self::create), writing through `wrap(file)`, so tests
    /// can make writing fail.
    fn create_with<W: Write + Send + 'static>(
        run_path: &Path,
        driver: TraceDriver,
        overwrite: bool,
        wrap: impl FnOnce(File) -> W,
    ) -> Result<Self> {
        driver_code(driver)?;
        std::fs::create_dir_all(run_path).with_context(|| {
            format!("failed to create {}", run_path.display())
        })?;
        let trace_path = run_path.join(TRACE_FILE_NAME);
        // The file this call created, and removes if it fails.
        let (path, file) = if overwrite {
            create_temp(run_path)?
        } else {
            (trace_path.clone(), create_new_trace(&trace_path)?)
        };
        // To read payloads back, to compare later ones with them.
        let read_back = file.try_clone();
        let mut trace_file = BufWriter::new(wrap(file));
        let created = read_back
            .with_context(|| {
                format!(
                    "failed to open {} again, to compare payloads with it",
                    path.display()
                )
            })
            .and_then(|read_back| {
                write_envelope(&mut trace_file, driver)
                    .and_then(|()| trace_file.flush())
                    .map_err(trace_write_error)?;
                Self::start(trace_file, read_back)
            })
            .and_then(|writer| {
                // The last step that can fail, so a failure leaves an
                // existing trace as it was. Writing continues on the same
                // file. On failure, dropping the writer stops it without
                // writing END.
                if overwrite {
                    std::fs::rename(&path, &trace_path).with_context(|| {
                        format!("failed to create {}", trace_path.display())
                    })?;
                }
                Ok(writer)
            });
        match created {
            Ok(writer) => {
                log::info!("storing run trace in {}", run_path.display());
                Ok(writer)
            }
            Err(error) => {
                if let Err(error) = std::fs::remove_file(&path) {
                    log::warn!("failed to remove {}: {error}", path.display());
                }
                Err(error)
            }
        }
    }

    /// Starts the worker, writing to `trace_file` after its envelope, and
    /// reading payloads back through `read_back`, a handle of the file.
    fn start<W: Write + Send + 'static>(
        trace_file: BufWriter<W>,
        read_back: File,
    ) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(PENDING_STATES_MAX);
        let worker = std::thread::Builder::new()
            .name("bombadil-trace-writer".to_string())
            .spawn(move || worker_loop(receiver, trace_file, read_back))?;
        Ok(TraceFileWriter {
            sender: Some(sender),
            worker: Some(worker),
            failure: None,
        })
    }

    /// Queues a state, with its driver-defined payload if it has one.
    ///
    /// A payload too large for a trace is refused here, and the writer
    /// stays usable. The metadata is only encoded, and its size checked,
    /// on the writer thread, so a failure to encode or write a state is
    /// returned by a later call, or by [`finish`](Self::finish).
    pub fn write_state(
        &mut self,
        metadata: M,
        payload: Option<Arc<Vec<u8>>>,
    ) -> Result<()> {
        if let Some(payload) = &payload {
            check_payload_length(payload.len())
                .map_err(|error| anyhow!(error).context("invalid state"))?;
        }
        let state = Box::new(PendingState { metadata, payload });
        self.send(Message::State(state))
    }
}

impl<M> TraceFileWriter<M> {
    fn send(&mut self, message: Message<M>) -> Result<()> {
        if let Some(error) = self.earlier_failure() {
            return Err(error);
        }
        let Some(sender) = &self.sender else {
            bail!("trace writer already finished");
        };
        if sender.send(message).is_err() {
            return Err(self.worker_failure());
        }
        Ok(())
    }

    /// Writes every queued state and the [`tag::END`] record that marks
    /// the trace as finished, closes the file, and returns the error
    /// that stopped the writer, if any. Later calls return the same.
    pub fn finish(&mut self) -> Result<()> {
        if let Some(sender) = &self.sender {
            // If the worker stopped, joining it reports why.
            let _ = sender.send(Message::Finish);
        }
        self.stop()
    }

    /// Writes every queued state, without the [`tag::END`] record unless
    /// [`finish`](Self::finish) sent it, closes the file, and returns the
    /// error that stopped the writer, if any.
    fn stop(&mut self) -> Result<()> {
        self.sender = None;
        if let Some(worker) = self.worker.take()
            && let Err(error) = join(worker)
        {
            return Err(self.failed(error));
        }
        match self.earlier_failure() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Joins the worker thread to surface the error that made it exit.
    fn worker_failure(&mut self) -> anyhow::Error {
        self.sender = None;
        let worker = self
            .worker
            .take()
            .expect("the worker is set whenever the sender is");
        let error = match join(worker) {
            Ok(()) => anyhow!("trace writer thread exited unexpectedly"),
            Err(error) => error,
        };
        self.failed(error)
    }

    /// Keeps the message of `error`, which stopped the worker, for later
    /// calls.
    fn failed(&mut self, error: anyhow::Error) -> anyhow::Error {
        self.failure = Some(format!("{error:#}"));
        error
    }

    fn earlier_failure(&self) -> Option<anyhow::Error> {
        self.failure
            .as_ref()
            .map(|failure| anyhow!("{WRITER_FAILED_EARLIER}: {failure}"))
    }
}

/// Creates `trace_path`, failing if there is a file there.
fn create_new_trace(trace_path: &Path) -> Result<File> {
    match File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(trace_path)
    {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => bail!(
            "{TRACE_FILE_NAME} already exists at {}. \
             Use --output-path-overwrite to overwrite, or choose a different --output-path.",
            trace_path.display(),
        ),
        Err(error) => Err(error).with_context(|| {
            format!("failed to create {}", trace_path.display())
        }),
    }
}

// Numbers the temporary files of this process, so that writers in it
// don't take each other's.
static TEMP_FILES_CREATED: AtomicU64 = AtomicU64::new(0);

// How many taken names `create_temp` skips before giving up.
const TEMP_ATTEMPTS_MAX: u32 = 64;

/// Where an overwriting trace is written in `run_path` until it is
/// renamed to `trace.bin`: the `n`th temporary file of this process.
fn temp_path(run_path: &Path, n: u64) -> PathBuf {
    let pid = std::process::id();
    run_path.join(format!("{TRACE_FILE_NAME}.{pid}.{n}.tmp"))
}

/// Creates a new temporary file in `run_path` and returns its path. A
/// name that is taken, e.g. by a killed run whose process had the same
/// id (as is common in containers), is skipped rather than reused, since
/// it may belong to a live writer.
fn create_temp(run_path: &Path) -> Result<(PathBuf, File)> {
    for _ in 0..TEMP_ATTEMPTS_MAX {
        let n = TEMP_FILES_CREATED.fetch_add(1, Ordering::Relaxed);
        let path = temp_path(run_path, n);
        match File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to create {}", path.display())
                });
            }
        }
    }
    bail!(
        "failed to create a temporary trace file in {}: the last \
         {TEMP_ATTEMPTS_MAX} names tried were taken",
        run_path.display(),
    )
}

fn join(worker: JoinHandle<Result<()>>) -> Result<()> {
    worker.join().unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
        match message {
            Some(message) => bail!("trace writer thread panicked: {message}"),
            None => bail!("trace writer thread panicked"),
        }
    })
}

impl<M> Drop for TraceFileWriter<M> {
    fn drop(&mut self) {
        // A failure already returned to a caller isn't logged again.
        let reported = self.failure.is_some();
        // Without `finish`, the trace is left unfinished.
        if let Err(error) = self.stop()
            && !reported
        {
            log::error!("trace is incomplete: {error:#}");
        }
    }
}

fn worker_loop<M: Serialize, W: Write>(
    receiver: mpsc::Receiver<Message<M>>,
    mut trace_file: BufWriter<W>,
    read_back: File,
) -> Result<()> {
    let mut metadata = Vec::new();
    let mut payloads = WrittenPayloads::new(read_back);
    let mut states_written: u64 = 0;
    // Where the next record starts in the file.
    let mut offset = ENVELOPE_LEN as u64;
    loop {
        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) => {
                // Caught up with the test loop: put what's written in the
                // file, so it survives the process being killed.
                trace_file.flush().map_err(trace_write_error)?;
                match receiver.recv() {
                    Ok(message) => message,
                    Err(_) => break,
                }
            }
            Err(TryRecvError::Disconnected) => break,
        };
        match message {
            Message::State(state) => {
                metadata.clear();
                serde_json::to_writer(&mut metadata, &state.metadata)
                    .map_err(anyhow::Error::from)
                    .and_then(|()| Ok(check_payload_length(metadata.len())?))
                    .map_err(|error| {
                        error.context("failed to encode trace state")
                    })?;
                let state_tag = match state.payload {
                    Some(_) => tag::STATE_WITH_PAYLOAD,
                    None => tag::STATE,
                };
                write_record(&mut trace_file, state_tag, &[&metadata])
                    .map_err(trace_write_error)?;
                offset += (RECORD_HEADER_LEN + metadata.len()) as u64;
                if let Some(payload) = &state.payload {
                    let crc = crc32fast::hash(payload);
                    match payloads.find(payload, crc, &mut trace_file)? {
                        Some(earlier) => {
                            let reference = PayloadRef {
                                state: earlier,
                                length: payload.len() as u32,
                                crc,
                            }
                            .encode();
                            write_record(
                                &mut trace_file,
                                tag::STATE_PAYLOAD_REF,
                                &[&reference],
                            )
                            .map_err(trace_write_error)?;
                            offset +=
                                (RECORD_HEADER_LEN + reference.len()) as u64;
                            payloads.referred(payload, earlier);
                        }
                        None => {
                            write_record(
                                &mut trace_file,
                                tag::STATE_PAYLOAD,
                                &[payload],
                            )
                            .map_err(trace_write_error)?;
                            let payload_offset =
                                offset + RECORD_HEADER_LEN as u64;
                            offset = payload_offset + payload.len() as u64;
                            payloads.written(
                                payload,
                                crc,
                                states_written,
                                payload_offset,
                            );
                        }
                    }
                }
                states_written += 1;
            }
            Message::Finish => {
                // Every state is written: mark the trace as finished.
                return write_record(&mut trace_file, tag::END, &[])
                    .and_then(|()| trace_file.flush())
                    .map_err(trace_write_error);
            }
        }
    }
    // Dropped without being finished: leave the trace unfinished.
    trace_file.flush().map_err(trace_write_error)
}

/// The payloads written so far, so that a payload identical to an
/// earlier state's can be written as a reference to that state instead
/// (see [`tag::STATE_PAYLOAD_REF`]).
///
/// Candidates are found by CRC-32 and length, and confirmed by comparing
/// bytes, so a reference is only written for an identical payload. The
/// previous state's payload is still in memory and compared there, as
/// repeating it is the common case (a screen that didn't change). Older
/// ones are compared with the bytes in the file, after flushing what is
/// buffered, so a reference is only written to a payload that the file
/// holds in full. Memory grows with the number of distinct payloads, by
/// a few dozen bytes each, not with their size.
///
/// Only the first [`CANDIDATES_PER_KEY_MAX`] distinct payloads with the
/// same CRC-32 and length are kept as candidates, so payloads made to
/// collide can't make each state compare with every earlier one: each
/// state reads back at most that many payloads.
struct WrittenPayloads {
    // The state whose record holds each payload, and the payload's
    // offset, by CRC-32 and length.
    by_crc: HashMap<(u32, usize), Vec<(u32, u64)>>,
    // The previous state's payload, and the state whose record holds it.
    previous: Option<(Arc<Vec<u8>>, u32)>,
    // A handle of the trace file, read by offset (see `PositionalReader`:
    // on Unix without moving its cursor, elsewhere moving and restoring
    // it).
    read_back: File,
    // Whether reading back failed, after which older payloads aren't
    // compared any more.
    read_back_failed: bool,
    // Holds what is read back, a chunk at a time.
    chunk: Vec<u8>,
}

// The size of the chunks payloads are read back in.
const READ_BACK_CHUNK_BYTES: usize = 64 * 1024;

/// How many distinct payloads with the same CRC-32 and length a writer
/// keeps to compare later payloads with, as the [`TraceFileWriter`] and
/// [crate] docs say.
const CANDIDATES_PER_KEY_MAX: usize = 4;

impl WrittenPayloads {
    fn new(read_back: File) -> Self {
        WrittenPayloads {
            by_crc: HashMap::new(),
            previous: None,
            read_back,
            read_back_failed: false,
            chunk: Vec::new(),
        }
    }

    /// The earlier state whose record holds a payload identical to
    /// `payload`, whose CRC-32 is `crc`, if any.
    fn find<W: Write>(
        &mut self,
        payload: &Arc<Vec<u8>>,
        crc: u32,
        trace_file: &mut BufWriter<W>,
    ) -> Result<Option<u32>> {
        if payload.len() <= PAYLOAD_REF_LEN {
            return Ok(None);
        }
        if let Some((previous, state)) = &self.previous
            && (Arc::ptr_eq(previous, payload) || previous == payload)
        {
            return Ok(Some(*state));
        }
        let Some(candidates) = self.by_crc.get(&(crc, payload.len())) else {
            return Ok(None);
        };
        if self.read_back_failed {
            return Ok(None);
        }
        // So the candidates are in the file, not partly in the buffer.
        trace_file.flush().map_err(trace_write_error)?;
        for &(state, offset) in candidates {
            match file_holds(&self.read_back, offset, payload, &mut self.chunk)
            {
                Ok(true) => return Ok(Some(state)),
                Ok(false) => {}
                // Only a missed saving: the payload is written in full.
                Err(error) => {
                    log::warn!(
                        "failed to read back the payload of trace state \
                         {state}: {error}; payloads identical to older \
                         ones are written in full from now on"
                    );
                    self.read_back_failed = true;
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    /// Notes that `payload`, with CRC-32 `crc`, was written for state
    /// `state`, at `offset`.
    fn written(
        &mut self,
        payload: &Arc<Vec<u8>>,
        crc: u32,
        state: u64,
        offset: u64,
    ) {
        // A state that can't be referred to isn't kept.
        let Ok(state) = u32::try_from(state) else {
            self.previous = None;
            return;
        };
        // Candidates are only looked at while reading back works.
        if payload.len() > PAYLOAD_REF_LEN && !self.read_back_failed {
            let candidates =
                self.by_crc.entry((crc, payload.len())).or_default();
            if candidates.len() < CANDIDATES_PER_KEY_MAX {
                candidates.push((state, offset));
            }
        }
        self.previous = Some((payload.clone(), state));
    }

    /// Notes that `payload` was written as a reference to `state`.
    fn referred(&mut self, payload: &Arc<Vec<u8>>, state: u32) {
        self.previous = Some((payload.clone(), state));
    }
}

/// Whether `file` holds `payload` at `offset`, read a chunk at a time
/// into `chunk_buffer`.
fn file_holds(
    file: &File,
    offset: u64,
    payload: &[u8],
    chunk_buffer: &mut Vec<u8>,
) -> io::Result<bool> {
    chunk_buffer.resize(payload.len().min(READ_BACK_CHUNK_BYTES), 0);
    let mut reader = PositionalReader {
        file,
        position: offset,
    };
    for expected in payload.chunks(READ_BACK_CHUNK_BYTES) {
        let chunk = &mut chunk_buffer[..expected.len()];
        if read_full(&mut reader, chunk)? < chunk.len() || chunk != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn trace_write_error(error: io::Error) -> anyhow::Error {
    let message = match error.kind() {
        io::ErrorKind::QuotaExceeded | io::ErrorKind::StorageFull => {
            "try using `--output-path` to write traces outside \
            of the temporary directory"
        }
        _ => "failed to write to trace file",
    };
    anyhow!(error).context(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{State, open_trace};
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Metadata {
        n: u32,
    }

    /// The tags of the records of the trace in `directory`.
    fn tags(directory: &Path) -> Vec<u16> {
        let file = File::open(directory.join(TRACE_FILE_NAME)).unwrap();
        let mut file = io::BufReader::new(file);
        crate::envelope::read_envelope(&mut file).unwrap();
        let mut records = crate::record::RecordReader::new(file);
        std::iter::from_fn(|| records.next_record(false))
            .map(|record| record.unwrap().tag)
            .collect()
    }

    /// A state's payload and the state its record names, as read back.
    type ReadBack = (Option<Vec<u8>>, Option<usize>);

    /// Writes states with these payloads to a new trace in `directory`,
    /// holding the writer thread until all are queued, so they are
    /// written in one go, and the small ones stay buffered unless the
    /// writer flushes them to read them back. Returns each state's
    /// payload and the state its record names as read back, and how many
    /// times the writer flushed to compare payloads with older ones.
    fn write_payloads(
        directory: &Path,
        payloads: &[Option<Arc<Vec<u8>>>],
    ) -> (Vec<ReadBack>, usize) {
        /// A file whose second flush (the first after the envelope's)
        /// waits for the test, counting flushes.
        struct Gated {
            file: File,
            flushes: Arc<AtomicU64>,
            gate: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
        }

        impl Write for Gated {
            fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
                self.file.write(buffer)
            }

            fn flush(&mut self) -> io::Result<()> {
                let flushes = self.flushes.fetch_add(1, Ordering::Relaxed);
                if flushes == 1
                    && let Some((entered, release)) = self.gate.take()
                {
                    entered.send(()).unwrap();
                    release.recv_timeout(GATE_TIMEOUT).unwrap();
                }
                self.file.flush()
            }
        }

        assert!(payloads.len() < PENDING_STATES_MAX);
        let flushes = Arc::new(AtomicU64::new(0));
        let (entered_sender, entered) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let mut writer = TraceFileWriter::create_with(
            directory,
            TraceDriver::Terminal,
            false,
            |file| Gated {
                file,
                flushes: flushes.clone(),
                gate: Some((entered_sender, release_receiver)),
            },
        )
        .unwrap();
        // Declared after the writer, as in the disk full test.
        let release = release;
        entered.recv_timeout(GATE_TIMEOUT).unwrap();
        for (n, payload) in (0..).zip(payloads) {
            writer.write_state(Metadata { n }, payload.clone()).unwrap();
        }
        // Queued before the worker goes on, so it never idles (and
        // flushes) before it.
        let sender = writer.sender.as_ref().unwrap();
        sender.send(Message::Finish).unwrap();
        release.send(()).unwrap();
        writer.finish().unwrap();

        let states = open_trace::<Metadata>(directory)
            .unwrap()
            .map(|state| {
                let state = state.unwrap();
                (state.payload, state.payload_from)
            })
            .collect();
        // But for the envelope's, the gated one and the one after END,
        // the flushes before comparing with older payloads.
        let flushes = flushes.load(Ordering::Relaxed) as usize - 3;
        (states, flushes)
    }

    /// A payload longer than a payload reference, numbered `n`.
    fn long_payload(n: u8) -> Arc<Vec<u8>> {
        Arc::new([&b"a payload longer than a reference "[..], &[n]].concat())
    }

    #[test]
    fn writes_identical_payloads_once() {
        let directory = tempfile::tempdir().unwrap();
        let a = long_payload(b'a');
        let b = long_payload(b'b');
        // A copy of `a`, not the same allocation.
        let a_copy = Arc::new(a.to_vec());
        let payloads = [
            Some(a.clone()),
            // The previous payload, shared and copied: compared in
            // memory.
            Some(a.clone()),
            Some(a_copy.clone()),
            Some(b.clone()),
            None,
            // Older payloads, read back.
            Some(a_copy),
            Some(b.clone()),
            // Same length, but not the same.
            Some(long_payload(b'c')),
        ];
        let (states, flushes) = write_payloads(directory.path(), &payloads);
        let read: Vec<_> =
            states.iter().map(|(payload, _)| payload.clone()).collect();
        let written: Vec<_> = payloads
            .iter()
            .map(|payload| payload.as_ref().map(|payload| payload.to_vec()))
            .collect();
        assert_eq!(read, written);
        let from: Vec<_> = states.iter().map(|(_, from)| *from).collect();
        assert_eq!(
            from,
            [
                Some(0),
                Some(0),
                Some(0),
                Some(3),
                None,
                Some(0),
                Some(3),
                Some(7)
            ]
        );
        // Only the older payloads were read back.
        assert_eq!(flushes, 2);
        use tag::{
            END, STATE, STATE_PAYLOAD as PAYLOAD, STATE_PAYLOAD_REF as REF,
            STATE_WITH_PAYLOAD as WITH,
        };
        assert_eq!(
            tags(directory.path()),
            [
                WITH, PAYLOAD, WITH, REF, WITH, REF, WITH, PAYLOAD, STATE,
                WITH, REF, WITH, REF, WITH, PAYLOAD, END,
            ]
        );
    }

    /// Payloads no larger than a reference are written in full.
    #[test]
    fn writes_small_payloads_in_full() {
        let directory = tempfile::tempdir().unwrap();
        let small = Arc::new(vec![1; crate::PAYLOAD_REF_LEN]);
        let large = Arc::new(vec![2; crate::PAYLOAD_REF_LEN + 1]);
        let payloads = [
            Some(small.clone()),
            Some(small.clone()),
            Some(large.clone()),
            Some(large.clone()),
            Some(small.clone()),
        ];
        let (states, flushes) = write_payloads(directory.path(), &payloads);
        let from: Vec<_> = states.iter().map(|(_, from)| *from).collect();
        assert_eq!(from, [Some(0), Some(1), Some(2), Some(2), Some(4)]);
        assert_eq!(flushes, 0);
        for (state, payload) in states.iter().zip(payloads) {
            assert_eq!(state.0.as_deref(), payload.as_deref().map(|p| &p[..]));
        }
    }

    /// Payloads with the same CRC-32 and length are compared byte for
    /// byte, so they are each written, wherever they differ.
    #[test]
    fn writes_payloads_with_the_same_crc_separately() {
        // CRC-32 differences of equal-length inputs don't depend on what
        // comes before or after, so these collide however they're padded.
        let [x, y]: [&[u8]; 2] = [b"plumless", b"buckeroo"];
        assert_eq!(crc32fast::hash(x), crc32fast::hash(y));
        // Different in the first chunk read back, and in a later one.
        let padding = vec![7; 3 * READ_BACK_CHUNK_BYTES];
        for (x, y) in [
            ([x, &padding].concat(), [y, &padding].concat()),
            ([&padding, x].concat(), [&padding, y].concat()),
        ] {
            assert_eq!(crc32fast::hash(&x), crc32fast::hash(&y));
            let directory = tempfile::tempdir().unwrap();
            let payloads = [
                Some(Arc::new(x.clone())),
                None,
                Some(Arc::new(y.clone())),
                Some(Arc::new(x.clone())),
                Some(Arc::new(b"other".to_vec())),
                Some(Arc::new(y.clone())),
            ];
            let (states, _) = write_payloads(directory.path(), &payloads);
            assert_eq!(
                states,
                [
                    (Some(x.clone()), Some(0)),
                    (None, None),
                    (Some(y.clone()), Some(2)),
                    (Some(x.clone()), Some(0)),
                    (Some(b"other".to_vec()), Some(4)),
                    (Some(y.clone()), Some(2)),
                ]
            );
        }
    }

    /// Only the first few distinct payloads with the same CRC-32 and
    /// length are kept as candidates, and none no larger than a
    /// reference.
    #[test]
    fn keeps_few_candidates_per_crc_and_length() {
        let directory = tempfile::tempdir().unwrap();
        let file = File::create(directory.path().join("file")).unwrap();
        let mut payloads = WrittenPayloads::new(file);
        // As if all had this CRC-32.
        for n in 0..2 * CANDIDATES_PER_KEY_MAX as u8 {
            payloads.written(&long_payload(n), 7, n.into(), n.into());
        }
        let length = long_payload(0).len();
        let kept = &payloads.by_crc[&(7, length)];
        let expected: Vec<_> = (0..CANDIDATES_PER_KEY_MAX as u32)
            .map(|n| (n, n.into()))
            .collect();
        assert_eq!(kept, &expected);

        let small = Arc::new(vec![0; crate::PAYLOAD_REF_LEN]);
        payloads.written(&small, 8, 100, 100);
        assert!(!payloads.by_crc.contains_key(&(8, small.len())));
    }

    /// A failure to read back is warned of once, and older payloads
    /// aren't read back after it.
    #[test]
    fn stops_reading_back_after_a_failure() {
        crate::test_log::capture_warnings();
        let directory = tempfile::tempdir().unwrap();
        // Write-only, so reading back fails.
        let file = File::create(directory.path().join("file")).unwrap();
        let mut payloads = WrittenPayloads::new(file);
        let mut trace_file = BufWriter::new(io::sink());
        let a = long_payload(b'a');
        let b = long_payload(b'b');
        let crc = |payload: &[u8]| crc32fast::hash(payload);
        payloads.written(&a, crc(&a), 0, 100);
        payloads.written(&b, crc(&b), 1, 200);
        let copy = Arc::new(a.to_vec());
        let found = payloads.find(&copy, crc(&a), &mut trace_file).unwrap();
        assert_eq!(found, None);
        assert!(crate::test_log::warned("failed to read back"));
        assert!(payloads.read_back_failed);
        // Even once the file could be read back, with `a` where it was
        // written.
        let readable = directory.path().join("readable");
        std::fs::write(&readable, [&[0; 100][..], &a].concat()).unwrap();
        payloads.read_back = File::open(&readable).unwrap();
        let found = payloads.find(&copy, crc(&a), &mut trace_file).unwrap();
        assert_eq!(found, None);

        // The previous payload is still compared in memory.
        let c = long_payload(b'c');
        payloads.written(&c, crc(&c), 2, 300);
        let copy = Arc::new(c.to_vec());
        let found = payloads.find(&copy, crc(&c), &mut trace_file).unwrap();
        assert_eq!(found, Some(2));
        // But isn't kept as a candidate, as none are looked at.
        assert!(!payloads.by_crc.contains_key(&(crc(&c), c.len())));
    }

    #[test]
    fn writes_states_that_read_back() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = TraceFileWriter::create(
            directory.path(),
            TraceDriver::Terminal,
            false,
        )
        .unwrap();
        writer
            .write_state(Metadata { n: 0 }, Some(Arc::new(b"zero".to_vec())))
            .unwrap();
        writer.write_state(Metadata { n: 1 }, None).unwrap();
        writer.finish().unwrap();
        let error = writer.write_state(Metadata { n: 2 }, None).unwrap_err();
        assert!(error.to_string().contains("already finished"), "{error}");

        let mut reader = open_trace::<Metadata>(directory.path()).unwrap();
        assert_eq!(reader.driver, TraceDriver::Terminal);
        let states = reader
            .by_ref()
            .collect::<Result<Vec<State<Metadata>>>>()
            .unwrap();
        assert!(reader.finished());
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].metadata, Metadata { n: 0 });
        assert_eq!(states[0].payload.as_deref(), Some(&b"zero"[..]));
        assert_eq!(states[1].payload, None);
    }

    #[test]
    fn written_states_reach_the_file_without_finishing() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = TraceFileWriter::create(
            directory.path(),
            TraceDriver::Terminal,
            false,
        )
        .unwrap();
        // The envelope is there before any state.
        assert!(open_trace::<Metadata>(directory.path()).is_ok());
        writer.write_state(Metadata { n: 0 }, None).unwrap();
        // The writer flushes once it is idle; wait for that.
        let read = || {
            open_trace::<Metadata>(directory.path())
                .unwrap()
                .collect::<Result<Vec<_>>>()
                .unwrap()
                .len()
        };
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(10);
        while read() == 0 {
            assert!(std::time::Instant::now() < deadline, "never flushed");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // As if the process were killed.
        std::mem::forget(writer);
    }

    #[test]
    fn reports_failures_to_write() {
        #[derive(Serialize)]
        struct Unencodable(std::collections::HashMap<Vec<u8>, u32>);

        let directory = tempfile::tempdir().unwrap();
        let mut writer = TraceFileWriter::create(
            directory.path(),
            TraceDriver::Terminal,
            false,
        )
        .unwrap();
        let state = Unencodable([(vec![1], 1)].into_iter().collect());
        writer.write_state(state, None).unwrap();
        let error = format!("{:#}", writer.finish().unwrap_err());
        assert!(error.contains("failed to encode trace state"), "{error}");
        assert!(writer.finish().is_err());
        // The trace isn't marked as finished after a failure.
        let mut reader = open_trace::<Metadata>(directory.path()).unwrap();
        assert_eq!(reader.by_ref().count(), 0);
        assert!(!reader.finished());
    }

    /// Fails to encode, with a message unique to the test.
    struct FailsToEncode(&'static str);

    impl Serialize for FailsToEncode {
        fn serialize<S: serde::Serializer>(
            &self,
            _: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(self.0))
        }
    }

    #[test]
    fn drop_logs_only_failures_not_returned_already() {
        crate::test_log::capture_warnings();
        let directory = tempfile::tempdir().unwrap();
        let create = |name| {
            TraceFileWriter::create(
                &directory.path().join(name),
                TraceDriver::Terminal,
                false,
            )
            .unwrap()
        };

        let mut writer = create("reported");
        writer
            .write_state(FailsToEncode("drop-test-reported"), None)
            .unwrap();
        assert!(writer.finish().is_err());
        drop(writer);
        assert!(!crate::test_log::warned("drop-test-reported"));

        let mut writer = create("unreported");
        writer
            .write_state(FailsToEncode("drop-test-unreported"), None)
            .unwrap();
        drop(writer);
        assert!(crate::test_log::warned("drop-test-unreported"));
    }

    #[test]
    fn drop_writes_queued_states_without_end() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = TraceFileWriter::create(
            directory.path(),
            TraceDriver::Terminal,
            false,
        )
        .unwrap();
        let payload = |n: u32| {
            n.is_multiple_of(3)
                .then(|| Arc::new(vec![n as u8; 1000 + n as usize]))
        };
        // More than fit in the queue.
        let count = 4 * PENDING_STATES_MAX as u32;
        for n in 0..count {
            writer.write_state(Metadata { n }, payload(n)).unwrap();
        }
        // Without `finish`, as on a panic or an early return.
        drop(writer);

        let mut reader = open_trace::<Metadata>(directory.path()).unwrap();
        let states = reader
            .by_ref()
            .collect::<Result<Vec<State<Metadata>>>>()
            .unwrap();
        assert!(!reader.finished());
        assert_eq!(states.len(), count as usize);
        for (n, state) in (0..count).zip(states) {
            assert_eq!(state.metadata, Metadata { n });
            assert_eq!(state.payload.map(Arc::new), payload(n));
        }
    }

    #[test]
    fn reports_the_message_of_a_writer_thread_panic() {
        /// Panics with a `&'static str` payload, or with a `String` one
        /// formatted from `n` if set.
        struct PanicsToEncode {
            n: Option<u32>,
        }

        impl Serialize for PanicsToEncode {
            fn serialize<S: serde::Serializer>(
                &self,
                _: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                if let Some(n) = self.n {
                    panic!("injected panic while encoding {n}");
                }
                panic!("injected panic while encoding");
            }
        }

        for (n, message) in [
            (None, "injected panic while encoding"),
            (Some(1), "injected panic while encoding 1"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut writer = TraceFileWriter::create(
                directory.path(),
                TraceDriver::Terminal,
                false,
            )
            .unwrap();
            writer.write_state(PanicsToEncode { n }, None).unwrap();
            let error = format!("{:#}", writer.finish().unwrap_err());
            assert!(error.contains(message), "{error}");
        }
    }

    #[test]
    fn concurrent_writers_keep_their_own_traces() {
        for overwrite in [false, true] {
            for _ in 0..20 {
                concurrent_writers(overwrite);
            }
        }
    }

    /// Creates two traces in the same directory at once, and checks that
    /// `trace.bin` is all of one whose creation succeeded.
    fn concurrent_writers(overwrite: bool) {
        const STATES: u32 = 10;
        let directory = tempfile::tempdir().unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let drivers = [TraceDriver::Terminal, TraceDriver::Browser];
        let threads = drivers.map(|driver| {
            let run_path = directory.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let mut writer =
                    TraceFileWriter::create(&run_path, driver, overwrite)?;
                for n in 0..STATES {
                    let n = u32::from(driver.code()) * 100 + n;
                    writer.write_state(Metadata { n }, None)?;
                }
                writer.finish()
            })
        });
        let results = threads.map(|thread| thread.join().unwrap());

        let mut reader = open_trace::<Metadata>(directory.path()).unwrap();
        let driver = reader.driver;
        let numbers: Vec<_> = reader
            .by_ref()
            .map(|state| state.unwrap().metadata.n)
            .collect();
        assert!(reader.finished());
        let first = u32::from(driver.code()) * 100;
        assert_eq!(numbers, (first..first + STATES).collect::<Vec<_>>());
        assert_eq!(files(directory.path()), [TRACE_FILE_NAME]);
        let published = drivers.iter().position(|&d| d == driver).unwrap();
        assert!(results[published].is_ok(), "{:?}", results[published]);
        let other = &results[1 - published];
        if overwrite {
            // Both succeed; the other trace was replaced, as asked.
            assert!(other.is_ok(), "{other:?}");
        } else {
            // The other one failed to create its trace, rather than being
            // told it wrote states that aren't there.
            let error = format!("{:#}", other.as_ref().unwrap_err());
            assert!(error.contains("already exists"), "{error}");
        }
    }

    #[test]
    fn keeps_failing_after_a_failed_write() {
        #[derive(Serialize)]
        struct Unencodable(std::collections::HashMap<Vec<u8>, u32>);

        let directory = tempfile::tempdir().unwrap();
        let mut writer = TraceFileWriter::create(
            directory.path(),
            TraceDriver::Terminal,
            false,
        )
        .unwrap();
        let state = || Unencodable([(vec![1], 1)].into_iter().collect());
        writer.write_state(state(), None).unwrap();
        // The worker stops on the first state, so a later `write_state`
        // sees the failure once the worker has stopped.
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(10);
        let first = loop {
            if let Err(error) = writer.write_state(state(), None) {
                break error;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker never failed"
            );
        };
        // Whichever call first sees the failure gets the original error,
        // and every later one, including `finish`, reports it too.
        let errors = [
            first,
            writer.write_state(state(), None).unwrap_err(),
            writer.finish().unwrap_err(),
            writer.finish().unwrap_err(),
        ];
        for error in errors {
            let error = format!("{error:#}");
            assert!(error.contains("failed to encode trace state"), "{error}");
        }
    }

    #[test]
    fn create_leaves_no_file_for_an_invalid_driver() {
        let directory = tempfile::tempdir().unwrap();
        let run_path = directory.path().join("run");
        let create = |driver| {
            TraceFileWriter::<Metadata>::create(&run_path, driver, false)
        };
        for driver in [TraceDriver::Unknown(0), TraceDriver::Unknown(2)] {
            let error = format!("{:#}", create(driver).err().unwrap());
            assert!(error.contains("invalid trace driver code"), "{error}");
            // Checked before anything is created, even the run directory.
            assert!(!run_path.exists());
        }
        create(TraceDriver::Terminal).unwrap().finish().unwrap();
    }

    #[test]
    fn refuses_oversized_payloads_up_front() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = TraceFileWriter::create(
            directory.path(),
            TraceDriver::Terminal,
            false,
        )
        .unwrap();
        let payload = Arc::new(vec![0; crate::RECORD_PAYLOAD_BYTES_MAX + 1]);
        let error = writer
            .write_state(Metadata { n: 0 }, Some(payload))
            .unwrap_err();
        assert!(format!("{error:#}").contains("too large"), "{error:#}");
        writer.finish().unwrap();
        let states = open_trace::<Metadata>(directory.path())
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(states.is_empty());
    }

    #[test]
    fn refuses_to_overwrite_unless_asked() {
        let directory = tempfile::tempdir().unwrap();
        let create = |driver, overwrite| {
            TraceFileWriter::<Metadata>::create(
                directory.path(),
                driver,
                overwrite,
            )
        };
        let mut writer = create(TraceDriver::Terminal, false).unwrap();
        writer.write_state(Metadata { n: 1 }, None).unwrap();
        writer.finish().unwrap();
        let trace_path = directory.path().join(TRACE_FILE_NAME);
        let trace = std::fs::read(&trace_path).unwrap();

        let error =
            format!("{:#}", create(TraceDriver::Browser, false).err().unwrap());
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(std::fs::read(&trace_path).unwrap(), trace);

        let mut writer = create(TraceDriver::Browser, true).unwrap();
        writer.write_state(Metadata { n: 2 }, None).unwrap();
        writer.finish().unwrap();
        assert_eq!(files(directory.path()), [TRACE_FILE_NAME]);
        let mut reader = open_trace::<Metadata>(directory.path()).unwrap();
        assert_eq!(reader.driver, TraceDriver::Browser);
        let numbers: Vec<_> = reader
            .by_ref()
            .map(|state| state.unwrap().metadata.n)
            .collect();
        assert_eq!(numbers, [2]);
        assert!(reader.finished());
    }

    /// Fails to write, or only to flush.
    struct Failing {
        write: bool,
    }

    impl Write for Failing {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.write {
                return Err(io::Error::other("injected write failure"));
            }
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("injected flush failure"))
        }
    }

    // How long either side of a `DiskFull` gate waits for the other.
    const GATE_TIMEOUT: std::time::Duration =
        std::time::Duration::from_secs(10);

    /// Takes the envelope, then refuses every write as if the disk were
    /// full, numbering the refusals. Flushing itself succeeds, as it
    /// holds nothing back. With a gate, the first flush after the
    /// envelope's signals the first channel, then waits for the second.
    struct DiskFull {
        accepted: usize,
        refused: usize,
        flushes: usize,
        gate: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    }

    impl Write for DiskFull {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.accepted + buffer.len() <= crate::ENVELOPE_LEN {
                self.accepted += buffer.len();
                return Ok(buffer.len());
            }
            self.refused += 1;
            Err(io::Error::new(
                io::ErrorKind::StorageFull,
                format!("disk full (refused write {})", self.refused),
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.flushes == 2
                && let Some((entered, release)) = self.gate.take()
            {
                entered.send(()).unwrap();
                release
                    .recv_timeout(GATE_TIMEOUT)
                    .expect("the test never released the writer thread");
            }
            Ok(())
        }
    }

    #[test]
    fn reports_failures_when_the_disk_is_full() {
        crate::test_log::capture_warnings();
        let directory = tempfile::tempdir().unwrap();
        let create = |name: &str, gate| {
            TraceFileWriter::create_with(
                &directory.path().join(name),
                TraceDriver::Terminal,
                false,
                |_| DiskFull {
                    accepted: 0,
                    refused: 0,
                    flushes: 0,
                    gate,
                },
            )
            .unwrap()
        };

        // Small states stay buffered, so a flush fails: with no states,
        // always the one after END.
        let mut writer = create("small-0", None);
        let error = format!("{:#}", writer.finish().unwrap_err());
        assert!(error.contains("--output-path"), "{error}");

        // Hold the worker in its first flush until the states are
        // queued, so it can't fail a flush between them, which would
        // make the next `write_state` fail.
        let (entered_sender, entered) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let mut writer =
            create("small-3", Some((entered_sender, release_receiver)));
        // Declared after the writer, so a panic drops it first, freeing
        // the worker that dropping the writer joins.
        let release = release;
        entered
            .recv_timeout(GATE_TIMEOUT)
            .expect("the writer thread never flushed");
        for n in 0..3 {
            writer.write_state(Metadata { n }, None).unwrap();
        }
        release.send(()).unwrap();
        let error = format!("{:#}", writer.finish().unwrap_err());
        assert!(error.contains("--output-path"), "{error}");

        // A state larger than the buffer fails as it is written, and
        // that error is reported rather than a later flush's.
        let mut writer = create("large", None);
        let payload = Arc::new(vec![0; 9 * 1024]);
        writer
            .write_state(Metadata { n: 0 }, Some(payload))
            .unwrap();
        let error = format!("{:#}", writer.finish().unwrap_err());
        assert!(error.contains("--output-path"), "{error}");
        assert!(error.contains("refused write 1)"), "{error}");

        // Dropped, the writer flushes the queued states as it stops, and
        // logs the failure. Hold the worker in its first flush until the
        // state is queued and the channel closed, so that flush is the
        // only one after the state.
        let (entered_sender, entered) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let mut writer =
            create("dropped", Some((entered_sender, release_receiver)));
        // Declared after the writer, as above.
        let release = release;
        entered
            .recv_timeout(GATE_TIMEOUT)
            .expect("the writer thread never flushed");
        writer.write_state(Metadata { n: 0 }, None).unwrap();
        writer.sender = None;
        release.send(()).unwrap();
        drop(writer);
        assert!(crate::test_log::warned("disk full (refused write 1)"));
    }

    /// The names of the files in `directory`.
    fn files(directory: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn create_leaves_nothing_behind_if_it_fails() {
        let directory = tempfile::tempdir().unwrap();
        let create = |overwrite, write| {
            TraceFileWriter::<Metadata>::create_with(
                directory.path(),
                TraceDriver::Terminal,
                overwrite,
                |_| Failing { write },
            )
        };
        for write in [true, false] {
            let error = format!("{:#}", create(false, write).err().unwrap());
            assert!(error.contains("injected"), "{error}");
            assert!(files(directory.path()).is_empty());
        }

        // An existing trace survives a failed overwrite.
        let mut writer = TraceFileWriter::create(
            directory.path(),
            TraceDriver::Terminal,
            false,
        )
        .unwrap();
        writer.write_state(Metadata { n: 7 }, None).unwrap();
        writer.finish().unwrap();
        let trace_path = directory.path().join(TRACE_FILE_NAME);
        let trace = std::fs::read(&trace_path).unwrap();
        for write in [true, false] {
            assert!(create(true, write).is_err());
            assert_eq!(files(directory.path()), [TRACE_FILE_NAME]);
            assert_eq!(std::fs::read(&trace_path).unwrap(), trace);
        }

        // A failed rename over the existing trace leaves it as it was.
        let directory = tempfile::tempdir().unwrap();
        let trace_path = directory.path().join(TRACE_FILE_NAME);
        std::fs::create_dir(&trace_path).unwrap();
        std::fs::write(trace_path.join("kept"), b"kept").unwrap();
        assert!(
            TraceFileWriter::<Metadata>::create(
                directory.path(),
                TraceDriver::Terminal,
                true,
            )
            .is_err()
        );
        assert_eq!(files(directory.path()), [TRACE_FILE_NAME]);
        assert_eq!(std::fs::read(trace_path.join("kept")).unwrap(), b"kept");
    }

    #[test]
    fn create_skips_taken_temporary_names() {
        let directory = tempfile::tempdir().unwrap();
        // The names this tries first, unless tests running at the same
        // time take them.
        let next = TEMP_FILES_CREATED.load(Ordering::Relaxed);
        let taken: Vec<_> = (next..next + 3)
            .map(|n| temp_path(directory.path(), n))
            .collect();
        for path in &taken {
            std::fs::write(path, b"taken").unwrap();
        }
        let mut writer = TraceFileWriter::<Metadata>::create(
            directory.path(),
            TraceDriver::Terminal,
            true,
        )
        .unwrap();
        writer.write_state(Metadata { n: 0 }, None).unwrap();
        writer.finish().unwrap();
        for path in &taken {
            assert_eq!(std::fs::read(path).unwrap(), b"taken");
            std::fs::remove_file(path).unwrap();
        }
        assert_eq!(files(directory.path()), [TRACE_FILE_NAME]);
        let mut reader = open_trace::<Metadata>(directory.path()).unwrap();
        assert_eq!(reader.by_ref().count(), 1);
        assert!(reader.finished());
    }
}
