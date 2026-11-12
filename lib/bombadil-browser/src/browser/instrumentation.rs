use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use cdp_protocol::cdp::browser_protocol::fetch;
use cdp_protocol::cdp::browser_protocol::network;
use cdp_protocol::cdp::browser_protocol::page;
use cdp_protocol::cdp::browser_protocol::target::SessionId;
use crossbeam_channel as mpmc;
use log;
use oxc::span::SourceType;
use serde_json as json;
use std::collections::HashMap;
use std::thread;
use url::Url;

use crate::instrumentation;
use crate::instrumentation::BlockRegistry;
use crate::instrumentation::InstrumentationConfig;
use crate::match_pattern::MatchPattern;

pub fn instrument_js_coverage(
    connection: cdp::Connection,
    session_id: &SessionId,
    frame_id_main: page::FrameId,
    config: InstrumentationConfig,
    allow_urls: Vec<MatchPattern>,
    blocks: BlockRegistry,
    origin: Url,
) -> Result<mpmc::Receiver<anyhow::Error>> {
    connection
        .send(
            fetch::EnableParams::builder()
                .pattern(
                    fetch::RequestPattern::builder()
                        .request_stage(fetch::RequestStage::Response)
                        .resource_type(network::ResourceType::Script)
                        .build(),
                )
                .pattern(
                    fetch::RequestPattern::builder()
                        .request_stage(fetch::RequestStage::Request)
                        .resource_type(network::ResourceType::Document)
                        .build(),
                )
                .pattern(
                    fetch::RequestPattern::builder()
                        .request_stage(fetch::RequestStage::Response)
                        .resource_type(network::ResourceType::Document)
                        .build(),
                )
                .build(),
            Some(session_id),
        )
        .context("failed enabling request interception")?;

    // Requests answered by a service worker never reach Fetch interception,
    // so the page would run uninstrumented scripts.
    if config.instrument_files || config.instrument_inline {
        connection
            .send(
                network::SetBypassServiceWorkerParams::new(true),
                Some(session_id),
            )
            .context("failed bypassing service workers")?;
    }

    let events = connection.events.subscribe::<fetch::EventRequestPaused>();

    let session_id = session_id.clone();
    let cleanup_connection = connection.clone();
    let cleanup_session_id = session_id.clone();
    let (error_tx, error_rx) = mpmc::bounded(1);
    let worker = thread::spawn(move || -> Result<()> {
        let continue_request =
            |event: &fetch::EventRequestPaused| -> Result<()> {
                connection
                    .send(
                        fetch::ContinueRequestParams::builder()
                            .request_id(event.request_id.clone())
                            .build()
                            .map_err(|error| {
                                anyhow!(
                                    "failed building ContinueRequestParams: {}",
                                    error
                                )
                            })?,
                        Some(&session_id),
                    )
                    .map(|_| ())
                    .context("failed continuing request")
            };
        let intercept = |event: &fetch::EventRequestPaused| -> Result<()> {
            let url = Url::parse(&event.request.url)
                .context("request URL does not parse")?;
            let is_document =
                event.resource_type == network::ResourceType::Document;
            let interception = Interception::of(
                &allow_urls,
                &url,
                is_document && event.frame_id == frame_id_main,
            );

            // Only documents are paused before they are requested.
            if event.response_status_code.is_none()
                && event.response_error_reason.is_none()
            {
                if interception != Interception::Block {
                    return continue_request(event);
                }
                log::info!("blocked navigation to disallowed URL: {url}");
                // The page shows the browser's error page and the test
                // strategy navigates back. A request that cannot be failed
                // stays paused rather than being let through.
                if let Err(error) = connection.send(
                    fetch::FailRequestParams::new(
                        event.request_id.clone(),
                        network::ErrorReason::BlockedByClient,
                    ),
                    Some(&session_id),
                ) {
                    log::warn!(
                        "failed blocking navigation to {url}: {error:#}"
                    );
                }
                return Ok(());
            }

            if interception != Interception::Instrument {
                log::debug!(
                    "passing through request to a disallowed URL: {url}"
                );
                return continue_request(event);
            }

            // Any non-200 upstream response is forwarded as-is.
            if let Some(status) = event.response_status_code
                && status != 200
            {
                return continue_request(event);
            }

            let headers: HashMap<String, String> =
                json::from_value(event.request.headers.inner().clone())?;

            let body_response = connection
                .send(
                    fetch::GetResponseBodyParams::builder()
                        .request_id(event.request_id.clone())
                        .build()
                        .map_err(|error| {
                            anyhow!(
                                "failed building GetResponseBodyParams: {}",
                                error
                            )
                        })?,
                    Some(&session_id),
                )
                .context("failed getting response body")?;

            let body = if body_response.base64_encoded {
                let bytes = body_response.body.as_bytes();
                String::from_utf8(BASE64_STANDARD.decode(bytes)?)?
            } else {
                body_response.body.clone()
            };

            let source_id = blocks.source_id(&url, &origin);
            let etag = content_etag(headers, &body);

            let is_html_document = is_document
                && event
                    .response_headers
                    .as_ref()
                    .and_then(|headers| {
                        headers.iter().find(|h| {
                            h.name.eq_ignore_ascii_case("content-type")
                        })
                    })
                    .map(|h| h.value.starts_with("text/html"))
                    .unwrap_or_else(|| !body.trim_start().starts_with("<?xml"));

            let body_instrumented = if event.resource_type
                == network::ResourceType::Script
            {
                if !config.instrument_files {
                    log::debug!(
                        "skipping script file (disabled): {}",
                        event.request.url
                    );
                    body.clone()
                } else {
                    let instrumented =
                        instrumentation::js::instrument_source_code(
                            source_id,
                            &body,
                            // As we can't know if the script is an ES module or a regular script,
                            // we use this source type to let the parser decide.
                            SourceType::unambiguous(),
                        )?;
                    blocks.register(instrumented.blocks);
                    instrumented.text
                }
            } else if is_html_document {
                if config.instrument_inline {
                    let instrumented =
                        instrumentation::html::instrument_inline_scripts(
                            source_id, &body,
                        )?;
                    blocks.register(instrumented.blocks);
                    instrumented.text
                } else {
                    log::debug!("skipping inline scripts (disabled)");
                    body.clone()
                }
            } else if is_document {
                // Non-HTML documents (XML, PDF, etc.) are passed
                // through without instrumentation.
                body.clone()
            } else {
                bail!(
                    "should only intercept script and document resources, but got {:?}",
                    event.resource_type
                );
            };

            // Exclude headers that are invalidated by instrumentation
            // or connection-specific (hop-by-hop headers per HTTP
            // specification)
            let excluded_headers = [
                "content-length",
                "content-encoding",
                "transfer-encoding",
                "content-md5",
                "digest",
                "content-range",
                "connection",
                "keep-alive",
                "proxy-authenticate",
                "proxy-authorization",
                "te",
                "trailer",
                "upgrade",
                "etag",
            ];

            let mut builder = fetch::FulfillRequestParams::builder()
                .request_id(event.request_id.clone())
                .body(BASE64_STANDARD.encode(body_instrumented))
                .response_code(200)
                .response_header(fetch::HeaderEntry {
                    name: "etag".to_string(),
                    value: etag,
                });

            if let Some(headers) = &event.response_headers {
                for header in headers {
                    let name_lower = header.name.to_lowercase();
                    if !excluded_headers.contains(&name_lower.as_str()) {
                        builder = builder.response_header(fetch::HeaderEntry {
                            name: header.name.clone(),
                            value: header.value.clone(),
                        });
                    }
                }
            }

            connection
                .send(
                    builder.build().map_err(|error| {
                        anyhow!(
                            "failed building FulfillRequestParams: {}",
                            error
                        )
                    })?,
                    Some(&session_id),
                )
                .context("failed fulfilling request")?;
            log::debug!(
                "intercepted and instrumented request: {}",
                event.request.url
            );
            Ok(())
        };

        while let Some(event) = events.next()? {
            if let Err(error) = intercept(&event) {
                let error_debug = format!("{error:?}");
                if error_debug.contains("Invalid InterceptionId") {
                    log::debug!(
                        "interception invalidated (likely due to navigation): {}",
                        event.request.url
                    );
                    continue;
                }

                log::warn!(
                    "failed handling request to {}, continuing it as is: {error:#}",
                    event.request.url
                );
                if let Err(error) = continue_request(&event) {
                    log::warn!(
                        "failed continuing request after handling it failed: {error:#}"
                    );
                }
            }
        }

        Ok(())
    });

    thread::spawn(move || {
        let error = match worker.join() {
            Ok(Ok(())) => return,
            Ok(Err(error)) => format!("{error:#}"),
            Err(_) => "worker panicked".to_string(),
        };
        log::error!("Fetch instrumentation worker failed: {error}");
        let cleanup_error = cleanup_connection
            .send(fetch::DisableParams::default(), Some(&cleanup_session_id))
            .err()
            .map(|error| {
                log::error!(
                    "disabling Fetch after worker failure failed: {error:#}"
                );
                format!("; disabling Fetch also failed: {error:#}")
            })
            .unwrap_or_default();
        let _ = error_tx.send(anyhow!(
            "Fetch instrumentation worker failed: {error}{cleanup_error}"
        ));
    });

    Ok(error_rx)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interception {
    Instrument,
    PassThrough,
    Block,
}

impl Interception {
    fn of(
        allow_urls: &[MatchPattern],
        url: &Url,
        is_document_main: bool,
    ) -> Self {
        // A URL no pattern can describe (blob:, data:) is made by a page
        // that was itself let through.
        if !MatchPattern::covers_scheme(url)
            || allow_urls.iter().any(|pattern| pattern.matches(url))
        {
            Interception::Instrument
        } else if is_document_main {
            Interception::Block
        } else if url.scheme() == "file" {
            // The default boundary of a file origin is that one file, but the
            // scripts and frames it loads from disk are still its own.
            Interception::Instrument
        } else {
            Interception::PassThrough
        }
    }
}

/// A hash of the original ETag, or of the body when there is none, so the
/// browser revalidates when content changes.
fn content_etag(headers: HashMap<String, String>, body: &str) -> String {
    let content = headers.get("etag").map_or(body, String::as_str);
    format!("{}", bombadil::hash::fnv1a64(content.bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::{
        Generator, TestCase,
        generators::{booleans, integers, optional, sampled_from, text, vecs},
    };

    const HOSTS_UNDER_TEST: &[&str] = &["app.test", "api.app.test"];
    const HOSTS_ELSEWHERE: &[&str] =
        &["cdn.example", "app.test.example", "myapp.test", "test"];

    #[hegel::composite]
    fn paths(tc: &TestCase) -> String {
        tc.draw_silent(text().alphabet("ab/.?=").max_size(6))
    }

    #[hegel::composite]
    fn http_urls(tc: &TestCase, hosts: &'static [&'static str]) -> Url {
        let scheme = tc.draw_silent(sampled_from(["http", "https"].as_slice()));
        let host = tc.draw_silent(sampled_from(hosts));
        let port = tc
            .draw_silent(optional(integers::<u16>().min_value(1024)))
            .map_or_else(String::new, |port| format!(":{port}"));
        let path = tc.draw_silent(paths());
        Url::parse(&format!("{scheme}://{host}{port}/{path}")).unwrap()
    }

    #[hegel::composite]
    fn file_urls(tc: &TestCase, directory: &'static str) -> Url {
        let path = tc.draw_silent(paths());
        Url::parse(&format!("file:///{directory}/{path}")).unwrap()
    }

    /// Patterns that match nothing but `HOSTS_UNDER_TEST` and files in `/app`.
    #[hegel::composite]
    fn allow_urls(tc: &TestCase) -> Vec<MatchPattern> {
        let raw = sampled_from(
            [
                "*://app.test/*",
                "https://*.app.test/*",
                "http://api.app.test:*/a*",
                "file:///app/*",
            ]
            .as_slice(),
        );
        let mut patterns: Vec<MatchPattern> = tc
            .draw_silent(vecs(raw).max_size(3))
            .into_iter()
            .map(|raw| MatchPattern::parse(raw).unwrap())
            .collect();
        for origin in
            tc.draw_silent(vecs(http_urls(HOSTS_UNDER_TEST)).max_size(2))
        {
            patterns.push(MatchPattern::from_origin(&origin).unwrap());
        }
        patterns
    }

    #[hegel::test]
    fn test_url_under_test_is_instrumented_wherever_it_loads(tc: TestCase) {
        let url = if tc.draw(booleans()) {
            tc.draw(http_urls(HOSTS_UNDER_TEST).print_as_debug())
        } else {
            tc.draw(file_urls("app").print_as_debug())
        };
        let mut patterns = tc.draw(allow_urls().print_as_debug());
        let index = tc.draw(integers::<usize>().max_value(patterns.len()));
        patterns.insert(index, MatchPattern::from_origin(&url).unwrap());
        let is_document_main = tc.draw(booleans());
        assert_eq!(
            Interception::of(&patterns, &url, is_document_main),
            Interception::Instrument
        );
    }

    #[hegel::test]
    fn test_http_url_elsewhere_is_blocked_in_the_main_frame_only(tc: TestCase) {
        let url = tc.draw(http_urls(HOSTS_ELSEWHERE).print_as_debug());
        let patterns = tc.draw(allow_urls().print_as_debug());
        assert_eq!(
            Interception::of(&patterns, &url, true),
            Interception::Block
        );
        assert_eq!(
            Interception::of(&patterns, &url, false),
            Interception::PassThrough
        );
    }

    #[hegel::test]
    fn test_file_elsewhere_is_blocked_in_the_main_frame_only(tc: TestCase) {
        let url = tc.draw(file_urls("elsewhere").print_as_debug());
        let patterns = tc.draw(allow_urls().print_as_debug());
        assert_eq!(
            Interception::of(&patterns, &url, true),
            Interception::Block
        );
        assert_eq!(
            Interception::of(&patterns, &url, false),
            Interception::Instrument
        );
    }

    #[hegel::test]
    fn test_url_no_pattern_describes_is_never_blocked(tc: TestCase) {
        let url = tc.draw(sampled_from(
            [
                "blob:https://cdn.example/0b5e",
                "data:text/javascript,1",
                "about:srcdoc",
                "chrome-error://chromewebdata/",
            ]
            .as_slice(),
        ));
        let patterns = tc.draw(allow_urls().print_as_debug());
        let is_document_main = tc.draw(booleans());
        assert_eq!(
            Interception::of(
                &patterns,
                &Url::parse(url).unwrap(),
                is_document_main
            ),
            Interception::Instrument
        );
    }
}
