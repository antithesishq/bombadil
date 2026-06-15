use std::{
    io::{BufWriter, Write},
    path::PathBuf,
    sync::mpsc,
    thread::JoinHandle,
};

use anyhow::{Result, anyhow};
use bombadil::runner::PropertyViolation;
use bombadil::specification::convert::ToSchema;
use bombadil::specification::domain::Snapshot;
use bombadil_schema::{TerminalCell, TerminalGrid, Time, TraceEntry};
use serde_json as json;
use std::fs::File;

use crate::{driver::TerminalAction, state::TerminalState};

pub type TerminalTraceEntry =
    TraceEntry<TerminalAction, bombadil_schema::TerminalStateSummary>;

/// Writes trace entries on a dedicated thread so that JSON
/// serialization and disk I/O (hundreds of kilobytes per state) overlap
/// with the test loop instead of stalling it. The bounded channel
/// provides backpressure if the writer cannot keep up.
pub struct TraceWriter {
    sender: mpsc::SyncSender<Message>,
    worker: Option<JoinHandle<Result<()>>>,
}

enum Message {
    Entry(Box<OwnedEntry>),
    Flush(mpsc::SyncSender<Result<()>>),
}

struct OwnedEntry {
    state: TerminalState,
    action: Option<TerminalAction>,
    snapshots: Vec<bombadil_schema::Snapshot>,
    violations: Vec<bombadil_schema::PropertyViolation>,
}

/// Writes a trace entry byte-identically to serializing
/// `TerminalTraceEntry` with serde, but lays out the (large, highly
/// repetitive) grid cell arrays manually: runs of identical cells are
/// emitted by copying the previously serialized bytes instead of
/// re-running serde for every cell. This is the hot path of the test
/// loop, dominated by grid serialization.
fn write_entry(
    buffer: &mut Vec<u8>,
    timestamp: Time,
    action: Option<&TerminalAction>,
    state: &TerminalState,
    snapshots: &[bombadil_schema::Snapshot],
    violations: &[bombadil_schema::PropertyViolation],
) -> Result<()> {
    buffer.extend_from_slice(b"{\"timestamp\":");
    json::to_writer(&mut *buffer, &timestamp)?;
    buffer.extend_from_slice(b",\"action\":");
    json::to_writer(&mut *buffer, &action)?;
    buffer.extend_from_slice(b",\"state\":{\"grid\":");
    write_grid(buffer, &state.grid)?;
    buffer.extend_from_slice(b",\"scrollback\":");
    write_grid(buffer, &state.scrollback)?;
    buffer.extend_from_slice(b",\"scroll_offset\":");
    json::to_writer(&mut *buffer, &state.scroll_offset)?;
    buffer.extend_from_slice(b",\"cursor\":");
    json::to_writer(&mut *buffer, &state.cursor)?;
    buffer.extend_from_slice(b",\"exit_status\":");
    json::to_writer(&mut *buffer, &state.exit_status)?;
    buffer.extend_from_slice(b"},\"snapshots\":");
    json::to_writer(&mut *buffer, snapshots)?;
    buffer.extend_from_slice(b",\"violations\":");
    json::to_writer(&mut *buffer, violations)?;
    buffer.push(b'}');
    Ok(())
}

// Number of distinct cells whose serialized bytes are cached while
// writing one grid. Text rows reuse a small alphabet of cells, so a
// small ring cache turns almost every cell into a byte copy.
const CELL_CACHE_SIZE: usize = 64;

fn write_grid(buffer: &mut Vec<u8>, grid: &TerminalGrid) -> Result<()> {
    buffer.extend_from_slice(b"{\"cells\":[");
    // Ranges index into `buffer`, which only grows while a grid is
    // written, so cached ranges stay valid for the whole call.
    let mut cache: Vec<(&TerminalCell, std::ops::Range<usize>)> =
        Vec::with_capacity(CELL_CACHE_SIZE);
    let mut next_slot = 0;
    let mut first = true;
    for cell in grid {
        if !first {
            buffer.push(b',');
        }
        first = false;
        match cache.iter().find(|(cached, _)| *cached == cell) {
            Some((_, range)) => {
                buffer.extend_from_within(range.clone());
            }
            None => {
                let start = buffer.len();
                json::to_writer(&mut *buffer, cell)?;
                let entry = (cell, start..buffer.len());
                if cache.len() < CELL_CACHE_SIZE {
                    cache.push(entry);
                } else {
                    cache[next_slot] = entry;
                    next_slot = (next_slot + 1) % CELL_CACHE_SIZE;
                }
            }
        }
    }
    buffer.extend_from_slice(b"],\"size\":");
    json::to_writer(&mut *buffer, &grid.size)?;
    buffer.push(b'}');
    Ok(())
}

// Bounds the number of in-flight entries (each a full grid clone) to
// cap memory while letting the writer thread run behind the test loop.
const PENDING_ENTRIES_MAX: usize = 32;

