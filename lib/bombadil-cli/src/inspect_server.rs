use anyhow::Result;
use axum::{Json, Router, response::IntoResponse, routing::get};
use bombadil_inspect_api::HelloResponse;
use include_dir::{Dir, include_dir};
use std::path::PathBuf;

static INSPECT_ASSETS: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/../../target/inspect");

pub async fn serve(
    trace_path: PathBuf,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    log::info!("Trace path provided (not used in MVP): {:?}", trace_path);
    let app = Router::new()
        .route("/api/hello", get(hello_handler))
        .route("/", get(serve_index))
        .fallback(serve_assets);

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

async fn hello_handler() -> Json<HelloResponse> {
    Json(HelloResponse {
        message: "Hello from Bombadil!".to_string(),
    })
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
