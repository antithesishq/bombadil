use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tungstenite::client::{IntoClientRequest, connect_with_config};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message as WsMessage, WebSocket};

use cdp_types::{
    CallId, CdpJsonEventMessage, Command, Message as CdpMessage, MethodCall,
};
use serde_json as json;

use anyhow::{Context, anyhow, bail, ensure};

use crate::error::Result;

#[derive(Debug)]
pub struct Connection {
    next_id: usize,
    worker_tx: mpsc::SyncSender<WorkerRequest>,
    handle: thread::JoinHandle<Result<()>>,
}

impl Connection {
    pub fn connect(
        url: impl IntoClientRequest,
    ) -> Result<(Self, mpsc::Receiver<CdpJsonEventMessage>)> {
        let config = WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        let (mut ws, _resp) = connect_with_config(url, Some(config), 3)?;
        let stream = ws.get_mut();
        match stream {
            MaybeTlsStream::Plain(stream) => {
                stream.set_nonblocking(true)?;
            }
            _ => bail!("unsupported stream type"),
        }

        let (events_tx, events_rx) = mpsc::sync_channel(1024);
        let (worker_tx, worker_rx) = mpsc::sync_channel(1);

        let handle =
            thread::spawn(move || websocket_worker(ws, worker_rx, events_tx));

        Ok((
            Self {
                next_id: 0,
                worker_tx,
                handle,
            },
            events_rx,
        ))
    }

    fn next_call_id(&mut self) -> CallId {
        let id = CallId::new(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    pub fn send<T: Command>(
        &mut self,
        cmd: T,
        session_id: Option<String>,
    ) -> Result<T::Response> {
        let call_id = self.next_call_id();
        log::info!("sending {} ({})", cmd.identifier(), call_id);

        let call = MethodCall {
            id: call_id,
            method: cmd.identifier(),
            session_id,
            params: serde_json::to_value(&cmd)?,
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        self.worker_tx.send(WorkerRequest::Send {
            call,
            reply: reply_tx,
        })?;

        let value =
            reply_rx
                .recv_timeout(Duration::from_secs(5))
                .context(format!(
                    "timed out waiting for response for {}",
                    cmd.identifier()
                ))?;

        log::info!("got response for {} ({})", cmd.identifier(), call_id);
        Ok(T::response_from_value(value)?)
    }

    pub fn close(self) -> Result<()> {
        self.worker_tx.send(WorkerRequest::Close)?;
        self.handle
            .join()
            .map_err(|err| anyhow!("websocket worker panicked: {:?}", err,))?
    }
}

enum WorkerRequest {
    Send {
        call: MethodCall,
        reply: oneshot::Sender<json::Value>,
    },
    Close,
}

fn websocket_worker(
    mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
    requests_rx: mpsc::Receiver<WorkerRequest>,
    events_tx: mpsc::SyncSender<CdpJsonEventMessage>,
) -> Result<()> {
    log::info!("starting websocket worker");
    let mut call_current = None;
    loop {
        match requests_rx.try_recv() {
            Ok(WorkerRequest::Send { call, reply }) => {
                ensure!(
                    call_current.is_none(),
                    "concurrent send() is not supported"
                );
                call_current = Some((call.clone(), reply));
                let payload = serde_json::to_string(&call)?;
                ws.send(WsMessage::text(payload))?;
            }
            Ok(WorkerRequest::Close) => {
                ws.close(None)?;
                return Ok(());
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                bail!("command channel closed unexpectedly");
            }
        };
        match ws.read() {
            Ok(WsMessage::Text(text)) => {
                let parsed: CdpMessage<CdpJsonEventMessage> =
                    serde_json::from_str(text.as_str()).map_err(|e| {
                        anyhow!(
                            "Failed to parse ws text frame '{}': {e}",
                            text.as_str()
                        )
                    })?;
                match parsed {
                    CdpMessage::Response(resp) => {
                        if let Some((call, reply)) = call_current.take() {
                            if resp.id != call.id {
                                bail!(
                                    "Response id {got} did not match in-flight request id {expected} (concurrent send() is not supported)",
                                    expected = call.id,
                                    got = resp.id,
                                );
                            }
                            if let Some(err) = resp.error {
                                return Err(err.into());
                            }
                            let result =
                                resp.result.unwrap_or(serde_json::Value::Null);
                            reply.send(result)?;
                        } else {
                            bail!(
                                "Got unexpected response with no request in flight"
                            );
                        }
                    }
                    CdpMessage::Event(ev) => {
                        let _ = events_tx.send(ev);
                    }
                }
            }
            Ok(WsMessage::Ping(payload)) => {
                ws.send(WsMessage::Pong(payload))?;
            }
            Ok(WsMessage::Pong(_)) => {}
            Ok(WsMessage::Close(_)) => {
                bail!("The websocket connection was closed by the peer.");
            }
            Ok(other @ (WsMessage::Binary(_) | WsMessage::Frame(_))) => {
                bail!("Received unexpected ws message: {other:?}");
            }
            Err(tungstenite::error::Error::Io(ref e))
                if e.kind() == ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }
}