impl TraceWriter {
    pub fn initialize(root_path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root_path)?;
        let trace_path = root_path.join("trace.jsonl");
        let trace_file = File::options()
            .append(true)
            .create(true)
            .open(&trace_path)?;
        log::info!("storing trace in {}", root_path.display());
        let (sender, receiver) = mpsc::sync_channel(PENDING_ENTRIES_MAX);
        let worker = std::thread::Builder::new()
            .name("bombadil-trace-writer".to_string())
            .spawn(move || worker_loop(receiver, BufWriter::new(trace_file)))?;
        Ok(Self {
            sender,
            worker: Some(worker),
        })
    }

    #[hotpath::measure]
    pub fn write(
        &mut self,
        state: &TerminalState,
        last_action: Option<&TerminalAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let entry = Box::new(OwnedEntry {
            state: state.clone(),
            action: last_action.cloned(),
            snapshots: snapshots.iter().map(|s| s.to_schema()).collect(),
            violations: violations.iter().map(|v| v.to_schema()).collect(),
        });
        if self.sender.send(Message::Entry(entry)).is_err() {
            return Err(self.worker_failure());
        }
        Ok(())
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

fn worker_loop(
    receiver: mpsc::Receiver<Message>,
    mut trace_file: BufWriter<File>,
) -> Result<()> {
    let mut buffer = Vec::new();
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Entry(entry) => {
                buffer.clear();
                write_entry(
                    &mut buffer,
                    Time::from_system_time(entry.state.timestamp),
                    entry.action.as_ref(),
                    &entry.state,
                    &entry.snapshots,
                    &entry.violations,
                )?;
                buffer.push(b'\n');
                trace_file.write_all(&buffer)?;
            }
            Message::Flush(ack) => {
                let result = trace_file.flush().map_err(Into::into);
                // The flusher may have given up waiting; ignore that.
                let _ = ack.send(result);
            }
        }
    }
    trace_file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use bombadil_schema::{
        TerminalAttributes, TerminalColor, TerminalCursor,
        TerminalCursorPosition, TerminalCursorVisualStyle, TerminalSize,
        TerminalStateSummary, TerminalStyle, TerminalUnderline,
    };
    use small_string::SmallString;

    use serde::Serialize;

    use super::*;

    /// The reference layout this writer must stay byte-compatible with.
    #[derive(Serialize)]
    struct DerivedEntry<'a> {
        timestamp: Time,
        action: Option<&'a TerminalAction>,
        state: TerminalStateSummary,
        snapshots: Vec<bombadil_schema::Snapshot>,
        violations: Vec<bombadil_schema::PropertyViolation>,
    }

    #[test]
    fn write_entry_matches_derived_serde_output() {
        let size = TerminalSize {
            columns: 3,
            rows: 2,
        };
        let style = TerminalStyle {
            foreground_color: TerminalColor::Palette(3),
            background_color: TerminalColor::RGB { r: 1, g: 2, b: 3 },
            underline_color: TerminalColor::None,
            underline: TerminalUnderline::Curly,
            attributes: TerminalAttributes::BOLD,
        };
        let cells = vec![
            TerminalCell::Occupied {
                contents: SmallString::from(['a'].as_slice()),
                wide: false,
                style: style.clone(),
            },
            TerminalCell::Empty {
                style: TerminalStyle::default(),
            },
            TerminalCell::Empty {
                style: TerminalStyle::default(),
            },
            TerminalCell::Continuation { style },
            TerminalCell::Empty {
                style: TerminalStyle::default(),
            },
            TerminalCell::Occupied {
                contents: SmallString::from(['"', 'x', '\\'].as_slice()),
                wide: true,
                style: TerminalStyle::default(),
            },
        ];
        let state = TerminalState {
            timestamp: SystemTime::now(),
            grid: TerminalGrid::from_cells(size, cells),
            scrollback: TerminalGrid::with_size(TerminalSize {
                rows: 0,
                ..size
            }),
            scroll_offset: 7,
            cursor: TerminalCursor {
                position: TerminalCursorPosition { column: 1, row: 2 },
                visible: true,
                blinking: false,
                visual_style: TerminalCursorVisualStyle::Block,
                color: TerminalColor::None,
            },
            exit_status: None,
            last_action: None,
        };
        let action = TerminalAction::TypeText {
            text: "hi".to_string(),
        };
        let timestamp = Time::from_system_time(state.timestamp);

        let mut buffer = Vec::new();
        write_entry(&mut buffer, timestamp, Some(&action), &state, &[], &[])
            .expect("manual serialization failed");

        let derived = json::to_string(&DerivedEntry {
            timestamp,
            action: Some(&action),
            state: TerminalStateSummary {
                grid: state.grid.clone(),
                scrollback: state.scrollback.clone(),
                scroll_offset: state.scroll_offset,
                cursor: state.cursor.clone(),
                exit_status: state.exit_status,
            },
            snapshots: vec![],
            violations: vec![],
        })
        .expect("derived serialization failed");

        assert_eq!(String::from_utf8(buffer).unwrap(), derived);
    }
}
