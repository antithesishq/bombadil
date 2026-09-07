use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::os::fd::{AsRawFd, RawFd};
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

use anyhow::{Context, anyhow, bail};

use crate::error::Result;
use crate::events::{Events, Subscribers};

const WEBSOCKET: Token = Token(0);
const COMMANDS: Token = Token(1);

#[cfg(test)]
static RETRYABLE_WRITE_COUNT: AtomicUsize = AtomicUsize::new(0);

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
                supervise_worker(&subscribers, || {
                    websocket_worker(ws, poll, worker_rx, subscribers.clone())
                });
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
        log::debug!(
            "posting {} ({}), session={:?}",
            cmd.identifier(),
            call_id,
            session_id
        );

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
        log::debug!(
            "sending {} ({}), session={:?}",
            cmd.identifier(),
            call_id,
            session_id
        );

        let call = MethodCall {
            id: call_id,
            method: cmd.identifier(),
            session_id: session_id.map(|id| id.inner().into()),
            params: serde_json::to_value(&cmd)?,
        };
        let (reply_tx, reply_rx) = mpmc::bounded(1);
        self.worker_tx
            .send(WorkerRequest::Send {
                call,
                reply_tx: Some(reply_tx),
            })
            .context(format!("send failed for {}", cmd.identifier()))?;
        self.commands_waker.wake()?;

        let result = match reply_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => {
                result.context(format!("send failed for {}", cmd.identifier()))
            }
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

fn supervise_worker(
    subscribers: &Arc<Mutex<Subscribers>>,
    worker: impl FnOnce() -> Result<()>,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker));
    let error = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("{error:#}")),
        Err(_) => Some("websocket worker panicked".to_string()),
    };
    subscribers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .close(error.clone());
    subscribers.clear_poison();
    if let Some(error) = error {
        log::error!("websocket worker died: {error}");
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
                let reply_error =
                    |reply_tx: &Option<mpmc::Sender<Result<json::Value>>>,
                     error| {
                        if let Some(tx) = reply_tx {
                            if let Err(error) = tx.send(Err(error)) {
                                log::error!(
                                    "failed sending command error: {error:#}"
                                );
                            }
                        } else {
                            log::error!("posted command failed: {error:#}");
                        }
                    };

                if calls_in_flight.contains_key(&call.id) {
                    reply_error(
                        &reply_tx,
                        anyhow!("call {} already in flight", call.id),
                    );
                    continue;
                }

                let payload = match serde_json::to_string(&call) {
                    Ok(payload) => payload,
                    Err(error) => {
                        reply_error(&reply_tx, error.into());
                        continue;
                    }
                };
                if let Err(error) =
                    write_nonblocking(ws, WsMessage::text(payload))
                {
                    let message = format!("{error:#}");
                    reply_error(&reply_tx, anyhow!(message.clone()));
                    bail!("failed writing call {}: {message}", call.id);
                }
                calls_in_flight.insert(call.id, reply_tx);
            }
            Ok(WorkerRequest::Close) => {
                match ws.close(None) {
                    Ok(()) => {}
                    Err(error) if is_retryable(&error) => {}
                    Err(error) => return Err(error.into()),
                }
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

fn is_retryable(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::Io(error)
            if error.kind() == ErrorKind::WouldBlock
                || error.kind() == ErrorKind::TimedOut
    )
}

fn record_retryable_write() {
    #[cfg(test)]
    RETRYABLE_WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
}

