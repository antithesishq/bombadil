use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cdp_protocol::cdp::browser_protocol::target::SessionId;
use crossbeam_channel as mpmc;
use mio::unix::SourceFd;
use mio::{Events as MioEvents, Interest, Poll, Token, Waker};
use tungstenite::client::{IntoClientRequest, connect_with_config};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message as WsMessage, WebSocket};

use cdp_types::{CallId, CdpJsonEventMessage, Command, MethodCall, Response};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json as json;

use anyhow::{anyhow, bail, ensure};

use crate::error::Result;
use crate::events::{Events, Subscribers};

const WEBSOCKET: Token = Token(0);
const COMMANDS: Token = Token(1);

#[derive(Debug, Clone)]
pub struct Connection {
    inner: Arc<ConnectionInner>,
    pub events: Events,
}

impl Connection {
    pub fn connect(url: impl IntoClientRequest) -> Result<Self> {
        let (inner, events) = ConnectionInner::connect(url)?;
        Ok(Connection {
            inner: Arc::new(inner),
            events,
        })
    }

    /// Send a command and await its response.
    pub fn send<T: Command>(
        &self,
        cmd: T,
        session_id: Option<&SessionId>,
    ) -> Result<T::Response> {
        self.inner.send(cmd, session_id)
    }

    /// Post a command without awaiting its response.
    pub fn post<T: Command>(
        &self,
        cmd: T,
        session_id: Option<&SessionId>,
    ) -> Result<()> {
        self.inner.post(cmd, session_id)
    }

    pub fn close(&self) -> Result<()> {
        self.events.close();
        self.inner.close()
    }
}

#[derive(Debug)]
struct ConnectionInner {
    next_id: AtomicUsize,
    worker_tx: mpmc::Sender<WorkerRequest>,
    commands_waker: Arc<Waker>,
    close_state: Mutex<CloseState>,
}

#[derive(Debug)]
struct CloseState {
    handle: Option<thread::JoinHandle<()>>,
    closed: bool,
}

