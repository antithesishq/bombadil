use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use cdp_protocol::cdp::browser_protocol::fetch;
use cdp_protocol::cdp::browser_protocol::io;
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
use crate::instrumentation::source_id::SourceId;
use crate::instrumentation::source_map::{
    self, Location, SourceMap, source_mapping_url,
};
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

    // A script waits for its source map on a thread of its own, so that
    // the other paused requests do not wait with it.
    let (source_map_jobs, source_map_jobs_rx) =
        mpmc::unbounded::<SourceMapJob>();
    let mut source_map_resolver = SourceMapResolver {
        connection: connection.clone(),
        session_id: session_id.clone(),
        frame_id: frame_id_main.clone(),
        allow_urls: allow_urls.clone(),
        blocks: blocks.clone(),
        source_maps: SourceMaps::new(SOURCE_MAPS_BYTES_MAX),
    };
    thread::spawn(move || {
        for job in source_map_jobs_rx {
            source_map_resolver.finish(job);
        }
    });

    let session_id = session_id.clone();
    let cleanup_connection = connection.clone();
    let cleanup_session_id = session_id.clone();
    let (error_tx, error_rx) = mpmc::bounded(1);
    let worker = thread::spawn(move || -> Result<()> {
        let continue_request =
            |event: &fetch::EventRequestPaused| -> Result<()> {
                continue_request(&connection, &session_id, event)
            };
        let intercept = |event: &fetch::EventRequestPaused| -> Result<
            Option<ScriptAwaitingMap>,
        > {
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
                    return continue_request(event).map(|()| None);
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
                return Ok(None);
            }

            if interception != Interception::Instrument {
                log::debug!(
                    "passing through request to a disallowed URL: {url}"
                );
                return continue_request(event).map(|()| None);
            }

            // Any non-200 upstream response is forwarded as-is.
            if let Some(status) = event.response_status_code
                && status != 200
            {
                return continue_request(event).map(|()| None);
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
                    // The map decides the block IDs, so the script is
                    // instrumented once its map is there.
                    if source_mapping_url(&body).is_some()
                        || source_map_header(event).is_some()
                    {
                        return Ok(Some(ScriptAwaitingMap {
                            url,
                            source_id,
                            text: body,
                            etag,
                        }));
                    }
                    let instrumented =
                        instrument_script(source_id, &body, None)?;
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

            fulfill_request(
                &connection,
                &session_id,
                event,
                body_instrumented,
                etag,
            )?;
            Ok(None)
        };

        while let Some(event) = events.next()? {
            match intercept(&event) {
                Ok(None) => {}
                Ok(Some(script)) => {
                    let job = SourceMapJob { event, script };
                    if let Err(mpmc::SendError(job)) = source_map_jobs.send(job)
                    {
                        finish_script(
                            &connection,
                            &session_id,
                            &blocks,
                            job,
                            None,
                        );
                    }
                }
                Err(error) => continue_after_failure(
                    &connection,
                    &session_id,
                    &event,
                    error,
                ),
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

fn continue_request(
    connection: &cdp::Connection,
    session_id: &SessionId,
    event: &fetch::EventRequestPaused,
) -> Result<()> {
    connection
        .send(
            fetch::ContinueRequestParams::builder()
                .request_id(event.request_id.clone())
                .build()
                .map_err(|error| {
                    anyhow!("failed building ContinueRequestParams: {}", error)
                })?,
            Some(session_id),
        )
        .map(|_| ())
        .context("failed continuing request")
}

fn continue_after_failure(
    connection: &cdp::Connection,
    session_id: &SessionId,
    event: &fetch::EventRequestPaused,
    error: anyhow::Error,
) {
    let error_debug = format!("{error:?}");
    if error_debug.contains("Invalid InterceptionId") {
        log::debug!(
            "interception invalidated (likely due to navigation): {}",
            event.request.url
        );
        return;
    }

    log::warn!(
        "failed handling request to {}, continuing it as is: {error:#}",
        event.request.url
    );
    if let Err(error) = continue_request(connection, session_id, event) {
        log::warn!(
            "failed continuing request after handling it failed: {error:#}"
        );
    }
}

fn fulfill_request(
    connection: &cdp::Connection,
    session_id: &SessionId,
    event: &fetch::EventRequestPaused,
    body_instrumented: String,
    etag: String,
) -> Result<()> {
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
                anyhow!("failed building FulfillRequestParams: {}", error)
            })?,
            Some(session_id),
        )
        .context("failed fulfilling request")?;
    log::debug!(
        "intercepted and instrumented request: {}",
        event.request.url
    );
    Ok(())
}

/// The `SourceMap` response header of a script, which names its map when
/// the script has no `sourceMappingURL` comment.
fn source_map_header(event: &fetch::EventRequestPaused) -> Option<&str> {
    let headers = event.response_headers.as_ref()?;
    let header = headers.iter().find(|header| {
        header.name.eq_ignore_ascii_case("sourcemap")
            || header.name.eq_ignore_ascii_case("x-sourcemap")
    })?;
    Some(&header.value)
}

fn instrument_script(
    source_id: SourceId,
    text: &str,
    source_map: Option<&SourceMap>,
) -> Result<instrumentation::Instrumented> {
    Ok(instrumentation::js::instrument_source_code(
        source_id,
        text,
        // As we can't know if the script is an ES module or a regular script,
        // we use this source type to let the parser decide.
        SourceType::unambiguous(),
        source_map,
    )?)
}

/// A script that refers to a source map and is not instrumented yet.
struct ScriptAwaitingMap {
    url: Url,
    source_id: SourceId,
    text: String,
    etag: String,
}

struct SourceMapJob {
    event: fetch::EventRequestPaused,
    script: ScriptAwaitingMap,
}

/// Instruments the script, registers its blocks and lets it through to the
/// page.
fn finish_script(
    connection: &cdp::Connection,
    session_id: &SessionId,
    blocks: &BlockRegistry,
    SourceMapJob { event, script }: SourceMapJob,
    source_map: Option<&SourceMap>,
) {
    let fulfilled =
        instrument_script(script.source_id, &script.text, source_map).and_then(
            |instrumented| {
                blocks.register(instrumented.blocks);
                fulfill_request(
                    connection,
                    session_id,
                    &event,
                    instrumented.text,
                    script.etag,
                )
            },
        );
    if let Err(error) = fulfilled {
        continue_after_failure(connection, session_id, &event, error);
    }
}

struct SourceMapResolver {
    connection: cdp::Connection,
    session_id: SessionId,
    frame_id: page::FrameId,
    allow_urls: Vec<MatchPattern>,
    blocks: BlockRegistry,
    /// `None` for a script whose map cannot be used.
    source_maps: SourceMaps<Option<SourceMap>>,
}

enum SourceMapError {
    /// The map did not load, and may load another time.
    Load(anyhow::Error),
    Unusable(anyhow::Error),
}

/// A dev server may serve a module under a new URL at every reload, each
/// with a small map of its own, and the map of one large bundle takes tens
/// of megabytes.
const SOURCE_MAPS_BYTES_MAX: usize = 64 << 20;

/// The maps of the scripts finished last, as many as fit in `bytes_max`, so
/// that a script that loads again unchanged gets the same block IDs without
/// its map being loaded again.
struct SourceMaps<M> {
    bytes_max: usize,
    bytes: usize,
    maps: HashMap<SourceId, CachedMap<M>>,
    puts_count: u64,
}

struct CachedMap<M> {
    etag: String,
    source_map: M,
    bytes: usize,
    /// `puts_count` when it was put.
    put: u64,
}

impl<M> SourceMaps<M> {
    fn new(bytes_max: usize) -> Self {
        SourceMaps {
            bytes_max,
            bytes: 0,
            maps: HashMap::new(),
            puts_count: 0,
        }
    }

    fn remove(&mut self, source_id: &SourceId) -> Option<CachedMap<M>> {
        let cached = self.maps.remove(source_id)?;
        self.bytes -= cached.bytes;
        Some(cached)
    }

    /// Takes out the script's ETag and map, if the map is that of the
    /// script with this ETag. A map of other content is dropped.
    fn take(
        &mut self,
        source_id: &SourceId,
        etag: &str,
    ) -> Option<(String, M)> {
        let cached = self.remove(source_id)?;
        (cached.etag == etag).then_some((cached.etag, cached.source_map))
    }

    /// Puts in the script's map, which takes `bytes`, and drops the maps
    /// put longest ago until the rest fit.
    fn put(
        &mut self,
        source_id: SourceId,
        etag: String,
        source_map: M,
        bytes: usize,
    ) {
        self.remove(&source_id);
        self.puts_count += 1;
        let cached = CachedMap {
            etag,
            source_map,
            bytes,
            put: self.puts_count,
        };
        self.maps.insert(source_id, cached);
        self.bytes += bytes;
        while self.bytes > self.bytes_max {
            let oldest = self
                .maps
                .iter()
                .min_by_key(|(_, cached)| cached.put)
                .map(|(source_id, _)| source_id.clone())
                .expect("bytes without maps");
            self.remove(&oldest);
        }
    }
}

impl SourceMapResolver {
    /// Finishes the script with its map, if that can be had.
    fn finish(&mut self, job: SourceMapJob) {
        let source_id = job.script.source_id.clone();
        let cached = self.source_maps.take(&source_id, &job.script.etag);
        // The ETag to keep the map under, or `None` for a map that failed
        // to load: the script's next load tries again.
        let (etag, source_map) = match cached {
            Some((etag, source_map)) => (Some(etag), source_map),
            None => match self.source_map(&job) {
                Ok(source_map) => {
                    (Some(job.script.etag.clone()), Some(source_map))
                }
                Err(SourceMapError::Unusable(error)) => {
                    log::debug!(
                        "no usable source map for {}: {error:#}",
                        job.script.url
                    );
                    (Some(job.script.etag.clone()), None)
                }
                Err(SourceMapError::Load(error)) => {
                    log::debug!(
                        "no source map for {} this time: {error:#}",
                        job.script.url
                    );
                    (None, None)
                }
            },
        };
        finish_script(
            &self.connection,
            &self.session_id,
            &self.blocks,
            job,
            source_map.as_ref(),
        );
        if let Some(etag) = etag {
            let bytes = source_id.0.len()
                + etag.len()
                + source_map.as_ref().map_or(0, SourceMap::size_bytes);
            self.source_maps.put(source_id, etag, source_map, bytes);
        }
    }

    fn source_map(
        &self,
        job: &SourceMapJob,
    ) -> Result<SourceMap, SourceMapError> {
        let script_url = &job.script.url;
        let location = source_mapping_url(&job.script.text)
            .or_else(|| source_map_header(&job.event))
            .context("the script names no map")
            .and_then(|map_url| source_map::locate(script_url, map_url))
            .map_err(SourceMapError::Unusable)?;
        let (json, map_url) = match location {
            Location::Inline(json) => (json, None),
            Location::Remote(url) => {
                if Interception::of(&self.allow_urls, &url, false)
                    != Interception::Instrument
                {
                    return Err(SourceMapError::Unusable(anyhow!(
                        "the map is at a disallowed URL: {url}"
                    )));
                }
                (self.load(&url)?, Some(url))
            }
        };
        SourceMap::parse(&json, Some(map_url.as_ref().unwrap_or(script_url)))
            .map_err(SourceMapError::Unusable)
    }

    /// Loads `url` as the page would, with its cookies and through the
    /// browser's proxy. A map the server says it does not have (404 or
    /// 410) is unusable; any other failure may not happen again.
    fn load(&self, url: &Url) -> Result<String, SourceMapError> {
        let resource = self
            .connection
            .send(
                network::LoadNetworkResourceParams {
                    frame_id: Some(self.frame_id.clone()),
                    url: url.to_string(),
                    options: network::LoadNetworkResourceOptions::new(
                        false, true,
                    ),
                },
                Some(&self.session_id),
            )
            .context("failed loading the map")
            .map_err(SourceMapError::Load)?
            .resource;
        let stream = match resource.stream {
            Some(stream) if resource.success => stream,
            _ => {
                let error = anyhow!(
                    "failed loading the map: {} (HTTP status {:?})",
                    resource.net_error_name.as_deref().unwrap_or("no error"),
                    resource.http_status_code,
                );
                let is_absent = resource
                    .http_status_code
                    .is_some_and(|status| status == 404.0 || status == 410.0);
                return Err(match is_absent {
                    true => SourceMapError::Unusable(error),
                    false => SourceMapError::Load(error),
                });
            }
        };
        let json = self
            .read(url, stream)
            .context("failed reading the map")
            .map_err(SourceMapError::Load)?;
        String::from_utf8(json)
            .context("the map is not UTF-8")
            .map_err(SourceMapError::Unusable)
    }

    fn read(&self, url: &Url, stream: io::StreamHandle) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let read = (|| -> Result<()> {
            loop {
                let chunk = self.connection.send(
                    io::ReadParams::new(stream.clone()),
                    Some(&self.session_id),
                )?;
                if chunk.base64_encoded == Some(true) {
                    BASE64_STANDARD.decode_vec(&chunk.data, &mut bytes)?;
                } else {
                    bytes.extend_from_slice(chunk.data.as_bytes());
                }
                if chunk.eof {
                    return Ok(());
                }
            }
        })();
        if let Err(error) = self
            .connection
            .post(io::CloseParams::new(stream), Some(&self.session_id))
        {
            log::debug!("failed closing the stream of {url}: {error:#}");
        }
        read?;
        Ok(bytes)
    }
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
        generators::{
            booleans, integers, optional, sampled_from, text, tuples3, vecs,
        },
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
    fn test_source_maps_are_those_of_the_scripts_finished_last(tc: TestCase) {
        let bytes_max = tc.draw(integers::<usize>().max_value(8));
        let scripts = tc.draw(vecs(tuples3(
            sampled_from(
                ["/a.js", "/b.js", "/c.js", "/d.js", "/e.js"].as_slice(),
            ),
            sampled_from(["1", "2"].as_slice()),
            integers::<usize>().max_value(4),
        )));
        let mut source_maps = SourceMaps::new(bytes_max);
        // Script, ETag, map and its bytes, the script finished last at the
        // end. A map is the number of the load that made it.
        let mut expected: Vec<(&str, &str, usize, usize)> = Vec::new();
        for (load, (script, etag, bytes)) in scripts.into_iter().enumerate() {
            let source_id = SourceId(script.into());
            let cached = expected
                .iter()
                .position(|(cached, _, _, _)| *cached == script)
                .map(|index| expected.remove(index))
                .filter(|(_, cached, _, _)| *cached == etag)
                .map(|(_, etag, map, _)| (etag.to_string(), map));
            let taken = source_maps.take(&source_id, etag);
            assert_eq!(taken, cached);

            let map = taken.map_or(load, |(_, map)| map);
            source_maps.put(source_id, etag.to_string(), map, bytes);
            expected.push((script, etag, map, bytes));
            let bytes_of = |maps: &[(&str, &str, usize, usize)]| -> usize {
                maps.iter().map(|(_, _, _, bytes)| bytes).sum()
            };
            let fitting = (0..=expected.len())
                .find(|&dropped| bytes_of(&expected[dropped..]) <= bytes_max)
                .unwrap();
            expected.drain(..fitting);
            assert_eq!(source_maps.maps.len(), expected.len());
            assert_eq!(source_maps.bytes, bytes_of(&expected));
        }
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
