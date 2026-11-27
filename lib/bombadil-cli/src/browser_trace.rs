//! Reading binary browser traces and their embedded screenshots.

use std::path::Path;

#[cfg(test)]
use bombadil::model::Recorded;
#[cfg(test)]
use bombadil_schema::browser::BrowserTraceEntry;
#[cfg(test)]
use std::{fs::File, io::BufReader};

use anyhow::{Result, bail};
use bombadil::shrink::{TraceActions, read_trace_actions};
use bombadil_schema::browser::BrowserAction;
use bombadil_trace::{TraceDriver, open_trace};

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
pub type BrowserRecords = bombadil_trace::TraceReader<
    BufReader<File>,
    BrowserTraceEntry,
    Recorded<BrowserAction>,
>;

/// Opens a binary browser trace.
#[cfg(test)]
pub fn open_binary(file: &Path) -> Result<BrowserRecords> {
    open_trace(file)?.expect_driver(TraceDriver::Browser)
}

#[cfg(test)]
pub fn load_actions(path: &Path) -> Result<Vec<Recorded<BrowserAction>>> {
    Ok(load_trace_actions(path)?.actions)
}

/// The actions of the browser trace at `path` and the earliest violation
/// of each property, for shrinking.
pub fn load_trace_actions(path: &Path) -> Result<TraceActions<BrowserAction>> {
    let file = resolve_browser_trace_file(path)?;
    let trace = read_trace_actions(
        open_trace(&file)?
            .expect_driver(TraceDriver::Browser)?
            .without_payloads(),
    )?;
    // A run killed early leaves actions and no states.
    if trace.states == 0 && trace.actions.is_empty() {
        bail!("{} holds no states", path.display());
    }
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_schema::ActionKey;
    use bombadil_trace::{TRACE_FILE_NAME, tag, write_envelope, write_record};
    use std::collections::BTreeMap;

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
        let Some(Ok(bombadil_trace::Record::State { action, state })) =
            states.next()
        else {
            panic!("no state");
        };
        assert_eq!(action.unwrap().action, BrowserAction::Back);
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

    /// Actions carry their `action_taken` key, or `None` where the trace
    /// has none.
    #[test]
    fn reads_the_recorded_template_keys() {
        let directory = tempfile::tempdir().unwrap();
        let with_key = STATE.replace(
            r#""screenshot""#,
            r#""action_taken":"00000000000000a1","screenshot""#,
        );
        assert_ne!(with_key, STATE);
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Browser).unwrap();
        for metadata in [&with_key, STATE] {
            write_record(&mut file, tag::STATE, &[metadata.as_bytes()])
                .unwrap();
        }
        write_record(&mut file, tag::END, &[]).unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), file).unwrap();
        let recorded = |template| Recorded {
            action: BrowserAction::Back,
            template,
        };
        assert_eq!(
            load_actions(&directory.path().join(TRACE_FILE_NAME)).unwrap(),
            vec![recorded(Some(ActionKey(0xa1))), recorded(None)]
        );
    }

    /// Actions that no state followed are loaded too, even alone.
    #[test]
    fn reads_the_actions_no_state_followed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let recorded = |action| Recorded {
            action,
            template: Some(ActionKey(0xa1)),
        };
        let state: serde_json::Value = serde_json::from_str(
            &STATE.replace(r#""action":"Back""#, r#""action":null"#),
        )
        .unwrap();
        let mut writer = bombadil_trace::TraceFileWriter::create(
            &path,
            TraceDriver::Browser,
            false,
        )
        .unwrap();
        writer.write_state(state.clone(), None).unwrap();
        writer
            .write_action(&recorded(BrowserAction::Reload))
            .unwrap();
        writer.write_state(state, None).unwrap();
        writer
            .write_action(&recorded(BrowserAction::Forward))
            .unwrap();
        writer
            .write_error(bombadil_trace::StepError::Stalled)
            .unwrap();
        writer.finish().unwrap();
        let both = vec![
            recorded(BrowserAction::Reload),
            recorded(BrowserAction::Forward),
        ];
        assert_eq!(load_actions(&path).unwrap(), both);

        // Cut before its first chunk of states.
        let file = std::fs::read(&path).unwrap();
        let chunk = file
            .windows(2)
            .position(|tag| tag == tag::CHUNK.to_le_bytes())
            .unwrap();
        std::fs::write(&path, &file[..chunk]).unwrap();
        assert_eq!(load_actions(&path).unwrap(), both);
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
