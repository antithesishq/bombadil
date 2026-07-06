use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Result, anyhow};
use bombadil::specification::convert::ToInternal;
use bombadil_schema::TraceEntry;
use serde::de::DeserializeOwned;

/// Load the actions of a previous run from a trace file (file path or
/// directory containing `trace.jsonl`), for reproduction.
pub fn load_reproduce_actions<Action, Internal, State>(
    path: &Path,
) -> Result<VecDeque<Internal>>
where
    Action: DeserializeOwned + ToInternal<Internal>,
    State: DeserializeOwned,
{
    let trace_file_path = if path.is_dir() {
        path.join("trace.jsonl")
    } else {
        path.to_path_buf()
    };
    let file = File::open(&trace_file_path).map_err(|error| {
        anyhow!(
            "failed to open trace file {}: {}",
            trace_file_path.display(),
            error
        )
    })?;
    let mut actions = VecDeque::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let entry: TraceEntry<Action, State> = serde_json::from_str(&line)?;
        if let Some(action) = entry.action {
            actions.push_back(action.to_internal());
        }
    }
    Ok(actions)
}
