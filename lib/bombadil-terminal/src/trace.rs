//! Terminal traces, stored as `trace.bin` in each run directory.
//!
//! Every integer is little-endian. The file starts with a header:
//!
//! ```text
//! magic "BMBDTRM\0" (8 bytes) | format version u16 | ghostty version length u16 | ghostty version (UTF-8)
//! ```
//!
//! followed by one record per state:
//!
//! ```text
//! metadata length u32 | snapshot length u32 | metadata | snapshot
//! ```
//!
//! The metadata is a JSON [`TerminalTraceEntry`] (action, cursor, exit
//! status, extractor snapshots, violations). The snapshot is ghostty's
//! binary encoding of the whole terminal (`GHOSTSNP`), holding the grid,
//! scrollback and modes; it is empty when none could be encoded for that
//! state. Each snapshot carries its own format version, which ghostty's
//! decoder checks; that format has no cross-version compatibility
//! guarantee yet. The ghostty version in the header is informational,
//! and does not distinguish unreleased builds (e.g. `0.1.0-dev`).

use std::{
    fs::File,
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread::JoinHandle,
};

use anyhow::{Context, Result, anyhow, bail};
use bombadil::{driver::OutputWriter, specification::convert::ToSchema};
use bombadil::{driver::RunId, specification::domain::Snapshot};
use bombadil::{driver::TraceWriter, runner::PropertyViolation};
use bombadil_schema::Time;
use bombadil_schema::terminal::{TerminalStateSummary, TerminalTraceEntry};
use serde_json as json;

use crate::{
    driver::{TerminalAction, TerminalSession},
    state::TerminalState,
};

pub const TRACE_FILE_NAME: &str = "trace.bin";

const MAGIC: &[u8; 8] = b"BMBDTRM\0";
const FORMAT_VERSION: u16 = 1;

pub struct TerminalOutputWriter {
    pub root_path: PathBuf,
    pub overwrite: bool,
}

impl OutputWriter<TerminalSession> for TerminalOutputWriter {
    type TraceWriter = TerminalTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path =
            self.root_path.join("runs").join(format!("{}", run_id.0));
        TerminalTraceWriter::initialize(run_path, self.overwrite)
    }
}

/// Writes trace records on a dedicated thread so that disk I/O overlaps
/// with the test loop instead of stalling it. The bounded channel
/// provides backpressure if the writer cannot keep up.
pub struct TerminalTraceWriter {
    sender: mpsc::SyncSender<Message>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl Drop for TerminalTraceWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

enum Message {
    Record(Box<OwnedRecord>),
    Flush(mpsc::SyncSender<Result<()>>),
}

struct OwnedRecord {
    entry: TerminalTraceEntry,
    // Shared with the state it was encoded for, so it reaches the file
    // without being copied.
    terminal_snapshot: Option<Arc<Vec<u8>>>,
}

// Bounds the number of in-flight records to cap memory while letting the
// writer thread run behind the test loop.
const PENDING_RECORDS_MAX: usize = 32;

impl TerminalTraceWriter {
    pub fn initialize(run_path: PathBuf, overwrite: bool) -> Result<Self> {
        std::fs::create_dir_all(&run_path)?;
        let trace_path = run_path.join(TRACE_FILE_NAME);
        if trace_path.try_exists()? {
            if !overwrite {
                bail!(
                    "{TRACE_FILE_NAME} already exists at {}. \
                     Use --output-path-overwrite to overwrite, or choose a different --output-path.",
                    trace_path.display(),
                );
            }
            std::fs::remove_file(&trace_path)?;
        }
        let mut trace_file = BufWriter::new(
            File::options()
                .write(true)
                .create_new(true)
                .open(&trace_path)?,
        );
        write_header(
            &mut trace_file,
            libghostty_vt::build_info::version_string()?,
        )
        .map_err(trace_write_error)?;
        log::info!("storing run trace in {}", run_path.display());
        let (sender, receiver) = mpsc::sync_channel(PENDING_RECORDS_MAX);
        let worker = std::thread::Builder::new()
            .name("bombadil-trace-writer".to_string())
            .spawn(move || worker_loop(receiver, trace_file))?;
        Ok(Self {
            sender,
            worker: Some(worker),
        })
    }

    pub fn flush(&mut self) -> Result<()> {
        let (ack_sender, ack_receiver) = mpsc::sync_channel(1);
        if self.sender.send(Message::Flush(ack_sender)).is_err() {
            return Err(self.worker_failure());
        }
        match ack_receiver.recv() {
            Ok(result) => result,
            Err(_) => Err(self.worker_failure()),
        }
    }

