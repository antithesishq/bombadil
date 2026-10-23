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
use bombadil_trace::{TraceDriver, open_trace};
use serde::Deserialize;

use crate::output_path::resolve_browser_trace_file;

/// File name of a legacy browser trace in a run directory.
pub const JSON_TRACE_FILE_NAME: &str = "trace.jsonl";

/// A browser trace on disk, as found by [`resolve_browser_trace_file`].
#[derive(Debug, PartialEq)]
pub enum BrowserTraceFile {
    Binary(PathBuf),
    /// A legacy trace, with its screenshots as files next to it.
    Json(PathBuf),
}

/// The media type of a screenshot format recorded in a binary trace, or
/// `None` for a format bombadil doesn't write. The format comes from the
/// trace file, so only these are used in file names and responses.
pub fn screenshot_media_type(format: &str) -> Option<&'static str> {
    match format {
        "png" => Some("image/png"),
        "jpeg" => Some("image/jpeg"),
        _ => None,
    }
}

#[cfg(test)]
pub type BrowserStates =
    bombadil_trace::StateReader<BufReader<File>, BrowserTraceEntry>;

/// Opens a binary browser trace.
#[cfg(test)]
pub fn open_binary(file: &Path) -> Result<BrowserStates> {
    open_trace(file)?.expect_driver(TraceDriver::Browser)
}

/// The entries of a legacy JSON trace up to its first bad line, and the
/// error that line gave, if any.
pub fn read_json_prefix(
    file: &Path,
) -> Result<(Vec<BrowserTraceEntry>, Option<anyhow::Error>)> {
    let reader = BufReader::new(File::open(file).with_context(|| {
        format!("failed to open trace file {}", file.display())
    })?);
    let mut entries = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let location = || format!("{}:{}", file.display(), index + 1);
        let entry = line
            .with_context(|| format!("failed to read {}", location()))
            .and_then(|line| {
                if line.trim().is_empty() {
                    return Ok(None);
                }
                serde_json::from_str(&line)
                    .map(Some)
                    .with_context(|| format!("failed to parse {}", location()))
            });
        match entry {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(error) => return Ok((entries, Some(error))),
        }
    }
    Ok((entries, None))
}

/// The recorded actions of the browser trace at `path`, for replaying.
pub fn load_actions(path: &Path) -> Result<Vec<BrowserAction>> {
    Ok(load_trace_actions(path)?.actions)
}

