use crate::instrumentation::{
    EDGES_CURRENT, EDGES_PREVIOUS, EDGE_MAP_SIZE, NAMESPACE,
};
use anyhow::{Context, Result};
use chromiumoxide::{
    cdp::{
        browser_protocol::page::{self, CaptureScreenshotFormat},
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
use url::Url;

use crate::browser::evaluation::{
    evaluate_expression_in_debugger, evaluate_function_call_in_debugger,
};

#[derive(Clone, Debug)]
pub struct BrowserState {
    page: Arc<Page>,
    call_frame_id: CallFrameId,

    pub url: Url,
    pub title: String,
    pub content_type: String,
    pub console_entries: Vec<ConsoleEntry>,
    pub navigation_history: NavigationHistory,
    pub exception: Option<Exception>,
    pub coverage: Coverage,

    #[allow(unused, reason = "we'll store this later")]
    screenshot_path: PathBuf,
}

pub type EdgeIndex = u32;
pub type EdgeBucket = u8;

#[derive(Clone, Debug)]
pub struct Coverage {
    pub edges_new: Vec<(EdgeIndex, EdgeBucket)>,
}

#[derive(Clone, Debug)]
pub struct NavigationHistory {
    pub back: Vec<NavigationEntry>,
    pub current: NavigationEntry,
    pub forward: Vec<NavigationEntry>,
}

#[derive(Clone, Debug)]
pub struct NavigationEntry {
    pub id: u32,
    pub title: String,
    pub url: Url,
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
    ) -> Result<Self> {
        let url = Url::parse(
            &evaluate_expression_in_debugger::<String>(
                &page,
                call_frame_id,
                "window.location.href",
            )
            .await?,
        )?;

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

        let navigation_history_result = page
            .execute(page::GetNavigationHistoryParams {})
            .await?
            .result;

        let navigation_entries = navigation_history_result
            .entries
            .iter()
            .map(|entry| NavigationEntry {
                id: entry.id as u32,
                title: entry.title.clone(),
                url: Url::parse(&entry.url)
                    .expect("url from getNavigationHistory doesn't parse"),
            })
            .collect::<Vec<_>>();
        let index = navigation_history_result.current_index as usize;
        let navigation_history = NavigationHistory {
            back: navigation_entries[0..index].to_vec(),
            current: navigation_entries[index].clone(),
            forward: navigation_entries[index..].to_vec(),
        };
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

        let edges_new: Vec<(u32, u8)> = evaluate_expression_in_debugger(
            &page,
            call_frame_id,
            format!("
                (() => {{
                    if (!window.{NAMESPACE}) return [];

                    // Bucket current hits into [1,8], similar to AFL.
                    function bucket(hits) {{
                        if (hits <= 3) return hits;
                        let msb = 0;
                        let n = hits;
                        while (n > 0) {{
                            n = n >> 1;
                            msb++;
                        }}
                        return Math.min(msb + 1, 8);
                    }}
                    for (let i = 0; i < window.{NAMESPACE}.{EDGES_CURRENT}.length; i++) {{
                        window.{NAMESPACE}.{EDGES_CURRENT}[i] = bucket(window.{NAMESPACE}.{EDGES_CURRENT}[i]);
                    }}

                    // Compute differences.
                    const differences = [];
                    for (let i = 0; i < window.{NAMESPACE}.{EDGES_CURRENT}.length; i++) {{
                        if (window.{NAMESPACE}.{EDGES_CURRENT}[i] !== window.{NAMESPACE}.{EDGES_PREVIOUS}[i]) {{
                            differences.push([i, window.{NAMESPACE}.{EDGES_CURRENT}[i]]);
                        }}
                    }}

                    // Shift the arrays.
                    window.{NAMESPACE}.{EDGES_PREVIOUS} = window.{NAMESPACE}.{EDGES_CURRENT};
                    window.{NAMESPACE}.{EDGES_CURRENT} = new Uint8Array({EDGE_MAP_SIZE});

                    return differences;
                }})()
                "
            ),
        )
        .await?;

        Ok(BrowserState {
            page: page.clone(),
            call_frame_id: call_frame_id.clone(),
            url,
            title,
            content_type,
            console_entries,
            navigation_history,
            exception,
            coverage: Coverage { edges_new },
            screenshot_path,
        })
    }

    pub async fn evaluate_function_call<Output: DeserializeOwned>(
        &self,
        function_expression: impl Into<String>,
        arguments: Vec<json::Value>,
    ) -> Result<Output> {
        evaluate_function_call_in_debugger(
            &self.page,
            &self.call_frame_id,
            function_expression,
            arguments,
        )
        .await
    }
}
