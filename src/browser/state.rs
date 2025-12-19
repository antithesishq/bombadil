use anyhow::Context;
use chromiumoxide::{
    cdp::{
        browser_protocol::page::CaptureScreenshotFormat,
        js_protocol::debugger::CallFrameId,
    },
    page::ScreenshotParams,
    Page,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json as json;
use std::{io::Write, path::Path};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::browser::evaluation::{
    evaluate_expression_in_debugger, evaluate_function_call_in_debugger,
};

#[derive(Clone, Debug)]
pub struct BrowserState {
    page: Arc<Page>,
    call_frame_id: CallFrameId,

    pub url: String,
    pub title: String,
    pub content_type: String,
    pub console_entries: Vec<ConsoleEntry>,
    pub exception: Option<Exception>,

    #[allow(unused, reason = "we'll store this later")]
    screenshot_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Exception {
    UncaughtException(json::Value),
    UnhandledPromiseRejection(json::Value),
}

#[derive(Clone, Debug)]
pub struct ConsoleEntry {
    pub timestamp: SystemTime,
    pub level: ConsoleEntryLevel,
    pub args: Vec<json::Value>,
}

#[derive(Clone, Debug)]
pub enum ConsoleEntryLevel {
    Warning,
    Error,
}

impl BrowserState {
    pub(crate) async fn current(
        page: Arc<Page>,
        call_frame_id: &CallFrameId,
        console_entries: Vec<ConsoleEntry>,
        exception: Option<Exception>,
        screenshots_directory: &Path,
    ) -> anyhow::Result<Self> {
        let url: String = evaluate_expression_in_debugger(
            &page,
            call_frame_id,
            "window.location.href",
        )
        .await?;

        let title: String = evaluate_expression_in_debugger(
            &page,
            call_frame_id,
            "document.title",
        )
        .await?;

        let content_type: String = evaluate_expression_in_debugger(
            &page,
            call_frame_id,
            "document.contentType",
        )
        .await?;

        let screenshot_content = page
            .screenshot(
                ScreenshotParams::builder()
                    .omit_background(true)
                    .format(CaptureScreenshotFormat::Webp)
                    .build(),
            )
            .await
            .context("take screenshot")?;

        let screenshot_path = screenshots_directory.join(format!(
            "{}.webp",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros()
        ));
        let mut screenshot_file = std::fs::File::create(&screenshot_path)?;
        screenshot_file.write_all(&screenshot_content)?;

        Ok(BrowserState {
            page: page.clone(),
            call_frame_id: call_frame_id.clone(),
            url,
            title,
            content_type,
            console_entries,
            exception,
            screenshot_path,
        })
    }

    pub async fn evaluate_function_call<Output: DeserializeOwned>(
        &self,
        function_expression: impl Into<String>,
        arguments: Vec<json::Value>,
    ) -> anyhow::Result<Output> {
        evaluate_function_call_in_debugger(
            &self.page,
            &self.call_frame_id,
            function_expression,
            arguments,
        )
        .await
    }
}