    /// Joins the worker thread to surface the error that made it exit.
    fn worker_failure(&mut self) -> anyhow::Error {
        match self.worker.take() {
            Some(worker) => match worker.join() {
                Ok(Ok(())) => {
                    anyhow!("trace writer thread exited unexpectedly")
                }
                Ok(Err(error)) => error,
                Err(_) => anyhow!("trace writer thread panicked"),
            },
            None => anyhow!("trace writer thread already failed"),
        }
    }
}

impl TraceWriter<TerminalSession> for TerminalTraceWriter {
    #[hotpath::measure]
    fn write(
        &mut self,
        state: &TerminalState,
        last_action: Option<&TerminalAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let record = Box::new(OwnedRecord {
            entry: TerminalTraceEntry {
                timestamp: Time::from_system_time(state.timestamp),
                action: last_action.map(ToSchema::to_schema),
                state: TerminalStateSummary {
                    scroll_offset: state.scroll_offset,
                    cursor: state.cursor.clone(),
                    exit_status: state.exit_status.clone(),
                },
                snapshots: snapshots.iter().map(|s| s.to_schema()).collect(),
                violations: violations.iter().map(|v| v.to_schema()).collect(),
            },
            terminal_snapshot: state.terminal_snapshot.clone(),
        });
        if self.sender.send(Message::Record(record)).is_err() {
            return Err(self.worker_failure());
        }
        Ok(())
    }
}

fn write_header(
    writer: &mut impl Write,
    ghostty_version: &str,
) -> io::Result<()> {
    let version_length = u16::try_from(ghostty_version.len())
        .map_err(|_| io::Error::other("ghostty version string too long"))?;
    writer.write_all(MAGIC)?;
    writer.write_all(&FORMAT_VERSION.to_le_bytes())?;
    writer.write_all(&version_length.to_le_bytes())?;
    writer.write_all(ghostty_version.as_bytes())
}

fn write_record(
    writer: &mut impl Write,
    metadata: &[u8],
    snapshot: &[u8],
) -> io::Result<()> {
    let length = |bytes: &[u8]| {
        u32::try_from(bytes.len())
            .map_err(|_| io::Error::other("trace record too large"))
    };
    writer.write_all(&length(metadata)?.to_le_bytes())?;
    writer.write_all(&length(snapshot)?.to_le_bytes())?;
    writer.write_all(metadata)?;
    // Snapshots are larger than the `BufWriter` buffer, which then
    // writes them to the file directly instead of copying them.
    writer.write_all(snapshot)
}

fn worker_loop(
    receiver: mpsc::Receiver<Message>,
    mut trace_file: BufWriter<File>,
) -> Result<()> {
    let mut metadata = Vec::new();
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Record(record) => {
                metadata.clear();
                json::to_writer(&mut metadata, &record.entry)?;
                write_record(
                    &mut trace_file,
                    &metadata,
                    record
                        .terminal_snapshot
                        .as_deref()
                        .map_or(&[], Vec::as_slice),
                )
                .map_err(trace_write_error)?;
            }
            Message::Flush(ack) => {
                let result = trace_file.flush().map_err(trace_write_error);
                // The flusher may have given up waiting; ignore that.
                let _ = ack.send(result);
            }
        }
    }
    trace_file.flush().map_err(trace_write_error)?;
    Ok(())
}

fn trace_write_error(error: io::Error) -> anyhow::Error {
    let message = match error.kind() {
        io::ErrorKind::QuotaExceeded => {
            "try using `--output-path` to write traces outside \
            of the temporary directory"
        }
        _ => "failed to write to trace file",
    };

    anyhow!(error).context(message)
}

/// One state read back from a trace.
pub struct TraceRecord {
    pub entry: TerminalTraceEntry,
    /// Ghostty's snapshot of the terminal, or `None` if none was encoded.
    pub terminal_snapshot: Option<Vec<u8>>,
}

/// Reads the records of a `trace.bin`, in the order they were written.
pub struct TraceReader<R> {
    reader: R,
    /// Version of the ghostty that wrote the trace (informational).
    pub ghostty_version: String,
}

impl TraceReader<BufReader<File>> {
    /// Opens a trace file, or the trace in a run directory.
    pub fn open(path: &Path) -> Result<Self> {
        let path = if path.is_dir() {
            path.join(TRACE_FILE_NAME)
        } else {
            path.to_path_buf()
        };
        let file = File::open(&path).with_context(|| {
            format!("failed to open trace file {}", path.display())
        })?;
        TraceReader::new(BufReader::new(file))
            .with_context(|| format!("failed to read {}", path.display()))
    }
}

