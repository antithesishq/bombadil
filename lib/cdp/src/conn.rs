use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cdp_protocol::cdp::browser_protocol::target::SessionId;
use crossbeam_channel as mpmc;
use tungstenite::client::{IntoClientRequest, connect_with_config};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message as WsMessage, WebSocket};

use cdp_types::{
    CallId, CdpJsonEventMessage, Command, Message as CdpMessage, MethodCall,
};
use serde_json as json;

use anyhow::{anyhow, bail, ensure};

use crate::error::Result;
use crate::events::{Events, Subscribers};

#[derive(Debug, Clone)]
pub struct Connection {
    inner: Arc<Mutex<ConnectionInner>>,
    pub events: Events,
}

impl Connection {
    pub fn connect(url: impl IntoClientRequest) -> Result<Self> {
        let (inner, events) = ConnectionInner::connect(url)?;
        Ok(Connection {
            inner: Arc::new(Mutex::new(inner)),
            events,
        })
    }

    pub fn send<T: Command>(
        &self,
        cmd: T,
        session_id: Option<&SessionId>,
    ) -> Result<T::Response> {
        let mut inner = self
            .inner
            .lock()
            .expect("couldn't acquire lock for inner connection");
        inner.send(cmd, session_id)
    }

    pub fn close(&self) -> Result<()> {
        self.events.close();
        let mut inner = self
            .inner
            .lock()
            .expect("couldn't acquire lock for inner connection");
        inner.close()
    }
}

#[derive(Debug)]
struct ConnectionInner {
    next_id: usize,
    worker_tx: mpmc::Sender<WorkerRequest>,
    handle: Option<thread::JoinHandle<()>>,
    closed: bool,
}

impl ConnectionInner {
    fn connect(url: impl IntoClientRequest) -> Result<(Self, Events)> {
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

        let (worker_tx, worker_rx) = mpmc::bounded(1);
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));

        let handle = {
            let subscribers = subscribers.clone();
            thread::spawn(move || {
                if let Err(err) = websocket_worker(ws, worker_rx, subscribers) {
                    log::error!("websocket worker died: {err}");
                }
            })
        };

        Ok((
            Self {
                next_id: 0,
                worker_tx,
                handle: Some(handle),
                closed: false,
            },
            Events { subscribers },
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
        session_id: Option<&SessionId>,
    ) -> Result<T::Response> {
        let call_id = self.next_call_id();
        log::debug!("sending {} ({})", cmd.identifier(), call_id);

        let call = MethodCall {
            id: call_id,
            method: cmd.identifier(),
            session_id: session_id.map(|id| id.inner().into()),
            params: serde_json::to_value(&cmd)?,
        };
        let (reply_tx, reply_rx) = mpmc::bounded(1);
        self.worker_tx
            .send(WorkerRequest::Send { call, reply_tx })?;

        let result = match reply_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(mpmc::RecvTimeoutError::Timeout) => {
                bail!(
                    "timed out waiting for response for {}",
                    cmd.identifier(),
                );
            }
            Err(mpmc::RecvTimeoutError::Disconnected) => {
                bail!(
                    "channel disconnected while waiting for response for {}",
                    cmd.identifier(),
                )
            }
        };

        match result {
            Ok(value) => {
                log::debug!(
                    "got response for {} ({}): {}",
                    cmd.identifier(),
                    call_id,
                    value
                );
                Ok(T::response_from_value(value)?)
            }
            Err(err) => {
                log::debug!(
                    "got error for {} ({}): {}",
                    cmd.identifier(),
                    call_id,
                    err
                );
                Err(err)
            }
        }
    }

    pub fn close(&mut self) -> Result<()> {
        log::debug!("closing CDP websocket");
        if self.closed {
            return Ok(());
        }
        let _ = self.worker_tx.send(WorkerRequest::Close);
        if let Some(handle) = self.handle.take() {
            handle.join().expect("websocket worker panicked");
        }
        self.closed = true;
        Ok(())
    }
}

impl Drop for ConnectionInner {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

enum WorkerRequest {
    Send {
        call: MethodCall,
        reply_tx: mpmc::Sender<Result<json::Value>>,
    },
    Close,
}

fn websocket_worker(
    mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
    requests_rx: mpmc::Receiver<WorkerRequest>,
    subscribers: Arc<Mutex<Subscribers>>,
) -> Result<()> {
    log::debug!("starting websocket worker");
    let mut call_current = None;
    loop {
        match requests_rx.try_recv() {
            Ok(WorkerRequest::Send { call, reply_tx }) => {
                ensure!(
                    call_current.is_none(),
                    "concurrent send() is not supported"
                );
                call_current = Some((call.clone(), reply_tx));
                let payload = serde_json::to_string(&call)?;
                ws.send(WsMessage::text(payload))?;
            }
            Ok(WorkerRequest::Close) => {
                ws.close(None)?;
                return Ok(());
            }
            Err(mpmc::TryRecvError::Empty) => {}
            Err(mpmc::TryRecvError::Disconnected) => {
                bail!("command mpmc closed unexpectedly");
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
                    CdpMessage::Response(response) => {
                        log::debug!("got response: {response:?}");
                        if let Some((call, reply_tx)) = call_current.take() {
                            if response.id != call.id {
                                bail!(
                                    "Response id {got} did not match in-flight request id {expected} (concurrent send() is not supported)",
                                    expected = call.id,
                                    got = response.id,
                                );
                            }
                            if let Some(err) = response.error {
                                reply_tx.send(Err(err.into()))?;
                            } else {
                                let result = response
                                    .result
                                    .unwrap_or(serde_json::Value::Null);
                                reply_tx.send(Ok(result))?;
                            }
                        } else {
                            bail!(
                                "Got unexpected response with no request in flight"
                            );
                        }
                    }
                    CdpMessage::Event(event) => {
                        let subscribers = subscribers.lock().map_err(|_| {
                            anyhow!("failed to acquire lock for subscribers")
                        })?;
                        if !subscribers.closed {
                            subscribers.dispatch(event);
                        }
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
                // Both worker commands and events are non-blocking, so wait in order to not
                // busy-loop.
                thread::sleep(Duration::from_millis(2));
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }
}
