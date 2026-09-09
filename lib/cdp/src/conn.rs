use std::borrow::Cow;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::net::TcpStream;
use std::ops::Range;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cdp_protocol::cdp::browser_protocol::target::SessionId;
use crossbeam_channel as mpmc;
use mio::unix::SourceFd;
use mio::{Events as MioEvents, Interest, Poll, Token, Waker};
use tungstenite::client::{IntoClientRequest, connect_with_config};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message as WsMessage, Utf8Bytes, WebSocket};

use cdp_types::{CallId, CdpJsonEventMessage, Command, MethodCall, MethodId};
use serde::Deserialize;
#[cfg(test)]
use serde_json as json;
use serde_json::value::RawValue;

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
    #[hotpath::measure]
    pub fn send<T: Command>(
        &self,
        cmd: T,
        session_id: Option<&SessionId>,
    ) -> Result<T::Response> {
        self.request(cmd, session_id)?.wait()
    }

    /// Submit a command without waiting for its response.
    ///
    /// The five-second response deadline starts at submission. Submit independent
    /// commands before calling [`PendingResponse::wait`] to overlap their waits.
    /// Submission may block on the bounded worker queue, within that deadline.
    pub fn request<T: Command>(
        &self,
        cmd: T,
        session_id: Option<&SessionId>,
    ) -> Result<PendingResponse<T>> {
        self.inner.request(cmd, session_id)
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

/// A submitted command's typed response. Waiting consumes the handle.
///
/// Dropping this handle discards the response; it does not cancel the command.
/// Keep the connection alive until the outstanding responses have been collected.
#[derive(Debug)]
#[must_use = "wait for the response, or explicitly drop it to discard the result"]
pub struct PendingResponse<T: Command> {
    call_id: CallId,
    method: MethodId,
    deadline: Instant,
    reply_rx: mpmc::Receiver<Reply>,
    command: PhantomData<T>,
}

impl<T: Command> PendingResponse<T> {
    /// Wait until the original submission deadline, then decode the response.
    /// A response received before the deadline can be collected after it expires.
    #[hotpath::measure]
    pub fn wait(self) -> Result<T::Response> {
        let (received_at, result) = match self
            .reply_rx
            .recv_deadline(self.deadline)
        {
            Ok(reply) => reply,
            Err(mpmc::RecvTimeoutError::Timeout) => {
                bail!("timed out waiting for response for {}", self.method);
            }
            Err(mpmc::RecvTimeoutError::Disconnected) => {
                bail!(
                    "channel disconnected while waiting for response for {}",
                    self.method
                );
            }
        };
        if received_at > self.deadline {
            bail!("timed out waiting for response for {}", self.method);
        }
        let value = result
            .with_context(|| format!("send failed for {}", self.method))?;
        log::debug!("got response for {} ({})", self.method, self.call_id);
        serde_json::from_str(value.get())
            .with_context(|| format!("decoding response for {}", self.method))
    }
}

type Reply = (Instant, Result<ResponsePayload>);

/// Keep the received buffer alive until the caller decodes its result.
#[derive(Debug)]
struct ResponsePayload {
    text: Utf8Bytes,
    range: Range<usize>,
}

impl ResponsePayload {
    // Locate the borrowed payload before moving its backing frame into the reply.
    fn range_in_frame(
        text: &str,
        result: Option<&RawValue>,
    ) -> Option<Range<usize>> {
        result.map(|result| {
            // Borrowed RawValue is a subslice of this frame, so these are
            // UTF-8 byte offsets, including any original JSON escapes.
            let start = result.get().as_ptr() as usize - text.as_ptr() as usize;
            start..start + result.get().len()
        })
    }

    fn from_frame(text: Utf8Bytes, range: Option<Range<usize>>) -> Self {
        match range {
            Some(range) => Self { text, range },
            None => Self {
                text: Utf8Bytes::from_static("null"),
                range: 0..4,
            },
        }
    }

    fn get(&self) -> &str {
        &self.text.as_str()[self.range.clone()]
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
    fn request<T: Command>(
        &self,
        cmd: T,
        session_id: Option<&SessionId>,
    ) -> Result<PendingResponse<T>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let call_id = self.next_call_id();
        let method = cmd.identifier();
        log::debug!(
            "sending {} ({}), session={:?}",
            method,
            call_id,
            session_id
        );
        let call = MethodCall {
            id: call_id,
            method: method.clone(),
            session_id: session_id.map(|id| id.inner().into()),
            params: serde_json::to_value(&cmd)?,
        };
        let (reply_tx, reply_rx) = mpmc::bounded(1);
        self.worker_tx
            .send_deadline(
                WorkerRequest::Send {
                    call,
                    reply_tx: Some(reply_tx),
                },
                deadline,
            )
            .with_context(|| format!("send failed for {method}"))?;
        self.commands_waker.wake()?;
        Ok(PendingResponse {
            call_id,
            method,
            deadline,
            reply_rx,
            command: PhantomData,
        })
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
        reply_tx: Option<mpmc::Sender<Reply>>,
    },
    Close,
}

type CallsInFlight = HashMap<CallId, Option<mpmc::Sender<Reply>>>;

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
                let reply_error = |reply_tx: &Option<mpmc::Sender<Reply>>,
                                   error| {
                    if let Some(tx) = reply_tx {
                        let _ = tx.send((Instant::now(), Err(error)));
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
            // Borrow payloads until routing decides whether a consumer needs them.
            // RawValue validates JSON without allocating a Value tree.
            #[derive(Deserialize)]
            struct Envelope<'a> {
                id: Option<CallId>,
                #[serde(default, borrow)]
                method: Cow<'a, str>,
                #[serde(borrow)]
                result: Option<&'a RawValue>,
                error: Option<cdp_types::Error>,
            }
            let response: Envelope<'_> = serde_json::from_str(text_str)
                .map_err(|err| {
                    anyhow!(
                        "failed to parse ws text frame '{}': {err}",
                        text_str
                    )
                })?;
            if let Some(id) = response.id {
                if let Some(error) = &response.error {
                    log::debug!(
                        "received command error from websocket: call={}, error={}",
                        id,
                        error,
                    );
                }
                if let Some(reply_tx) = calls_in_flight.remove(&id) {
                    // Requests expect a response; posts discard it.
                    if let Some(reply_tx) = reply_tx {
                        if let Some(err) = response.error {
                            let _ = reply_tx
                                .send((Instant::now(), Err(err.into())));
                        } else {
                            let range = ResponsePayload::range_in_frame(
                                text_str,
                                response.result,
                            );
                            let result =
                                ResponsePayload::from_frame(text, range);
                            let _ = reply_tx.send((Instant::now(), Ok(result)));
                        }
                    } else if response.error.is_none()
                        && log::log_enabled!(log::Level::Debug)
                    {
                        #[derive(Deserialize)]
                        struct PostResult<'a> {
                            #[serde(rename = "exceptionDetails", borrow)]
                            exception_details: Option<&'a RawValue>,
                        }
                        let exception_details =
                            response.result.and_then(|result| {
                                serde_json::from_str::<PostResult<'_>>(
                                    result.get(),
                                )
                                .ok()
                                .and_then(|result| result.exception_details)
                            });
                        log::debug!(
                            "received response for post {}: exception_details={:?}",
                            id,
                            exception_details.map(RawValue::get),
                        );
                    }
                } else {
                    bail!(
                        "got unexpected response ({}) with no corresponding request in flight",
                        id
                    );
                }
            } else {
                let method = response.method.as_ref();
                if method.is_empty() {
                    bail!("event is missing its method");
                }
                let mut subscribers = subscribers.lock().map_err(|_| {
                    anyhow!("failed to acquire lock for subscribers")
                })?;
                if !subscribers.is_interested_in(method) {
                    return Ok(());
                }
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
                subscribers.dispatch(event);
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
    use std::time::{Duration, Instant};

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

    fn scripted_server(
        script: impl FnOnce(&mut WebSocket<TcpStream>) + Send + 'static,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            script(&mut ws);
            while let Ok(message) = ws.read() {
                if matches!(message, WsMessage::Close(_)) {
                    break;
                }
            }
        });
        (format!("ws://{address}"), server)
    }

    fn read_command(ws: &mut WebSocket<TcpStream>) -> json::Value {
        let message = ws.read().unwrap();
        json::from_str(message.to_text().unwrap()).unwrap()
    }

    fn respond(
        ws: &mut WebSocket<TcpStream>,
        command: &json::Value,
        result: json::Value,
    ) {
        ws.send(WsMessage::text(
            json::json!({"id": command["id"], "result": result}).to_string(),
        ))
        .unwrap();
    }

    fn test_command() -> TestCommand {
        TestCommand {
            payload: "small".into(),
        }
    }

    #[derive(Debug, Serialize)]
    struct NumberCommand {}

    impl Method for NumberCommand {
        fn identifier(&self) -> MethodId {
            Cow::Borrowed("Test.number")
        }
    }

    impl Command for NumberCommand {
        type Response = u64;
    }

    #[test]
    fn response_payload_stays_raw_until_typed_wait() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let (tx, rx) = mpmc::bounded(1);
        let id = CallId::new(7);
        let mut calls = HashMap::from([(id, Some(tx))]);
        // This unused number cannot be represented by Value, but typed decoding
        // can skip it. Preserve the original payload through the worker queue.
        let payload = r#"{ "received": true, "unused": 1e999 }"#;
        let text = format!(r#"{{"result":{payload},"id":7}}"#);
        let payload_ptr = text[10..].as_ptr();
        handle_message(WsMessage::text(text), &mut calls, &subscribers)
            .unwrap();
        assert!(calls.is_empty());
        let reply = rx.recv().unwrap();
        assert_eq!(reply.1.as_ref().unwrap().get(), payload);
        assert_eq!(reply.1.as_ref().unwrap().get().as_ptr(), payload_ptr);
        let (tx, rx) = mpmc::bounded(1);
        tx.send(reply).unwrap();
        let pending = PendingResponse::<TestCommand> {
            call_id: id,
            method: Cow::Borrowed("Test.command"),
            deadline: Instant::now() + Duration::from_secs(1),
            reply_rx: rx,
            command: PhantomData,
        };
        assert_eq!(pending.wait().unwrap(), TestResponse { received: true });
    }

    #[test]
    fn response_ranges_preserve_unicode_escapes_and_null_defaults() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        for (text, expected) in [
            (
                r#"{"sessionId":"ø","id":7,"result":{"data":"é\n\""}}"#,
                r#"{"data":"é\n\""}"#,
            ),
            (r#"{"result":"é","id":7}"#, r#""é""#),
            (r#"{"id":7,"result":null}"#, "null"),
            (r#"{"id":7}"#, "null"),
        ] {
            let (tx, rx) = mpmc::bounded(1);
            let mut calls = HashMap::from([(CallId::new(7), Some(tx))]);
            handle_message(WsMessage::text(text), &mut calls, &subscribers)
                .unwrap();
            let response = rx.recv().unwrap().1.unwrap();
            assert_eq!(response.get(), expected);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(response.get())
                    .unwrap(),
                serde_json::from_str::<serde_json::Value>(expected).unwrap(),
            );
        }
    }

    #[test]
    fn post_response_skips_unused_payload() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let mut calls = HashMap::from([(CallId::new(7), None)]);
        handle_message(
            WsMessage::text(r#"{"result":{"unused":1e999},"id":7}"#),
            &mut calls,
            &subscribers,
        )
        .unwrap();
        assert!(calls.is_empty());
        assert!(
            handle_message(
                WsMessage::text(r#"{"result":{},"id":8}"#),
                &mut calls,
                &subscribers,
            )
            .unwrap_err()
            .to_string()
            .contains("no corresponding request")
        );
    }

    #[test]
    fn selected_events_keep_order_session_and_raw_params() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let events = Events {
            subscribers: subscribers.clone(),
        };
        let selected = events.methods([
            Cow::Borrowed("Test.first"),
            Cow::Borrowed("Test.second"),
            Cow::Borrowed("Test.first"),
        ]);
        let mut calls = HashMap::new();
        for method in ["Test.first", "Test.ignored", "Test.second"] {
            handle_message(
                WsMessage::text(format!(
                    r#"{{"params":{{"unused":1e999}},"sessionId":"session","method":"{method}"}}"#
                )),
                &mut calls,
                &subscribers,
            ).unwrap();
        }
        assert_eq!(selected.len(), 2);
        for method in ["Test.first", "Test.second"] {
            let event = selected.recv().unwrap();
            assert_eq!(event.method, method);
            assert_eq!(event.session_id.as_deref(), Some("session"));
            assert_eq!(event.params.get(), r#"{"unused":1e999}"#);
        }
        events.close();
        assert!(matches!(
            selected.try_recv(),
            Err(mpmc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn uninterested_events_skip_payload_decoding() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let mut calls = HashMap::new();
        // The envelope is valid JSON but has no typed event payload.
        handle_message(
            WsMessage::text(r#"{"method":"Test.ignored"}"#),
            &mut calls,
            &subscribers,
        )
        .unwrap();
        assert!(
            handle_message(
                WsMessage::text(
                    r#"{"method":"Test.ignored","params":invalid}"#
                ),
                &mut calls,
                &subscribers,
            )
            .is_err()
        );
    }

    #[test]
    fn pending_requests_route_out_of_order_heterogeneous_responses() {
        let (url, server) = scripted_server(|ws| {
            // No response is sent until all requests arrive. A serialized
            // implementation cannot make progress through this barrier.
            let first = read_command(ws);
            let second = read_command(ws);
            assert_eq!(first["method"], "Test.command");
            assert_eq!(second["method"], "Test.number");
            assert_eq!(first["sessionId"], "test-session");
            assert_eq!(second["sessionId"], "test-session");
            respond(ws, &second, json::json!(42));
            respond(ws, &first, json::json!({"received": true}));
        });
        let connection = Connection::connect(url).unwrap();
        let session = SessionId::from("test-session".to_owned());
        let first = connection.request(test_command(), Some(&session)).unwrap();
        let second = connection
            .request(NumberCommand {}, Some(&session))
            .unwrap();
        let responses = (first.wait().unwrap(), second.wait().unwrap());
        assert_eq!(responses, (TestResponse { received: true }, 42));
        connection.close().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn dropped_and_failed_requests_do_not_disrupt_other_responses() {
        let (url, server) =
            scripted_server(|ws| {
                let dropped = read_command(ws);
                let failed = read_command(ws);
                let survivor = read_command(ws);
                respond(ws, &dropped, json::json!({"received": true}));
                ws.send(WsMessage::text(json::json!({
                "id": failed["id"],
                "error": {"code": -32000, "message": "injected failure"},
            }).to_string())).unwrap();
                respond(ws, &survivor, json::json!(42));
                let subsequent = read_command(ws);
                respond(ws, &subsequent, json::json!({"received": true}));
            });
        let connection = Connection::connect(url).unwrap();
        drop(connection.request(test_command(), None).unwrap());
        let failed = connection.request(test_command(), None).unwrap();
        let survivor = connection.request(NumberCommand {}, None).unwrap();
        let error = format!("{:#}", failed.wait().unwrap_err());
        assert!(error.contains("Test.command"));
        assert!(error.contains("injected failure"));
        assert_eq!(survivor.wait().unwrap(), 42);
        assert_eq!(
            connection.send(test_command(), None).unwrap(),
            TestResponse { received: true }
        );
        connection.close().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn response_deadlines_start_at_submission_not_collection() {
        let (release_tx, release_rx) = mpmc::bounded(1);
        let (url, server) = scripted_server(move |ws| {
            let early = read_command(ws);
            let late = read_command(ws);
            let _unanswered = read_command(ws);
            respond(ws, &early, json::json!({"received": true}));
            let barrier = read_command(ws);
            respond(ws, &barrier, json::json!(1));
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            respond(ws, &late, json::json!({"received": true}));
            let barrier = read_command(ws);
            respond(ws, &barrier, json::json!(2));
        });
        let connection = Connection::connect(url).unwrap();
        let early = connection.request(test_command(), None).unwrap();
        let late = connection.request(test_command(), None).unwrap();
        let unanswered = connection.request(test_command(), None).unwrap();
        // Receiving this response proves the worker delivered the early reply.
        assert_eq!(connection.send(NumberCommand {}, None).unwrap(), 1);
        thread::sleep(Duration::from_millis(5100));
        release_tx.send(()).unwrap();
        // Likewise, ensure the late reply is queued before collecting it.
        assert_eq!(connection.send(NumberCommand {}, None).unwrap(), 2);
        assert_eq!(early.wait().unwrap(), TestResponse { received: true });
        let start = Instant::now();
        assert!(late.wait().unwrap_err().to_string().contains("timed out"));
        assert!(
            unanswered
                .wait()
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "wait restarted the timeout"
        );
        connection.close().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn closing_connection_wakes_pending_responses() {
        let (received_tx, received_rx) = mpmc::bounded(1);
        let (url, server) = scripted_server(move |ws| {
            read_command(ws);
            received_tx.send(()).unwrap();
        });
        let connection = Connection::connect(url).unwrap();
        let pending = connection.request(test_command(), None).unwrap();
        received_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        connection.close().unwrap();
        assert!(
            pending
                .wait()
                .unwrap_err()
                .to_string()
                .contains("disconnected")
        );
        server.join().unwrap();
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