/// The actions of the browser trace at `path` and the earliest violation
/// of each property, for shrinking.
pub fn load_trace_actions(path: &Path) -> Result<TraceActions<BrowserAction>> {
    match resolve_browser_trace_file(path)? {
        BrowserTraceFile::Json(file) => shrink::load_trace_actions(&file),
        BrowserTraceFile::Binary(file) => {
            // Only what is needed, as the legacy loader reads it: the rest
            // (e.g. snapshot values) would be read into `serde_json::Value`s,
            // which fail on, or misread, some JSON.
            #[derive(Deserialize)]
            struct Entry {
                #[serde(default)]
                action: Option<BrowserAction>,
                #[serde(default)]
                violations: Vec<Violation>,
            }
            #[derive(Deserialize)]
            struct Violation {
                name: String,
            }
            let mut actions = Vec::new();
            let mut violations_earliest = BTreeMap::new();
            let mut states = 0;
            let reader = open_trace::<Entry>(&file)?
                .expect_driver(TraceDriver::Browser)?
                .without_payloads();
            for state in reader {
                let entry = state?.metadata;
                states += 1;
                if let Some(action) = entry.action {
                    actions.push(action);
                }
                for violation in entry.violations {
                    violations_earliest
                        .entry(violation.name)
                        .or_insert(actions.len());
                }
            }
            if states == 0 {
                bail!("{} holds no states", path.display());
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
    use bombadil_trace::{TRACE_FILE_NAME, tag, write_envelope, write_record};

    const STATE: &str = r#"{"timestamp":0,"action":"Back","state":{"url":"https://example.com/","hash_previous":null,"hash_current":null,"screenshot":"jpeg","resources":{"js_heap_used":0,"js_heap_total":0,"dom_nodes":0,"documents":0,"js_event_listeners":0,"layout_objects":0,"timestamp":0.0,"thread_time":0.0,"task_duration":0.0,"script_duration":0.0}},"snapshots":[],"violations":[]}"#;

    fn binary_trace(directory: &Path) {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Browser).unwrap();
        write_record(&mut file, tag::STATE_WITH_PAYLOAD, &[STATE.as_bytes()])
            .unwrap();
        write_record(&mut file, tag::STATE_PAYLOAD, &[b"image"]).unwrap();
        write_record(&mut file, tag::END, &[]).unwrap();
        std::fs::write(directory.join(TRACE_FILE_NAME), file).unwrap();
    }

    #[test]
    fn reads_binary_traces() {
        let directory = tempfile::tempdir().unwrap();
        binary_trace(directory.path());
        assert_eq!(load_actions(directory.path()).unwrap().len(), 1);
        let mut states =
            open_binary(&directory.path().join(TRACE_FILE_NAME)).unwrap();
        let state = states.next().unwrap().unwrap();
        assert_eq!(state.metadata.state.screenshot, "jpeg");
        assert_eq!(state.payload.as_deref(), Some(&b"image"[..]));
        assert!(states.next().is_none());
        assert!(states.finished());
    }

    /// The actions are read without the rest of the metadata, so a
    /// snapshot value a `serde_json::Value` can't read doesn't matter.
    #[test]
    fn reads_actions_whatever_the_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let state = STATE.replace(
            r#""snapshots":[]"#,
            r#""snapshots":[{"index":0,"name":"x","value":{"$serde_json::private::RawValue":5},"time":0}]"#,
        );
        // Which the whole entry can't be read as.
        assert!(serde_json::from_str::<BrowserTraceEntry>(&state).is_err());
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Browser).unwrap();
        write_record(&mut file, tag::STATE, &[state.as_bytes()]).unwrap();
        write_record(&mut file, tag::END, &[]).unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), file).unwrap();
        assert_eq!(load_actions(directory.path()).unwrap().len(), 1);
    }

    /// Each property's earliest violation is the number of actions up
    /// to the first state violating it.
    #[test]
    fn finds_the_earliest_violations() {
        let directory = tempfile::tempdir().unwrap();
        let state = |action: &str, violations: &[&str]| {
            let violations: Vec<String> = violations
                .iter()
                .map(|name| format!(r#"{{"name":"{name}","violation":{{"False":{{"time":0,"condition":"c","snapshots":[]}}}}}}"#))
                .collect();
            STATE
                .replace(r#""action":"Back""#, &format!(r#""action":{action}"#))
                .replace(
                    r#""violations":[]"#,
                    &format!(r#""violations":[{}]"#, violations.join(",")),
                )
        };
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Browser).unwrap();
        for metadata in [
            state("null", &[]),
            state(r#""Back""#, &[]),
            state(r#""Forward""#, &["p"]),
            state(r#""Reload""#, &["p", "q"]),
        ] {
            write_record(&mut file, tag::STATE, &[metadata.as_bytes()])
                .unwrap();
        }
        write_record(&mut file, tag::END, &[]).unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), file).unwrap();
        let trace = load_trace_actions(directory.path()).unwrap();
        assert_eq!(trace.actions.len(), 3);
        assert_eq!(
            trace.violations_earliest,
            BTreeMap::from([("p".to_string(), 2), ("q".to_string(), 3)])
        );
    }

    #[test]
    fn refuses_an_empty_binary_trace() {
        let directory = tempfile::tempdir().unwrap();
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Browser).unwrap();
        write_record(&mut file, tag::END, &[]).unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), file).unwrap();
        assert_eq!(
            load_actions(directory.path()).unwrap_err().to_string(),
            format!("{} holds no states", directory.path().display())
        );
    }

    #[test]
    fn reads_legacy_json_traces() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(JSON_TRACE_FILE_NAME);
        std::fs::write(&file, format!("{STATE}\n\n{STATE}\n")).unwrap();
        assert_eq!(load_actions(directory.path()).unwrap().len(), 2);
        let (entries, error) = read_json_prefix(&file).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(error.is_none());
    }

    /// A bad line ends the entries of a JSON trace, after those before it.
    #[test]
    fn reads_a_json_trace_up_to_a_bad_line() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(JSON_TRACE_FILE_NAME);
        std::fs::write(&file, format!("{STATE}\n{{\n{STATE}\n")).unwrap();
        let (entries, error) = read_json_prefix(&file).unwrap();
        assert_eq!(entries.len(), 1);
        let expected = format!("failed to parse {}:2", file.display());
        assert_eq!(error.unwrap().to_string(), expected);
    }
}
