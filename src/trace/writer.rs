use std::{path::PathBuf, time::UNIX_EPOCH};

use anyhow::Result;
use serde_json as json;
use tokio::{fs::File, io::AsyncWriteExt};

use crate::{
    browser::{actions::BrowserAction, state::BrowserState},
    trace::{PropertyViolation, TraceEntry},
};

pub struct TraceWriter {
    screenshots_path: PathBuf,
    trace_file: File,
    last_transition_hash: Option<u64>,
}

impl TraceWriter {
    pub async fn initialize(root_path: PathBuf) -> Result<Self> {
        log::info!(
            "storing trace in {}",
            &root_path
                .to_str()
                .expect("states directory path is not valid unicode")
        );
        let screenshots_path = root_path.join("screenshots");
        tokio::fs::create_dir_all(&screenshots_path).await?;
        let trace_file = File::options()
            .append(true)
            .create(true)
            .open(root_path.join("trace.jsonl"))
            .await?;
        Ok(TraceWriter {
            screenshots_path,
            trace_file,
            last_transition_hash: None,
        })
    }
    pub async fn write(
        &mut self,
        last_action: Option<BrowserAction>,
        state: BrowserState,
        violations: Vec<PropertyViolation>,
    ) -> Result<()> {
        let screenshot_path = self.screenshots_path.join(format!(
            "{}.{}",
            state.timestamp.duration_since(UNIX_EPOCH)?.as_micros(),
            &state.screenshot.format.extension()
        ));
        File::create_new(&screenshot_path)
            .await?
            .write_all(&state.screenshot.data)
            .await?;

        let entry = TraceEntry {
            timestamp: state.timestamp,
            url: state.url,
            hash_previous: self.last_transition_hash,
            hash_current: state.transition_hash,
            action: last_action,
            screenshot: screenshot_path,
            violations,
        };

        self.last_transition_hash = state.transition_hash;

        self.trace_file
            .write(json::to_string(&entry)?.as_bytes())
            .await?;
        self.trace_file.write_u8(b'\n').await?;

        Ok(())
    }
}
