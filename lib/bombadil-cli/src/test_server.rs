/// This is a web server for `bombadil test` that streams trace entries for `bombadil inspect`. See https://github.com/antithesishq/bombadil/pull/141
use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::any,
};
use bombadil_schema::{Time, TraceEntry};
use std::{io::Result, path::PathBuf};
use tokio::sync::{broadcast, mpsc};

#[derive(Clone)]
struct AppState {
    trace_directory: PathBuf,
    trace_forward_tx: broadcast::Sender<TraceEntry>,
}

pub async fn serve(
    trace_path: PathBuf,
    port: Option<u16>,
    mut trace_rx: mpsc::Receiver<TraceEntry>,
) -> Result<()> {
    log::debug!("starting ws server");

    let trace_directory = if trace_path.is_file() {
        trace_path
            .parent()
            .expect("trace path has no parent")
            .to_path_buf()
    } else {
        trace_path
    };

    let trace_forward_tx = broadcast::Sender::new(64);

    let state = AppState {
        trace_directory,
        trace_forward_tx,
    };

    let address = format!("127.0.0.1:{}", port.unwrap_or(0));
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let actual_port = listener.local_addr()?.port();
    let actual_address = listener.local_addr()?;

    tokio::fs::create_dir_all(&state.trace_directory).await?;
    tokio::fs::write(
        state.trace_directory.join("WS_PORT"),
        actual_port.to_string(),
    )
    .await?;

    let trace_forward_tx = state.trace_forward_tx.clone();
    tokio::task::spawn(async move {
        while let Some(trace) = trace_rx.recv().await {
            // `send` returns Err when there are no active receivers
            // (eg when no ws clients). Ignore.
            let _ = trace_forward_tx.send(trace);
        }
    });

    let app = Router::new().route("/", any(ws_handler)).with_state(state);

    log::info!("connect to the WS with wscat -c ws://{actual_address}");

    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(serde::Serialize)]
#[serde(tag = "type", content = "data")]
enum WsMessage {
    #[serde(rename = "entry")]
    Entry(TraceEntry),
    #[serde(rename = "allEntries")]
    AllEntries(Vec<TraceEntry>),
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Subscribe before anything else so none are dropped.
    let mut new_trace_forward_rx = state.trace_forward_tx.subscribe();

    let existing = get_all_traces(state.trace_directory)
        .await
        .unwrap_or_default();
    let last_existing_timestamp = existing
        .last()
        .map(|e| e.timestamp)
        .unwrap_or(Time::from_micros(0));

    ws.on_upgrade(async move |mut socket: WebSocket| {
        log::debug!("ws client connected");

        // Send all existing traces first.
        let msg =
            serde_json::to_string(&WsMessage::AllEntries(existing)).unwrap();
        if socket.send(Message::Text(msg.into())).await.is_err() {
            log::debug!(
                "ws client disconnected before existing traces were sent"
            );
            return;
        }

        // Then stream new ones as they arrive.
        while let Ok(new_trace) = new_trace_forward_rx.recv().await {
            // Filter out duplicate traces (in case one comes in while get_all_traces is running).
            if new_trace.timestamp <= last_existing_timestamp {
                continue;
            }

            let msg = serde_json::to_string(&WsMessage::Entry(new_trace))
                .expect("Failed to serialize trace entry");
            if socket.send(Message::Text(msg.into())).await.is_err() {
                log::debug!("ws client disconnected");
                return;
            }
        }
    })
}

async fn get_all_traces(
    trace_directory: PathBuf,
) -> anyhow::Result<Vec<TraceEntry>> {
    let contents =
        &tokio::fs::read(trace_directory.join("trace.jsonl")).await?;
    String::from_utf8_lossy(contents)
        .lines()
        .map(|e| Ok(serde_json::from_str(e)?))
        .collect()
}
