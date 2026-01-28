use anyhow::{anyhow, bail, Context, Result};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use chromiumoxide::cdp::browser_protocol::fetch;
use chromiumoxide::cdp::browser_protocol::network;
use chromiumoxide::Page;
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

    let _ = spawn(async move {
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

                let body_instrumented = if event.resource_type
                    == network::ResourceType::Script
                {
                    instrumentation::js::instrument_source_code(
                        source_id,
                        &body,
                        // As we can't know if the script is an ES module or a regular script,
                        // we use this source type to let the parser decide.
                        SourceType::unambiguous(),
                    )?
                } else if event.resource_type == network::ResourceType::Document
                {
                    instrumentation::html::instrument_inline_scripts(
                        source_id, &body,
                    )?
                } else {
                    bail!(
                        "should only intercept script and document resources, but got {:?}", event.resource_type
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
                log::error!("failed to instrument requested script: {error}");
                page.execute(
                    fetch::ContinueRequestParams::builder()
                        .request_id(event.request_id.clone())
                        .build()
                        .map_err(|error| {
                            anyhow!(
                                "failed building ContinueRequestParams: {}",
                                error
                            )
                        })
                        .expect("failed to build continue request params"),
                )
                .await
                .map(|_| ())
                .expect(
                    "failed continuing request after instrumentation failed",
                );
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