fn write_nonblocking(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    message: WsMessage,
) -> Result<()> {
    match ws.write(message) {
        Ok(()) => Ok(()),
        // Tungstenite retains the frame after an I/O failure. Writable
        // readiness will drive `flush` again.
        Err(error) if is_retryable(&error) => {
            record_retryable_write();
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn flush_nonblocking(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
) -> Result<bool> {
    match ws.flush() {
        Ok(()) => Ok(false),
        Err(error) if is_retryable(&error) => {
            record_retryable_write();
            Ok(true)
        }
        Err(error) => Err(error.into()),
    }
}

fn set_write_interest(
    poll: &Poll,
    fd_raw: RawFd,
    registered: &mut bool,
    write_pending: bool,
) -> Result<()> {
    if *registered == write_pending {
        return Ok(());
    }

    let interest = if write_pending {
        Interest::READABLE.add(Interest::WRITABLE)
    } else {
        Interest::READABLE
    };
    poll.registry()
        .reregister(&mut SourceFd(&fd_raw), WEBSOCKET, interest)?;
    *registered = write_pending;
    Ok(())
}

#[hotpath::measure]
fn handle_message(
    msg: WsMessage,
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
                if let Some(error) = &response.error {
                    log::debug!(
                        "received command error from websocket: call={}, error={}",
                        response.id,
                        error,
                    );
                }
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
                        if response.error.is_none() {
                            log::debug!(
                                "received response for post {}: exception_details={:?}",
                                response.id,
                                response.result.as_ref().and_then(|result| {
                                    result.get("exceptionDetails")
                                }),
                            );
                        }
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
                if matches!(
                    event.method.as_ref(),
                    "Debugger.paused" | "Debugger.resumed"
                ) {
                    log::debug!(
                        "received {} from websocket: session={:?}",
                        event.method,
                        event.session_id,
                    );
                }
                // Observe lifecycle changes before dispatch: the browser state
                // machine may be blocked waiting for an evaluation response.
                if matches!(
                    event.method.as_ref(),
                    "Page.frameStartedNavigating"
                        | "Page.frameRequestedNavigation"
                        | "Page.frameStartedLoading"
                        | "Page.frameStoppedLoading"
                        | "Page.frameNavigated"
                        | "Page.navigatedWithinDocument"
                        | "Page.frameDetached"
                        | "Runtime.executionContextCreated"
                        | "Runtime.executionContextDestroyed"
                        | "Runtime.executionContextsCleared"
                        | "Target.targetCreated"
                        | "Target.attachedToTarget"
                        | "Target.targetInfoChanged"
                        | "Target.targetDestroyed"
                        | "Target.detachedFromTarget"
                ) {
                    log::debug!(
                        "received {} from websocket: session={:?}, params={}",
                        event.method,
                        event.session_id,
                        event.params.get(),
                    );
                }
                let mut subscribers = subscribers.lock().map_err(|_| {
                    anyhow!("failed to acquire lock for subscribers")
                })?;
                if !subscribers.closed {
                    subscribers.dispatch(event);
                }
            }
        }
        // Tungstenite queues Pong replies automatically while reading.
        WsMessage::Ping(_) => {}
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
    let fd_raw = match ws.get_ref() {
        MaybeTlsStream::Plain(stream) => stream.as_raw_fd(),
        _ => bail!("unsupported stream type"),
    };
    let mut write_interest_registered = false;

    loop {
        if matches!(
            calls_drain(&mut ws, &requests_rx, &mut calls_in_flight)?,
            DrainResult::Shutdown
        ) {
            return Ok(());
        }
        let mut write_pending = flush_nonblocking(&mut ws)?;
        set_write_interest(
            &poll,
            fd_raw,
            &mut write_interest_registered,
            write_pending,
        )?;

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
                    write_pending = flush_nonblocking(&mut ws)?;
                }
                WEBSOCKET => {
                    if event.is_writable() {
                        write_pending = flush_nonblocking(&mut ws)?;
                    }
                    if event.is_readable() {
                        loop {
                            match ws.read() {
                                Ok(msg) => handle_message(
                                    msg,
                                    &mut calls_in_flight,
                                    &subscribers,
                                )?,
                                Err(error) if is_retryable(&error) => break,
                                Err(error) => return Err(error.into()),
                            }
                        }
                        write_pending = flush_nonblocking(&mut ws)?;
                    }
                }
                _ => {}
            }

            set_write_interest(
                &poll,
                fd_raw,
                &mut write_interest_registered,
                write_pending,
            )?;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::net::TcpListener;
    use std::time::Duration;

    use serde::{Deserialize, Serialize};

    use super::*;
    use cdp_types::{Command, Method, MethodId};

    #[derive(Debug, Serialize)]
    struct TestCommand {
        payload: String,
    }

    impl Method for TestCommand {
        fn identifier(&self) -> MethodId {
            Cow::Borrowed("Test.command")
        }
    }

    impl Command for TestCommand {
        type Response = TestResponse;
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct TestResponse {
        received: bool,
    }

    fn websocket_server(
        pause_before_reading: Duration,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let config = WebSocketConfig::default()
                .max_message_size(None)
                .max_frame_size(None);
            let mut websocket =
                tungstenite::accept_with_config(stream, Some(config)).unwrap();
            thread::sleep(pause_before_reading);

            loop {
                match websocket.read() {
                    Ok(WsMessage::Text(text)) => {
                        let message: json::Value =
                            serde_json::from_str(text.as_str()).unwrap();
                        websocket
                            .send(WsMessage::text(
                                json::json!({
                                    "id": message["id"],
                                    "result": {"received": true},
                                })
                                .to_string(),
                            ))
                            .unwrap();
                    }
                    Ok(WsMessage::Close(_))
                    | Err(tungstenite::Error::ConnectionClosed)
                    | Err(tungstenite::Error::AlreadyClosed) => break,
                    Ok(_) => {}
                    Err(error) => {
                        panic!("test WebSocket server failed: {error}")
                    }
                }
            }
        });
        (format!("ws://{address}"), handle)
    }

    #[test]
    fn large_write_survives_socket_backpressure() {
        RETRYABLE_WRITE_COUNT.store(0, Ordering::Relaxed);
        let (url, server) = websocket_server(Duration::from_millis(250));
        let connection = Connection::connect(url).unwrap();

        connection
            .post(
                TestCommand {
                    payload: "x".repeat(16 * 1024 * 1024),
                },
                None,
            )
            .unwrap();
        let response = connection
            .send(
                TestCommand {
                    payload: "small".into(),
                },
                None,
            )
            .unwrap();

        assert_eq!(response, TestResponse { received: true });
        assert!(RETRYABLE_WRITE_COUNT.load(Ordering::Relaxed) > 0);
        connection.close().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn worker_panic_wakes_subscribers() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let events = Events {
            subscribers: subscribers.clone(),
        };
        let receiver = events.subscribe::<TestEvent>();
        supervise_worker(&subscribers, || panic!("injected worker panic"));
        assert_eq!(
            receiver.next().unwrap_err().to_string(),
            "websocket worker panicked"
        );
    }

    #[test]
    fn worker_failure_disconnects_event_subscribers_with_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let websocket = tungstenite::accept(stream).unwrap();
            drop(websocket);
        });
        let connection =
            Connection::connect(format!("ws://{address}")).unwrap();
        let subscriber = connection.events.subscribe::<TestEvent>();
        let (result_tx, result_rx) = mpmc::bounded(1);

        thread::spawn(move || {
            result_tx.send(subscriber.next()).unwrap();
        });

        let result = result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("subscriber remained blocked after worker failure");
        assert!(result.is_err());
        server.join().unwrap();
    }

    #[derive(Debug, Deserialize)]
    struct TestEvent;

    impl cdp_types::MethodType for TestEvent {
        fn method_id() -> MethodId {
            Cow::Borrowed("Test.event")
        }
    }
}