impl<R: Read> TraceReader<R> {
    pub fn new(mut reader: R) -> Result<Self> {
        let mut magic = [0; MAGIC.len()];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            bail!("not a bombadil terminal trace");
        }
        let format_version = read_u16(&mut reader)?;
        if format_version != FORMAT_VERSION {
            bail!(
                "unsupported terminal trace format version {format_version} \
                 (expected {FORMAT_VERSION})"
            );
        }
        let mut ghostty_version = vec![0; usize::from(read_u16(&mut reader)?)];
        reader.read_exact(&mut ghostty_version)?;
        Ok(TraceReader {
            reader,
            ghostty_version: String::from_utf8(ghostty_version)?,
        })
    }

    fn read_record(&mut self) -> Result<Option<TraceRecord>> {
        let mut lengths = [0; 8];
        // A clean end of the file is only allowed between records.
        let mut filled = 0;
        while filled < lengths.len() {
            match self.reader.read(&mut lengths[filled..])? {
                0 if filled == 0 => return Ok(None),
                0 => bail!("truncated trace record"),
                read => filled += read,
            }
        }
        let [m0, m1, m2, m3, s0, s1, s2, s3] = lengths;
        let metadata_length = u32::from_le_bytes([m0, m1, m2, m3]) as usize;
        let snapshot_length = u32::from_le_bytes([s0, s1, s2, s3]) as usize;

        let mut metadata = vec![0; metadata_length];
        self.reader
            .read_exact(&mut metadata)
            .context("truncated trace record")?;
        let mut snapshot = vec![0; snapshot_length];
        self.reader
            .read_exact(&mut snapshot)
            .context("truncated trace record")?;
        Ok(Some(TraceRecord {
            entry: json::from_slice(&metadata)?,
            terminal_snapshot: (!snapshot.is_empty()).then_some(snapshot),
        }))
    }
}

impl<R: Read> Iterator for TraceReader<R> {
    type Item = Result<TraceRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        self.read_record().transpose()
    }
}

fn read_u16(reader: &mut impl Read) -> io::Result<u16> {
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use bombadil_schema::terminal::{
        TerminalColor, TerminalCursor, TerminalCursorPosition,
        TerminalCursorVisualStyle, TerminalGrid, TerminalSize,
    };

    use super::*;

    fn state(terminal_snapshot: Option<Vec<u8>>) -> TerminalState {
        let size = TerminalSize {
            columns: 10,
            rows: 2,
        };
        TerminalState {
            timestamp: SystemTime::now(),
            grid: TerminalGrid::with_size(size),
            scrollback: TerminalGrid::with_size(TerminalSize {
                rows: 0,
                ..size
            }),
            scroll_offset: 3,
            cursor: TerminalCursor {
                position: TerminalCursorPosition { column: 5, row: 0 },
                visible: true,
                blinking: false,
                visual_style: TerminalCursorVisualStyle::Block,
                color: TerminalColor::None,
            },
            exit_status: None,
            last_action: None,
            terminal_snapshot: terminal_snapshot.map(Arc::new),
        }
    }

    #[test]
    fn test_trace_round_trip() {
        let mut terminal = libghostty_vt::Terminal::new(10, 2).unwrap();
        terminal.set_continuation_max_bytes(1024).unwrap();
        // End mid-escape-sequence so the snapshot has to carry it.
        terminal.vt_write(b"hello\x1b[3");
        let mut snapshot = Vec::new();
        terminal.encode_snapshot(&mut snapshot).unwrap();

        let directory = tempfile::tempdir().unwrap();
        let action = TerminalAction::TypeText {
            text: "hi".to_string(),
        };
        let mut writer =
            TerminalTraceWriter::initialize(directory.path().into(), false)
                .unwrap();
        writer
            .write(&state(Some(snapshot.clone())), Some(&action), &[], &[])
            .unwrap();
        writer.write(&state(None), None, &[], &[]).unwrap();
        drop(writer);

        let reader = TraceReader::open(directory.path()).unwrap();
        assert_eq!(
            reader.ghostty_version,
            libghostty_vt::build_info::version_string().unwrap()
        );
        let records = reader.collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(records.len(), 2);

        assert!(matches!(
            &records[0].entry.action,
            Some(bombadil_schema::terminal::TerminalAction::TypeText { text })
                if text == "hi"
        ));
        assert_eq!(records[0].entry.state.scroll_offset, 3);
        assert_eq!(records[0].entry.state.cursor.position.column, 5);
        assert!(records[1].entry.action.is_none());
        assert!(records[1].terminal_snapshot.is_none());

        let restored_snapshot = records[0].terminal_snapshot.as_ref().unwrap();
        assert_eq!(restored_snapshot, &snapshot);
        let mut restored =
            libghostty_vt::snapshot::Decoder::new_buf(restored_snapshot)
                .unwrap()
                .decode()
                .unwrap();
        assert_eq!(restored.cursor_x().unwrap(), 5);
        restored.vt_write(b"1mX");
        assert_eq!(restored.cursor_x().unwrap(), 6);
    }

    #[test]
    fn test_trace_rejects_truncated_record() {
        let mut file = Vec::new();
        write_header(&mut file, "test").unwrap();
        write_record(&mut file, b"{}", b"snapshot").unwrap();
        file.truncate(file.len() - 1);

        let mut reader = TraceReader::new(file.as_slice()).unwrap();
        assert_eq!(reader.ghostty_version, "test");
        let error = reader.next().unwrap().err().unwrap();
        assert!(error.to_string().contains("truncated"), "{error}");
    }

    #[test]
    fn test_trace_rejects_other_files() {
        let error =
            TraceReader::new(&b"{\"timestamp\":0}\n"[..]).err().unwrap();
        assert!(error.to_string().contains("not a bombadil"), "{error}");
    }
}
