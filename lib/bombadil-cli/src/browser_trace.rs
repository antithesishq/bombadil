//! Reading binary browser traces and their embedded screenshots.

use std::{collections::BTreeMap, path::Path};

#[cfg(test)]
use bombadil_schema::browser::BrowserTraceEntry;
#[cfg(test)]
use std::{fs::File, io::BufReader};

use anyhow::{Result, bail};
use bombadil::shrink::TraceActions;
use bombadil_schema::browser::BrowserAction;
use bombadil_trace::{TraceDriver, open_trace};
use serde::Deserialize;

use crate::output_path::resolve_browser_trace_file;

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

/// The recorded actions of the browser trace at `path`, for replaying.
pub fn load_actions(path: &Path) -> Result<Vec<BrowserAction>> {
    Ok(load_trace_actions(path)?.actions)
}

/// The actions of the browser trace at `path` and the earliest violation
/// of each property, for shrinking.
pub fn load_trace_actions(path: &Path) -> Result<TraceActions<BrowserAction>> {
    let file = resolve_browser_trace_file(path)?;
    // Only what is needed: the rest
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

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_trace::{TRACE_FILE_NAME, tag, write_envelope, write_record};

    const STATE: &str = r#"{"timestamp":0,"action":"Back","state":{"url":"https://example.com/","screenshot":"jpeg","resources":{"js_heap_used":0,"js_heap_total":0,"dom_nodes":0,"documents":0,"js_event_listeners":0,"layout_objects":0,"timestamp":0.0,"thread_time":0.0,"task_duration":0.0,"script_duration":0.0}},"snapshots":[],"violations":[]}"#;

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
        assert_eq!(
            load_actions(&directory.path().join(TRACE_FILE_NAME))
                .unwrap()
                .len(),
            1
        );
        let mut states =
            open_binary(&directory.path().join(TRACE_FILE_NAME)).unwrap();
        let state = states.next().unwrap().unwrap();
        assert_eq!(state.metadata.state.screenshot, "jpeg");
        assert_eq!(state.payload.as_deref(), Some(&b"image"[..]));
        assert!(states.next().is_none());
        assert!(states.finished());
    }

    /// Traces written before the coverage transition hashes were dropped
    /// carry them in the state, under the same format version.
    #[test]
    fn reads_states_with_transition_hashes() {
        let state = STATE.replace(
            r#""screenshot""#,
            r#""hash_previous":null,"hash_current":7,"screenshot""#,
        );
        assert_ne!(state, STATE);
        assert_eq!(
            serde_json::from_str::<BrowserTraceEntry>(&state).unwrap(),
            serde_json::from_str::<BrowserTraceEntry>(STATE).unwrap()
        );
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
        assert_eq!(
            load_actions(&directory.path().join(TRACE_FILE_NAME))
                .unwrap()
                .len(),
            1
        );
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
        let trace = load_trace_actions(&directory.path().join(TRACE_FILE_NAME))
            .unwrap();
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
            load_actions(&directory.path().join(TRACE_FILE_NAME))
                .unwrap_err()
                .to_string(),
            format!(
                "{} holds no states",
                directory.path().join(TRACE_FILE_NAME).display()
            )
        );
    }

    #[test]
    fn refuses_json_lines_instead_of_a_binary_trace() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("trace.jsonl");
        std::fs::write(&file, format!("{STATE}\n")).unwrap();
        assert!(load_actions(&file).is_err());
        assert!(load_actions(directory.path()).is_err());
    }
}
