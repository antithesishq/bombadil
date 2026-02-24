use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::fetch;
use chromiumoxide::cdp::browser_protocol::network;
use futures::StreamExt;
use log;
use oxc::span::SourceType;
use serde_json as json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::spawn;

use crate::instrumentation;
use crate::instrumentation::source_id::SourceId;

pub async fn instrument_js_coverage(page: Arc<Page>) -> Result<()> {
    page.execute(
        fetch::EnableParams::builder()
            .pattern(
                fetch::RequestPattern::builder()
                    .request_stage(fetch::RequestStage::Response)
                    .resource_type(network::ResourceType::Script)
                    .build(),
            )
            .pattern(
                fetch::RequestPattern::builder()
                    .request_stage(fetch::RequestStage::Response)
                    .resource_type(network::ResourceType::Document)
                    .build(),
            )
            .build(),
    )
    .await
    .context("failed enabling request interception")?;

    let mut events = page.event_listener::<fetch::EventRequestPaused>().await?;

    let _handle = spawn(async move {
        let intercept =
            async |event: &fetch::EventRequestPaused| -> Result<()> {
                // Any non-200 upstream response is forwarded as-is.
                if let Some(status) = event.response_status_code
                    && status != 200
                {
                    return page
                        .execute(
                            fetch::ContinueRequestParams::builder()
                                .request_id(event.request_id.clone())
                                .build()
                                .map_err(|error| {
                                    anyhow!(
                                    "failed building ContinueRequestParams: {}",
                                    error
                                )
                                })?,
                        )
                        .await
                        .map(|_| ())
                        .context("failed continuing request");
                }

                let headers: HashMap<String, String> =
                    json::from_value(event.request.headers.inner().clone())?;

                let body_response = page
                    .execute(
                        fetch::GetResponseBodyParams::builder()
                            .request_id(event.request_id.clone())
                            .build()
                            .map_err(|error| {
                                anyhow!(
                                    "failed building GetResponseBodyParams: {}",
                                    error
                                )
                            })?,
                    )
                    .await
                    .context("failed getting response body")?;

                let body = if body_response.base64_encoded {
                    let bytes = body_response.body.as_bytes();
                    String::from_utf8(BASE64_STANDARD.decode(bytes)?)?
                } else {
                    body_response.body.clone()
                };

                let source_id = source_id(headers, &body);

                let is_html_document = event.resource_type
                    == network::ResourceType::Document
                    && event
                        .response_headers
                        .as_ref()
                        .and_then(|headers| {
                            headers.iter().find(|h| {
                                h.name.eq_ignore_ascii_case("content-type")
                            })
                        })
                        .map(|h| h.value.starts_with("text/html"))
                        .unwrap_or_else(|| {
                            !body.trim_start().starts_with("<?xml")
                        });

                let body_instrumented = if event.resource_type
                    == network::ResourceType::Script
                {
                    let instrumented =
                        instrumentation::js::instrument_source_code(
                            source_id,
                            &body,
                            // As we can't know if the script is an ES module or a regular script,
                            // we use this source type to let the parser decide.
                            SourceType::unambiguous(),
                        )?;

                    // Write to /tmp/ for debugging
                    if let Some(filename) =
                        event.request.url.split('/').next_back()
                    {
                        let safe_filename =
                            filename.replace(['?', '#', '&', '='], "_");
                        let path = format!("/tmp/{}", safe_filename);
                        if let Err(e) =
                            tokio::fs::write(&path, &instrumented).await
                        {
                            log::debug!(
                                "failed to write debug file to {}: {}",
                                path,
                                e
                            );
                        } else {
                            log::debug!(
                                "wrote instrumented script to {}",
                                path
                            );
                        }
                    }

                    instrumented
                } else if is_html_document {
                    instrumentation::html::instrument_inline_scripts(
                        source_id, &body,
                    )?
                } else if event.resource_type == network::ResourceType::Document
                {
                    // Non-HTML documents (XML, PDF, etc.) are passed
                    // through without instrumentation.
                    body.clone()
                } else {
                    bail!(
                        "should only intercept script and document resources, but got {:?}",
                        event.resource_type
                    );
                };

                page.execute(
                    fetch::FulfillRequestParams::builder()
                        .request_id(event.request_id.clone())
                        .body(BASE64_STANDARD.encode(body_instrumented))
                        .response_code(200)
                        .response_header(fetch::HeaderEntry {
                            name: "etag".to_string(),
                            value: format!("{}", source_id.0),
                        })
                        // TODO: forward headers
                        .build()
                        .map_err(|error| {
                            anyhow!(
                                "failed building FulfillRequestParams: {}",
                                error
                            )
                        })?,
                )
                .await
                .context("failed fulfilling request")?;
                log::debug!(
                    "intercepted and instrumented request: {}",
                    event.request.url
                );
                Ok(())
            };
        while let Some(event) = events.next().await {
            if let Err(error) = intercept(&event).await {
                let error_debug = format!("{error:?}");
                if error_debug.contains("Invalid InterceptionId") {
                    log::debug!(
                        "interception invalidated (likely due to navigation): {}",
                        event.request.url
                    );
                    continue;
                }

                log::warn!("failed to instrument requested script: {error}");
                if let Err(error) = async {
                    let params = fetch::ContinueRequestParams::builder()
                        .request_id(event.request_id.clone())
                        .build()
                        .map_err(|error| anyhow!("{error}"))?;
                    page.execute(params)
                        .await
                        .map(|_| ())
                        .map_err(|error| anyhow!("{error}"))
                }
                .await
                {
                    log::warn!(
                        "failed continuing request after instrumentation failed: {error}"
                    );
                }
            }
        }
    });

    Ok(())
}

/// Calculate source ID from etag or body.
fn source_id(headers: HashMap<String, String>, body: &str) -> SourceId {
    if let Some(etag) = headers.get("etag") {
        SourceId::hash(etag)
    } else {
        SourceId::hash(body)
    }
}
