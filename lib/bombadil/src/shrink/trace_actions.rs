//! Loading the actions and property violations of a recorded trace.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, de::DeserializeOwned};

#[derive(Debug, Clone)]
pub struct TraceActions<A> {
    pub actions: Vec<A>,
    /// Number of actions before each property's earliest violation.
    pub violations_earliest: BTreeMap<String, usize>,
}

#[derive(Deserialize)]
struct TraceEntryActionAndViolations<A> {
    action: Option<A>,
    #[serde(default)]
    violations: Vec<ViolationName>,
}

#[derive(Deserialize)]
struct ViolationName {
    name: String,
}

/// Skips states and snapshots without allocating them.
pub fn load_trace_actions<A: DeserializeOwned>(
    trace_file_path: &Path,
) -> Result<TraceActions<A>> {
    let file = File::open(trace_file_path).map_err(|error| {
        anyhow!(
            "failed to open trace file {}: {}",
            trace_file_path.display(),
            error
        )
    })?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut line_number = 0;
    let mut actions = Vec::new();
    let mut violations_earliest = BTreeMap::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        line_number += 1;
        if line.trim().is_empty() {
            continue;
        }
        let entry: TraceEntryActionAndViolations<A> =
            serde_json::from_str(&line).with_context(|| {
                format!(
                    "failed to parse {}:{}",
                    trace_file_path.display(),
                    line_number
                )
            })?;
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

impl<A> TraceActions<A> {
    pub fn map_actions<B>(self, f: impl FnMut(A) -> B) -> TraceActions<B> {
        TraceActions {
            actions: self.actions.into_iter().map(f).collect(),
            violations_earliest: self.violations_earliest,
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

    #[test]
    fn loads_actions_and_earliest_violations() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        for line in [
            r#"{"timestamp":0,"action":null,"state":{"big":[1,2,3]},"snapshots":[],"violations":[]}"#,
            r#"{"timestamp":1,"action":"a","state":{},"snapshots":[],"violations":[{"name":"p","violation":{}}]}"#,
            r#"{"timestamp":2,"action":"b","state":{},"snapshots":[],"violations":[{"name":"q","violation":{}}]}"#,
            r#"{"timestamp":3,"action":"c","state":{},"snapshots":[],"violations":[{"name":"p","violation":{}}]}"#,
        ] {
            writeln!(file, "{line}").unwrap();
        }
        let trace = load_trace_actions::<String>(file.path()).unwrap();
        assert_eq!(trace.actions, vec!["a", "b", "c"]);
        assert_eq!(trace.select_property(Some("p")).unwrap(), ("p".into(), 1));
        assert_eq!(trace.select_property(Some("q")).unwrap(), ("q".into(), 2));
        assert!(trace.select_property(None).is_err());
        assert!(trace.select_property(Some("r")).is_err());
    }
}
