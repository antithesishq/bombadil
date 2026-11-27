//! Loading the actions and property violations of a recorded trace.

use std::{collections::BTreeMap, io::Read, path::Path};

use anyhow::{Result, bail};
use bombadil_trace::{Record, StepError, TraceReader};
use serde::{Deserialize, de::DeserializeOwned};

use crate::model::Recorded;

#[derive(Debug, Clone)]
pub struct TraceActions<A> {
    /// Every action of the trace, those no state followed included.
    pub actions: Vec<Recorded<A>>,
    /// Number of actions before each property's earliest violation.
    pub violations_earliest: BTreeMap<String, usize>,
    pub states: usize,
    /// Why no state followed the last action, if none did.
    pub ended: Option<StepError>,
}

#[derive(Deserialize)]
pub struct StateViolations {
    #[serde(default)]
    violations: Vec<ViolationName>,
}

#[derive(Deserialize)]
struct ViolationName {
    name: String,
}

pub fn load_trace_actions<A: DeserializeOwned>(
    trace_file_path: &Path,
) -> Result<TraceActions<A>> {
    read_trace_actions(
        bombadil_trace::open_trace(trace_file_path)?.without_payloads(),
    )
}

pub fn read_trace_actions<R: Read, A: DeserializeOwned>(
    reader: TraceReader<R, StateViolations, Recorded<A>>,
) -> Result<TraceActions<A>> {
    let mut trace = TraceActions {
        actions: Vec::new(),
        violations_earliest: BTreeMap::new(),
        states: 0,
        ended: None,
    };
    for record in reader {
        match record? {
            Record::State { action, state } => {
                trace.states += 1;
                trace.actions.extend(action);
                for violation in state.metadata.violations {
                    trace
                        .violations_earliest
                        .entry(violation.name)
                        .or_insert(trace.actions.len());
                }
            }
            Record::Error { actions, error, .. } => {
                trace.actions.extend(actions);
                trace.ended = Some(error);
            }
        }
    }
    Ok(trace)
}

impl<A> TraceActions<A> {
    /// Maps the actions, keeping their recorded keys.
    pub fn map_actions<B>(self, mut f: impl FnMut(A) -> B) -> TraceActions<B> {
        TraceActions {
            actions: self
                .actions
                .into_iter()
                .map(|recorded| recorded.map(&mut f))
                .collect(),
            violations_earliest: self.violations_earliest,
            states: self.states,
            ended: self.ended,
        }
    }

    /// The given property, or the only violated one, with the number of
    /// actions before its earliest violation.
    pub fn select_property(
        &self,
        property_name: Option<&str>,
    ) -> Result<(String, usize)> {
        let names = || {
            self.violations_earliest
                .keys()
                .map(|name| format!("  - {name}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        if self.violations_earliest.is_empty() {
            bail!("the trace has no property violations to shrink");
        }
        match property_name {
            Some(name) => match self.violations_earliest.get(name) {
                Some(&index) => Ok((name.to_string(), index)),
                None => bail!(
                    "property `{name}` is not violated in the trace, violated properties are:\n{}",
                    names()
                ),
            },
            None if self.violations_earliest.len() == 1 => {
                let (name, &index) = self
                    .violations_earliest
                    .iter()
                    .next()
                    .expect("one violation");
                Ok((name.clone(), index))
            }
            None => bail!(
                "the trace has violations of multiple properties, pick one with --property:\n{}",
                names()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_schema::ActionKey;

    #[test]
    fn loads_actions_and_earliest_violations() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        bombadil_trace::write_envelope(
            &mut file,
            bombadil_trace::TraceDriver::Browser,
        )
        .unwrap();
        for line in [
            r#"{"timestamp":0,"action":null,"state":{"big":[1,2,3]},"snapshots":[],"violations":[]}"#,
            r#"{"timestamp":1,"action":"a","state":{"action_taken":"00000000000000a1"},"snapshots":[],"violations":[{"name":"p","violation":{}}]}"#,
            r#"{"timestamp":2,"action":"b","state":{},"snapshots":[],"violations":[{"name":"q","violation":{}}]}"#,
            r#"{"timestamp":3,"action":"c","snapshots":[],"violations":[{"name":"p","violation":{}}]}"#,
        ] {
            bombadil_trace::write_record(
                &mut file,
                bombadil_trace::tag::STATE,
                &[line.as_bytes()],
            )
            .unwrap();
        }
        bombadil_trace::write_record(&mut file, bombadil_trace::tag::END, &[])
            .unwrap();
        let trace = load_trace_actions::<String>(file.path()).unwrap();
        let recorded = |action: &str, template| Recorded {
            action: action.to_string(),
            template,
        };
        assert_eq!(
            trace.actions,
            vec![
                recorded("a", Some(ActionKey(0xa1))),
                recorded("b", None),
                recorded("c", None),
            ]
        );
        assert_eq!(trace.select_property(Some("p")).unwrap(), ("p".into(), 1));
        assert_eq!(trace.select_property(Some("q")).unwrap(), ("q".into(), 2));
        assert!(trace.select_property(None).is_err());
        assert!(trace.select_property(Some("r")).is_err());
    }

    /// Shrinking never replays the action no state followed: violations
    /// are of states, which all come before it.
    #[test]
    fn loads_the_action_no_state_followed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.bin");
        let recorded = |action: &str| Recorded {
            action: action.to_string(),
            template: Some(ActionKey(0xa1)),
        };
        let state = |violations: &[&str]| {
            let violations: Vec<_> = violations
                .iter()
                .map(|name| serde_json::json!({ "name": name }))
                .collect();
            serde_json::json!({ "violations": violations })
        };
        let mut writer = bombadil_trace::TraceFileWriter::create(
            &path,
            bombadil_trace::TraceDriver::Browser,
            false,
        )
        .unwrap();
        writer.write_state(state(&[]), None).unwrap();
        writer.write_action(&recorded("a")).unwrap();
        writer.write_state(state(&["p"]), None).unwrap();
        writer.write_action(&recorded("b")).unwrap();
        writer
            .write_error(bombadil_trace::StepError::Stalled)
            .unwrap();
        writer.finish().unwrap();

        let trace = load_trace_actions::<String>(&path).unwrap();
        assert_eq!(trace.actions, vec![recorded("a"), recorded("b")]);
        assert_eq!(trace.states, 2);
        assert_eq!(trace.ended, Some(bombadil_trace::StepError::Stalled));
        assert_eq!(trace.select_property(None).unwrap(), ("p".into(), 1));
    }
}
