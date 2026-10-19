use std::{
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use bombadil_schema::browser::BrowserTraceEntry;
use include_dir::{Dir, include_dir};

use crate::browser_trace::{self, BrowserTraceFile};

static INSPECT_ASSETS: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/../../target/inspect");

/// Where each state's screenshot is in a binary trace (offset and length).
type ScreenshotLocations = Arc<Mutex<Vec<Option<(u64, usize)>>>>;

#[derive(Clone)]
struct AppState {
    trace: Arc<BrowserTraceFile>,
    /// For a binary trace: where each state's screenshot is in the file
    /// (offset and length), filled in when the trace is served.
    screenshots: ScreenshotLocations,
}

pub async fn serve(
    trace_path: PathBuf,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    // Fail before serving rather than when the page asks for the trace.
    let trace = browser_trace::resolve(&trace_path)?;

    let state = AppState {
        trace: Arc::new(trace),
        screenshots: Arc::default(),
    };

    let app = Router::new()
        .route("/api/trace", get(trace_handler))
        .route("/api/screenshots/{filename}", get(screenshot_handler))
        .route("/", get(serve_index))
        .fallback(serve_assets)
        .with_state(state);

    let address = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let url = format!("http://{}", address);

    println!("Bombadil Inspect available at {}", url);

    if open_browser && let Err(error) = open::that(&url) {
        log::warn!("Failed to open browser: {}", error);
    }

    axum::serve(listener, app).await?;
    Ok(())
}

async fn trace_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<BrowserTraceEntry>>, StatusCode> {
    // TODO: this should be streamed over a websocket or SSE rather than
    // loaded into memory and sent as JSON. OK for now, but worth
    // revisiting when we do "live inspect mode".
    tokio::task::spawn_blocking(move || read_entries(&state))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .map_err(|error| {
            log::error!("Failed to read trace: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// The trace's entries, with each screenshot pointing at the screenshot
/// endpoint.
fn read_entries(state: &AppState) -> Result<Vec<BrowserTraceEntry>> {
    match state.trace.as_ref() {
        BrowserTraceFile::Json(file) => {
            let mut entries = browser_trace::read_json(file)?;
            for entry in &mut entries {
                let filename = std::path::Path::new(&entry.state.screenshot)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
                    .to_string();
                entry.state.screenshot = format!("/api/screenshots/{filename}");
            }
            Ok(entries)
        }
        BrowserTraceFile::Binary(file) => {
            let mut entries = Vec::new();
            let mut screenshots = Vec::new();
            for trace_state in browser_trace::open_binary(file)? {
                let trace_state = trace_state?;
                let mut entry = trace_state.metadata;
                let location = trace_state
                    .payload_offset
                    .zip(trace_state.payload.as_ref().map(Vec::len));
                screenshots.push(location);
                // The entry holds the screenshot's format; name it by the
                // state's index.
                entry.state.screenshot = format!(
                    "/api/screenshots/{}.{}",
                    trace_state.index, entry.state.screenshot
                );
                entries.push(entry);
            }
            *state.screenshots.lock().expect("screenshots lock") = screenshots;
            Ok(entries)
        }
    }
}

async fn screenshot_handler(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> impl IntoResponse {
    let sanitized = std::path::Path::new(&filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();

    let data = match state.trace.as_ref() {
        BrowserTraceFile::Json(file) => {
            let directory = file.parent().unwrap_or(std::path::Path::new("."));
            tokio::fs::read(directory.join("screenshots").join(&sanitized))
                .await
                .ok()
        }
        BrowserTraceFile::Binary(file) => {
            let location = sanitized
                .split_once('.')
                .and_then(|(index, _)| index.parse::<usize>().ok())
                .and_then(|index| {
                    let screenshots =
                        state.screenshots.lock().expect("screenshots lock");
                    screenshots.get(index).copied().flatten()
                });
            match location {
                Some((offset, length)) => {
                    let file = file.clone();
                    tokio::task::spawn_blocking(move || {
                        read_at(&file, offset, length)
                    })
                    .await
                    .ok()
                    .and_then(Result::ok)
                }
                None => None,
            }
        }
    };

    match data {
        Some(data) => {
            let mime =
                mime_guess::from_path(&sanitized).first_or_octet_stream();
            (
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        mime.as_ref().to_string(),
                    ),
                ],
                data,
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Reads `length` bytes at `offset`, e.g. one screenshot of a binary trace.
fn read_at(
    file: &std::path::Path,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(file)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut data = vec![0; length];
    file.read_exact(&mut data)?;
    Ok(data)
}

async fn serve_index() -> axum::response::Html<&'static str> {
    let html = INSPECT_ASSETS
        .get_file("index.html")
        .expect("index.html not found in embedded assets")
        .contents_utf8()
        .expect("index.html is not valid UTF-8");
    axum::response::Html(html)
}

async fn serve_assets(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    if let Some(file) = INSPECT_ASSETS.get_file(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        (
            [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
            file.contents(),
        )
            .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}
