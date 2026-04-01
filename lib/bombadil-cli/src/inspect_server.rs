use std::path::PathBuf;

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, State},
    response::IntoResponse,
    routing::get,
};
use bombadil_schema::TraceEntry;
use include_dir::{Dir, include_dir};

static INSPECT_ASSETS: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/../../target/inspect");

#[derive(Clone)]
struct AppState {
    trace_directory: PathBuf,
}

pub async fn serve(
    trace_path: PathBuf,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    let trace_directory = if trace_path.is_file() {
        trace_path
            .parent()
            .expect("trace path has no parent")
            .to_path_buf()
    } else {
        trace_path
    };

    let state = AppState { trace_directory };

    let app = Router::new()
        .route("/api/trace", get(trace_handler))
        .route("/api/screenshots/{filename}", get(screenshot_handler))
        .route("/", get(serve_index))
        .fallback(serve_assets)
        .with_state(state);

    let address = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let url = format!("http://{}", address);

    log::info!("Bombadil Inspect available at {}", url);

    if open_browser && let Err(error) = open::that(&url) {
        log::warn!("Failed to open browser: {}", error);
    }

    axum::serve(listener, app).await?;
    Ok(())
}

async fn trace_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<TraceEntry>>, axum::http::StatusCode> {
    let trace_file = state.trace_directory.join("trace.jsonl");
    // TODO: this should be streamed line-by-line over a websocket or SSE
    // rather than loaded into memory and sent as JSON. OK for now, but
    // worth revisiting when we do "live inspect mode".
    let content =
        tokio::fs::read_to_string(&trace_file)
            .await
            .map_err(|error| {
                log::error!("Failed to read trace file: {}", error);
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            })?;

    let entries: Vec<TraceEntry> = content
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| -> Result<TraceEntry, axum::http::StatusCode> {
            let mut entry: TraceEntry =
                serde_json::from_str(line).map_err(|error| {
                    log::error!("Failed to parse trace entry: {}", error);
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                })?;
            let filename = std::path::Path::new(&entry.screenshot)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_string();
            entry.screenshot = format!("/api/screenshots/{}", filename);
            Ok(entry)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(entries))
}

async fn screenshot_handler(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> impl IntoResponse {
    let sanitized = std::path::Path::new(&filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");

    let path = state.trace_directory.join("screenshots").join(sanitized);

    match tokio::fs::read(&path).await {
        Ok(data) => {
            let mime = mime_guess::from_path(sanitized).first_or_octet_stream();
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
        Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
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
        axum::http::StatusCode::NOT_FOUND.into_response()
    }
}
