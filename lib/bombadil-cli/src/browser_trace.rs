//! Reading browser traces: `trace.bin`, or the legacy `trace.jsonl` with
//! its `screenshots` directory.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use bombadil::shrink::{self, TraceActions};
use bombadil_schema::browser::{BrowserAction, BrowserTraceEntry};
use bombadil_trace::{
    StateReader, TRACE_FILE_NAME, TraceDriver, open_trace, trace_driver,
};

use crate::output_path::resolve_trace_directory;

/// File name of a legacy browser trace in a run directory.
pub const JSON_TRACE_FILE_NAME: &str = "trace.jsonl";

/// A browser trace on disk.
#[derive(Debug, PartialEq)]
pub enum BrowserTraceFile {
    Binary(PathBuf),
    /// A legacy trace, with its screenshots as files next to it.
    Json(PathBuf),
}

/// Finds the browser trace at a user-provided path (trace file or run
/// directory), refusing other drivers' traces so they aren't misread.
pub fn resolve(path: &Path) -> Result<BrowserTraceFile> {
    match trace_driver(path)? {
        Some(TraceDriver::Browser) => {
            let file = if path.is_dir() {
                path.join(TRACE_FILE_NAME)
            } else {
                path.to_path_buf()
            };
            Ok(BrowserTraceFile::Binary(file))
        }
        Some(driver) => bail!(
            "{} holds a {driver} trace, not a browser trace",
            path.display()
        ),
        None => {
            let file = resolve_trace_directory(path).join(JSON_TRACE_FILE_NAME);
            if !file.is_file() {
                bail!("no browser trace at {}", path.display());
            }
            Ok(BrowserTraceFile::Json(file))
        }
    }
}

pub type BrowserStates = StateReader<BufReader<File>, BrowserTraceEntry>;

/// Opens a binary browser trace.
pub fn open_binary(file: &Path) -> Result<BrowserStates> {
    open_trace(file)?.expect_driver(TraceDriver::Browser)
}

/// The entries of a legacy JSON trace.
pub fn read_json(file: &Path) -> Result<Vec<BrowserTraceEntry>> {
    let reader = BufReader::new(File::open(file).with_context(|| {
        format!("failed to open trace file {}", file.display())
    })?);
    let mut entries = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.context("failed to read line from trace")?;
        if line.trim().is_empty() {
            continue;
        }
        entries.push(serde_json::from_str(&line).with_context(|| {
            format!("failed to parse {}:{}", file.display(), index + 1)
        })?);
    }
    Ok(entries)
}

/// The recorded actions of the browser trace at `path`, for replaying.
pub fn load_actions(path: &Path) -> Result<Vec<BrowserAction>> {
    Ok(load_trace_actions(path)?.actions)
}

/// The actions of the browser trace at `path` and the earliest violation
/// of each property, for shrinking.
pub fn load_trace_actions(path: &Path) -> Result<TraceActions<BrowserAction>> {
    match resolve(path)? {
        BrowserTraceFile::Json(file) => shrink::load_trace_actions(&file),
        BrowserTraceFile::Binary(file) => {
            let mut actions = Vec::new();
            let mut violations_earliest = BTreeMap::new();
            for state in open_binary(&file)? {
                let entry = state?.metadata;
                if let Some(action) = entry.action {
                    actions.push(action);
                }
                for violation in entry.violations {
                    violations_earliest
                        .entry(violation.name)
                        .or_insert(actions.len());
                }
            }
            Ok(TraceActions {
                actions,
                violations_earliest,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_trace::{tag, write_envelope, write_record};

    const STATE: &str = r#"{"timestamp":0,"action":"Back","state":{"url":"https://example.com/","hash_previous":null,"hash_current":null,"screenshot":"jpeg","resources":{"js_heap_used":0,"js_heap_total":0,"dom_nodes":0,"documents":0,"js_event_listeners":0,"layout_objects":0,"timestamp":0.0,"thread_time":0.0,"task_duration":0.0,"script_duration":0.0}},"snapshots":[],"violations":[]}"#;

    fn binary_trace(directory: &Path, driver: TraceDriver) {
        let mut file = Vec::new();
        write_envelope(&mut file, driver).unwrap();
        write_record(&mut file, tag::STATE, &[STATE.as_bytes()]).unwrap();
        write_record(&mut file, tag::STATE_PAYLOAD, &[b"image"]).unwrap();
        std::fs::write(directory.join(TRACE_FILE_NAME), file).unwrap();
    }

    #[test]
    fn reads_binary_traces() {
        let directory = tempfile::tempdir().unwrap();
        binary_trace(directory.path(), TraceDriver::Browser);
        assert_eq!(
            resolve(directory.path()).unwrap(),
            BrowserTraceFile::Binary(directory.path().join(TRACE_FILE_NAME))
        );
        assert_eq!(load_actions(directory.path()).unwrap().len(), 1);
        let state = open_binary(&directory.path().join(TRACE_FILE_NAME))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(state.metadata.state.screenshot, "jpeg");
        assert_eq!(state.payload.as_deref(), Some(&b"image"[..]));
    }

    #[test]
    fn reads_legacy_json_traces() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(JSON_TRACE_FILE_NAME),
            format!("{STATE}\n{STATE}\n"),
        )
        .unwrap();
        assert!(matches!(
            resolve(directory.path()).unwrap(),
            BrowserTraceFile::Json(_)
        ));
        assert_eq!(load_actions(directory.path()).unwrap().len(), 2);
    }

    #[test]
    fn refuses_other_drivers_and_missing_traces() {
        let directory = tempfile::tempdir().unwrap();
        let error = resolve(directory.path()).unwrap_err();
        assert!(error.to_string().contains("no browser trace"), "{error}");

        binary_trace(directory.path(), TraceDriver::Terminal);
        let error = resolve(directory.path()).unwrap_err();
        assert!(
            error.to_string().contains("holds a terminal trace"),
            "{error}"
        );
    }
}
