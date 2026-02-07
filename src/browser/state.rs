use crate::instrumentation::js::{
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
use std::{sync::Arc, time::SystemTime};
use url::Url;

use crate::browser::evaluation::{
    evaluate_expression_in_debugger, evaluate_function_call_in_debugger,
};

#[derive(Clone, Debug)]
pub struct BrowserState {
    page: Arc<Page>,
    call_frame_id: CallFrameId,

    pub timestamp: SystemTime,
    pub url: Url,
    pub title: String,
    pub content_type: String,
    pub console_entries: Vec<ConsoleEntry>,
    pub navigation_history: NavigationHistory,
    pub exception: Option<Exception>,
    pub transition_hash: Option<u64>,
    pub coverage: Coverage,
    pub screenshot: Screenshot,
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

#[derive(Copy, Clone, Debug)]
pub enum ScreenshotFormat {
    Webp,
    Png,
    Jpeg,
}

impl ScreenshotFormat {
    pub fn extension(&self) -> &str {
        match self {
            ScreenshotFormat::Webp => "webp",
            ScreenshotFormat::Png => "png",
            ScreenshotFormat::Jpeg => "jpeg",
        }
    }
}

impl From<ScreenshotFormat> for CaptureScreenshotFormat {
    fn from(val: ScreenshotFormat) -> Self {
        match val {
            ScreenshotFormat::Webp => CaptureScreenshotFormat::Webp,
            ScreenshotFormat::Png => CaptureScreenshotFormat::Png,
            ScreenshotFormat::Jpeg => CaptureScreenshotFormat::Jpeg,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Screenshot {
    pub format: ScreenshotFormat,
    pub data: Vec<u8>,
}

impl BrowserState {
    pub(crate) async fn current(
        page: Arc<Page>,
        call_frame_id: &CallFrameId,
        console_entries: Vec<ConsoleEntry>,
        exception: Option<Exception>,
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
        let format = ScreenshotFormat::Webp;
        let screenshot = Screenshot {
            data: page
                .screenshot(
                    ScreenshotParams::builder()
                        .omit_background(true)
                        .format(format)
                        .build(),
                )
                .await
                .context("take screenshot")?,
            format,
        };

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

        let transition_hash_bigint: Option<String> =
            evaluate_expression_in_debugger(
                &page,
                call_frame_id,
                format!(
                    "
                (() => {{
                    if (!window.{NAMESPACE}) return null;

                    const SIMHASH_BITS = 64;
                    function hash64(x) {{
                        let h = BigInt(x) + 0x9e3779b97f4a7c15n;
                        h = (h ^ (h >> 30n)) * 0xbf58476d1ce4e5b9n;
                        h = (h ^ (h >> 27n)) * 0x94d049bb133111ebn;
                        return h ^ (h >> 31n);
                    }}

                    const acc = new Int32Array(SIMHASH_BITS);

                    for (let i = 0; i < {EDGE_MAP_SIZE}; i++) {{
                        const bucket = window.{NAMESPACE}.{EDGES_PREVIOUS}[i];
                        if (bucket === 0) continue;

                        const weight = Math.max(1, Math.min(3, Math.floor(Math.log2(bucket))));
                        // const weight = bucket > 0 ? 1 : 0; // presence only
                        let h = hash64(i);

                        for (let b = 0; b < SIMHASH_BITS; b++) {{
                            const bit = (h >> BigInt(b)) & 1n;
                            acc[b] += bit === 1n ? weight : -weight;
                        }}
                    }}

                    if (acc.every(b => b == 0)) return null;

                    let out = 0n;
                    for (let b = 0; b < SIMHASH_BITS; b++) {{
                        if (acc[b] > 0) {{
                            out |= 1n << BigInt(b);
                        }}
                    }}

                    window.{NAMESPACE}.{EDGES_CURRENT}.fill(0);
                    return out;
                }})()
            "
                ),
            )
            .await?;

        let transition_hash = match transition_hash_bigint {
            Some(string) => Some(string.parse::<u64>()?),
            None => None,
        };

        Ok(BrowserState {
            timestamp: SystemTime::now(),
            page: page.clone(),
            call_frame_id: call_frame_id.clone(),
            url,
            title,
            content_type,
            console_entries,
            navigation_history,
            exception,
            coverage: Coverage { edges_new },
            transition_hash,
            screenshot,
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