impl ConnectionInner {
    fn connect(url: impl IntoClientRequest) -> Result<(Self, Events)> {
        let config = WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        let (mut ws, _resp) = connect_with_config(url, Some(config), 3)?;
        let fd_raw = {
            let stream = ws.get_mut();
            match stream {
                MaybeTlsStream::Plain(stream) => {
                    stream.set_nodelay(true)?;
                    stream.set_nonblocking(true)?;
                    stream.as_raw_fd()
                }
                _ => bail!("unsupported stream type"),
            }
        };

        let poll = Poll::new()?;
        poll.registry().register(
            &mut SourceFd(&fd_raw),
            WEBSOCKET,
            Interest::READABLE,
        )?;
        let commands_waker = Arc::new(Waker::new(poll.registry(), COMMANDS)?);

        let (worker_tx, worker_rx) = mpmc::bounded(16);
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));

        let handle = {
            let subscribers = subscribers.clone();
            thread::spawn(move || {
                if let Err(err) =
                    websocket_worker(ws, poll, worker_rx, subscribers)
                {
                    log::error!("websocket worker died: {err}");
                }
            })
        };

        Ok((
            Self {
                next_id: AtomicUsize::new(0),
                worker_tx,
                commands_waker,
                close_state: Mutex::new(CloseState {
                    handle: Some(handle),
                    closed: false,
                }),
            },
            Events { subscribers },
        ))
    }

    fn next_call_id(&self) -> CallId {
        CallId::new(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    #[hotpath::measure]
    pub(crate) fn post<T: Command>(
        &self,
        cmd: T,
        session_id: Option<&SessionId>,
    ) -> Result<()> {
        let call_id = self.next_call_id();
        log::debug!("posting {} ({})", cmd.identifier(), call_id);

        let call = MethodCall {
            id: call_id,
            method: cmd.identifier(),
            session_id: session_id.map(|id| id.inner().into()),
            params: serde_json::to_value(&cmd)?,
        };

        self.worker_tx.send(WorkerRequest::Send {
            call,
            reply_tx: None,
        })?;
        self.commands_waker.wake()?;
        Ok(())
    }

    #[hotpath::measure]
    pub(crate) fn send<T: Command>(
        &self,
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
        self.worker_tx.send(WorkerRequest::Send {
            call,
            reply_tx: Some(reply_tx),
        })?;
        self.commands_waker.wake()?;

        let result = match reply_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(mpmc::RecvTimeoutError::Timeout) => {
                log::debug!(
                    "timed out waiting for response for {}",
                    cmd.identifier()
                );
                bail!(
                    "timed out waiting for response for {}",
                    cmd.identifier(),
                );
            }
            Err(mpmc::RecvTimeoutError::Disconnected) => {
                log::debug!(
                    "channel disconnected while waiting for response for {}",
                    cmd.identifier()
                );
                bail!(
                    "channel disconnected while waiting for response for {}",
                    cmd.identifier(),
                )
            }
        };

        match result {
            Ok(value) => {
                log::debug!(
                    "got response for {} ({})",
                    cmd.identifier(),
                    call_id,
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

    pub(crate) fn close(&self) -> Result<()> {
        let mut state = self
            .close_state
            .lock()
            .expect("couldn't acquire lock for close state");
        if state.closed {
            return Ok(());
        }
        log::debug!("closing CDP websocket");
        let _ = self.worker_tx.send(WorkerRequest::Close);
        let _ = self.commands_waker.wake();
        if let Some(handle) = state.handle.take() {
            handle.join().expect("websocket worker panicked");
        }
        state.closed = true;
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
        reply_tx: Option<mpmc::Sender<Result<json::Value>>>,
    },
    Close,
}

type CallsInFlight = HashMap<CallId, Option<mpmc::Sender<Result<json::Value>>>>;

enum DrainResult {
    Continue,
    Shutdown,
}

fn calls_drain(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    requests_rx: &mpmc::Receiver<WorkerRequest>,
    calls_in_flight: &mut CallsInFlight,
) -> Result<DrainResult> {
    loop {
        match requests_rx.try_recv() {
            Ok(WorkerRequest::Send { call, reply_tx }) => {
                ensure!(
                    !calls_in_flight.contains_key(&call.id),
                    "call {} already in flight",
                    call.id
                );
                calls_in_flight.insert(call.id, reply_tx);
                let payload = serde_json::to_string(&call)?;
                ws.send(WsMessage::text(payload))?;
            }
            Ok(WorkerRequest::Close) => {
                ws.close(None)?;
                return Ok(DrainResult::Shutdown);
            }
            Err(mpmc::TryRecvError::Empty) => {
                return Ok(DrainResult::Continue);
            }
            Err(mpmc::TryRecvError::Disconnected) => {
                bail!("command mpmc closed unexpectedly");
            }
        }
    }
}

#[hotpath::measure]
fn handle_message(
    msg: WsMessage,
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    calls_in_flight: &mut CallsInFlight,
    subscribers: &Arc<Mutex<Subscribers>>,
) -> Result<()> {
    match msg {
        WsMessage::Text(text) => {
            let text_str = text.as_str();
            // We only parse `Message` when we know that there's no `id` field, and otherwise
            // parse as `Response`. Hence, we need do a first parsing pass with this cheap struct.
            #[derive(Deserialize)]
            struct Peek {
                #[serde(default)]
                id: Option<IgnoredAny>,
            }
            let peek: Peek = serde_json::from_str(text_str).map_err(|err| {
                anyhow!("failed to parse ws text frame '{}': {err}", text_str)
            })?;
            if peek.id.is_some() {
                let response: Response = serde_json::from_str(text_str)
                    .map_err(|err| {
                        anyhow!(
                            "failed to parse response '{}': {err}",
                            text_str
                        )
                    })?;
                if let Some(reply_tx) = calls_in_flight.remove(&response.id) {
                    // There's only a reply_tx if it was a `send`, not for `post`.
                    if let Some(reply_tx) = reply_tx {
                        if let Some(err) = response.error {
                            let _ = reply_tx.send(Err(err.into()));
                        } else {
                            let result = response
                                .result
                                .unwrap_or(serde_json::Value::Null);
                            let _ = reply_tx.send(Ok(result));
                        }
                    } else {
                        log::debug!(
                            "ignoring response for post {}",
                            response.id
                        );
                    }
                } else {
                    bail!(
                        "got unexpected response ({}) with no corresponding request in flight",
                        response.id
                    );
                }
            } else {
                let event: CdpJsonEventMessage = serde_json::from_str(text_str)
                    .map_err(|err| {
                        anyhow!("failed to parse event '{}': {err}", text_str)
                    })?;
                let mut subscribers = subscribers.lock().map_err(|_| {
                    anyhow!("failed to acquire lock for subscribers")
                })?;
                if !subscribers.closed {
                    subscribers.dispatch(event);
                }
            }
        }
        WsMessage::Ping(payload) => {
            ws.send(WsMessage::Pong(payload))?;
        }
        WsMessage::Pong(_) => {}
        WsMessage::Close(_) => {
            bail!("The websocket connection was closed by the peer.");
        }
        other @ (WsMessage::Binary(_) | WsMessage::Frame(_)) => {
            bail!("Received unexpected ws message: {other:?}");
        }
    }
    Ok(())
}

fn websocket_worker(
    mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
    mut poll: Poll,
    requests_rx: mpmc::Receiver<WorkerRequest>,
    subscribers: Arc<Mutex<Subscribers>>,
) -> Result<()> {
    log::debug!("starting websocket worker");
    // TODO: clean up map periodically or using timers, as it
    // can grow unboundedly on requests timing out or for some
    // other reason not receiving responses
    let mut calls_in_flight: CallsInFlight = HashMap::new();
    let mut mio_events = MioEvents::with_capacity(16);

    loop {
        if matches!(
            calls_drain(&mut ws, &requests_rx, &mut calls_in_flight)?,
            DrainResult::Shutdown
        ) {
            return Ok(());
        }
        ws.flush()?;

        poll.poll(&mut mio_events, None)?;

        for event in mio_events.iter() {
            match event.token() {
                COMMANDS => {
                    if matches!(
                        calls_drain(
                            &mut ws,
                            &requests_rx,
                            &mut calls_in_flight,
                        )?,
                        DrainResult::Shutdown
                    ) {
                        return Ok(());
                    }
                    ws.flush()?;
                }
                WEBSOCKET => loop {
                    match ws.read() {
                        Ok(msg) => handle_message(
                            msg,
                            &mut ws,
                            &mut calls_in_flight,
                            &subscribers,
                        )?,
                        Err(tungstenite::error::Error::Io(ref e))
                            if e.kind() == ErrorKind::WouldBlock
                                || e.kind() == ErrorKind::TimedOut =>
                        {
                            break;
                        }
                        Err(e) => return Err(e.into()),
                    }
                },
                _ => {}
            }
        }
    }
}
