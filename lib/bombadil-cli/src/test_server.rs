use std::{io::Result, path::PathBuf};

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::{IntoResponse, Response},
    routing::any,
};

use bombadil_schema::TraceEntry;
use tokio::sync::{broadcast, mpsc};

#[derive(Clone)]
struct AppState {
    trace_directory: PathBuf,
    trace_forward_tx: broadcast::Sender<TraceEntry>,
}

// The test server is a websocket producer that sends out new trace entries.
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

    let trace_forward_tx = broadcast::Sender::new(100);

    let state = AppState {
        trace_directory,
        trace_forward_tx,
    };

    let address = format!("127.0.0.1:{}", port.unwrap_or(0));
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let actual_port = listener.local_addr()?.port();
    let actual_address = listener.local_addr()?;
    let url = format!("http://{}", actual_address);

    tokio::fs::create_dir_all(&state.trace_directory).await?;
    tokio::fs::write(
        state.trace_directory.join("WS_PORT"),
        actual_port.to_string(),
    )
    .await?;

    let trace_forward_tx = state.trace_forward_tx.clone();
    tokio::task::spawn(async move {
        while let Some(trace) = trace_rx.recv().await {
            // send returns Err when there are no active receivers,
            // which is expected when no WS clients are connected.
            let _ = trace_forward_tx.send(trace);
            log::trace!("forwarded trace");
        }
    });

    let app = Router::new().route("/", any(ws_handler)).with_state(state);

    log::info!("connect to the WS with wscat -c ws://{actual_address}");

    axum::serve(listener, app).await?;

    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let mut new_trace_forward_rx = state.trace_forward_tx.subscribe();
    ws.on_upgrade(|mut socket: WebSocket| async move {
        log::debug!("ws client connected");

        while let Ok(new_trace) = new_trace_forward_rx.recv().await {
            let trace_json = serde_json::to_string(&new_trace).unwrap();
            let msg = Message::Text(trace_json.into());

            if socket.send(msg).await.is_err() {
                log::debug!("ws client disconnected");
                return;
            }
        }
    })
}
