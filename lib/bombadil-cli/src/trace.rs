//! `bombadil trace`: show the states of a binary trace.

use std::{
    io::{self, BufWriter, Write},
    ops::Range,
    path::PathBuf,
    str::FromStr,
};

use anyhow::{Result, bail};
use bombadil_trace::{State, TRACE_FILE_NAME, open_trace};
use serde_json::{Value, json};

use crate::output_path;

#[derive(clap::Args)]
pub struct TraceArgs {
    /// Trace file (`trace.bin`), or the run directory containing it
    trace_path: PathBuf,
    /// Only show this state, or a range of states such as `40..50`
    #[arg(long, value_name = "INDEX|START..END")]
    state: Option<StateRange>,
    /// Print one JSON object per state: its index, metadata and payload
    /// size
    #[arg(long)]
    json: bool,
}

/// States to show, by index.
#[derive(Clone, Debug, PartialEq)]
struct StateRange(Range<usize>);

impl FromStr for StateRange {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let index = |text: &str| {
            text.parse::<usize>()
                .map_err(|_| format!("invalid state index `{text}`"))
        };
        let range = match value.split_once("..") {
            None => {
                let index = index(value)?;
                index..index + 1
            }
            Some((start, end)) => {
                let start = if start.is_empty() { 0 } else { index(start)? };
                let end = if end.is_empty() {
                    usize::MAX
                } else {
                    index(end)?
                };
                start..end
            }
        };
        Ok(StateRange(range))
    }
}

pub fn run(args: TraceArgs) -> Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    match show(&args, &mut out).and_then(|()| Ok(out.flush()?)) {
        // The reader went away, e.g. `bombadil trace … | head`.
        Err(error)
            if error.downcast_ref::<io::Error>().map(io::Error::kind)
                == Some(io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        result => result,
    }
}

fn show(args: &TraceArgs, out: &mut impl Write) -> Result<()> {
    let directory = output_path::resolve_trace_directory(&args.trace_path);
    if !args.trace_path.is_file()
        && !directory.join(TRACE_FILE_NAME).exists()
        && directory.join("trace.jsonl").exists()
    {
        bail!(
            "{} holds a JSON trace (trace.jsonl), which can be read directly",
            args.trace_path.display()
        );
    }
    let states = open_trace::<Value>(&args.trace_path)?;
    let range = args
        .state
        .as_ref()
        .map_or(0..usize::MAX, |range| range.0.clone());
    if !args.json {
        writeln!(out, "{} trace", states.driver)?;
    }
    let mut start_time = None;
    for state in states {
        let state = state?;
        let time = state.metadata.get("timestamp").and_then(Value::as_u64);
        start_time = start_time.or(time);
        if state.index < range.start {
            continue;
        }
        if state.index >= range.end {
            break;
        }
        if args.json {
            let payload_bytes = state.payload.as_ref().map(Vec::len);
            serde_json::to_writer(
                &mut *out,
                &json!({
                    "index": state.index,
                    "metadata": state.metadata,
                    "payload_bytes": payload_bytes,
                }),
            )?;
            writeln!(out)?;
        } else {
            write_state(out, &state, time, start_time)?;
        }
    }
    Ok(())
}

fn write_state(
    out: &mut impl Write,
    state: &State<Value>,
    time: Option<u64>,
    start_time: Option<u64>,
) -> Result<()> {
    let elapsed = match (time, start_time) {
        (Some(time), Some(start)) => {
            format!("+{:.3}s", time.saturating_sub(start) as f64 / 1e6)
        }
        _ => "?".to_string(),
    };
    let action = match state.metadata.get("action") {
        None | Some(Value::Null) => "(initial state)".to_string(),
        Some(action) => describe_action(action),
    };
    write!(out, "#{:<5} {elapsed:>10}  {action}", state.index)?;
    if let Some(payload) = &state.payload {
        write!(out, "  ({} payload)", format_bytes(payload.len()))?;
    }
    writeln!(out)?;

    let violations: Vec<&str> = state
        .metadata
        .get("violations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|violation| violation.get("name")?.as_str())
        .collect();
    if !violations.is_empty() {
        writeln!(out, "       violated: {}", violations.join(", "))?;
    }
    Ok(())
}

/// A serde-tagged action (`{"Name": {..fields}}`) as `Name {..fields}`.
fn describe_action(action: &Value) -> String {
    match action.as_object() {
        Some(object) if object.len() == 1 => {
            let (name, fields) = object.iter().next().expect("one entry");
            match fields {
                Value::Object(fields) if fields.is_empty() => name.clone(),
                fields => format!("{name} {fields}"),
            }
        }
        _ => action.to_string(),
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

impl TraceArgs {
    #[cfg(test)]
    fn new(trace_path: PathBuf) -> Self {
        TraceArgs {
            trace_path,
            state: None,
            json: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_trace::{TraceDriver, tag, write_envelope, write_record};

    fn write_trace(directory: &std::path::Path) {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        for (tag, payload) in [
            (tag::STATE, &br#"{"timestamp":1000000,"action":null,"violations":[]}"#[..]),
            (tag::STATE_PAYLOAD, &[0; 2048][..]),
            (
                tag::STATE,
                br#"{"timestamp":1500000,"action":{"TypeText":{"text":"hi"}},"violations":[{"name":"exitSuccess"}]}"#,
            ),
            (tag::STATE, br#"{"timestamp":2000000,"action":{"ScrollUp":{}},"violations":[]}"#),
        ] {
            write_record(&mut file, tag, &[payload]).unwrap();
        }
        std::fs::write(directory.join(TRACE_FILE_NAME), file).unwrap();
    }

    fn output(args: &TraceArgs) -> String {
        let mut out = Vec::new();
        show(args, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn shows_states() {
        let directory = tempfile::tempdir().unwrap();
        write_trace(directory.path());
        assert_eq!(
            output(&TraceArgs::new(directory.path().into())),
            "terminal trace\n\
             #0        +0.000s  (initial state)  (2.0 KB payload)\n\
             #1        +0.500s  TypeText {\"text\":\"hi\"}\n\
             \x20      violated: exitSuccess\n\
             #2        +1.000s  ScrollUp\n"
        );
    }

    #[test]
    fn shows_a_range_as_json() {
        let directory = tempfile::tempdir().unwrap();
        write_trace(directory.path());
        let args = TraceArgs {
            state: Some("1..".parse().unwrap()),
            json: true,
            ..TraceArgs::new(directory.path().join(TRACE_FILE_NAME))
        };
        let lines: Vec<Value> = output(&args)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["index"], 1);
        assert_eq!(lines[0]["metadata"]["action"]["TypeText"]["text"], "hi");
        assert_eq!(lines[1]["payload_bytes"], Value::Null);
    }

    #[test]
    fn points_json_traces_elsewhere() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("trace.jsonl"), "{}\n").unwrap();
        let mut out = Vec::new();
        let error = show(&TraceArgs::new(directory.path().into()), &mut out)
            .unwrap_err();
        assert!(error.to_string().contains("JSON trace"), "{error}");
    }

    #[test]
    fn parses_state_ranges() {
        let range =
            |text: &str| text.parse::<StateRange>().map(|range| range.0);
        assert_eq!(range("4"), Ok(4..5));
        assert_eq!(range("4..6"), Ok(4..6));
        assert_eq!(range("4.."), Ok(4..usize::MAX));
        assert_eq!(range("..6"), Ok(0..6));
        assert!(range("x").is_err());
    }
}
