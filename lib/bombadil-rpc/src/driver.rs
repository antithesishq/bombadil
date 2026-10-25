use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use boa_engine::JsValue;
use bombadil::driver::{
    DriverEvent, InterfaceDriver, InterfaceSession, RunId, RunState,
};
use bombadil::specification::domain::Snapshot;
use bombadil::specification::extract::SnapshotExtractor;
use bombadil_schema::Time;
use serde_json as json;

use crate::action::{ExternalAction, ExternalActionTemplate};
use crate::protocol::{BombadilMessage, DriverMessage, PROTOCOL_VERSION};

/// How long a driver may take to start and send `hello`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a driver may take to exit after `terminate` before it's killed.
const TERMINATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs an external driver program, one process per session.
pub struct ExternalDriver {
    pub specification_bundle: Arc<str>,
    pub program: String,
    pub arguments: Vec<String>,
}

impl InterfaceDriver for ExternalDriver {
    type Session = ExternalSession;

    fn new_session(&self, run_id: RunId) -> Result<ExternalSession> {
        let extractor =
            SnapshotExtractor::initialize(&self.specification_bundle)?;
        let mut child = Command::new(&self.program)
            .args(&self.arguments)
            .env("BOMBADIL_RUN_ID", run_id.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("failed to start `{}`", self.program))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");

        let (sender, messages) = mpsc::channel();
        thread::Builder::new()
            .name("bombadil-rpc-reader".to_string())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let message =
                        line.map_err(anyhow::Error::from).and_then(|line| {
                            json::from_str::<DriverMessage>(&line).with_context(
                                || {
                                    format!(
                                        "invalid message from driver: {line}"
                                    )
                                },
                            )
                        });
                    let failed = message.is_err();
                    if sender.send(message).is_err() || failed {
                        break;
                    }
                }
            })?;

        let mut session = ExternalSession {
            child,
            stdin,
            messages,
            extractor,
        };
        session.handshake()?;
        Ok(session)
    }
}

pub struct ExternalSession {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<Result<DriverMessage>>,
    extractor: SnapshotExtractor,
}

#[derive(Debug)]
pub struct ExternalState {
    pub timestamp: SystemTime,
    pub value: json::Value,
}

impl RunState for ExternalState {
    fn timestamp(&self) -> Time {
        Time::from_system_time(self.timestamp)
    }
}

impl ExternalSession {
    fn handshake(&mut self) -> Result<()> {
        match self.messages.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok(Ok(DriverMessage::Hello { protocol, name })) => {
                if protocol != PROTOCOL_VERSION {
                    bail!(
                        "driver `{name}` speaks protocol version {protocol}, \
                         but Bombadil speaks version {PROTOCOL_VERSION}"
                    );
                }
                log::info!("connected to external driver `{name}`");
                Ok(())
            }
            Ok(Ok(message)) => {
                bail!("expected `hello` from driver, got {message:?}")
            }
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                bail!("driver sent no `hello` within {HANDSHAKE_TIMEOUT:?}")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("driver exited before `hello` ({})", self.exit_status())
            }
        }
    }

    fn send(&mut self, message: &BombadilMessage) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("driver session is terminated"))?;
        let mut line = json::to_vec(message)?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .and_then(|()| stdin.flush())
            .with_context(|| {
                format!("failed to write to driver ({})", self.exit_status())
            })
    }

    fn exit_status(&mut self) -> String {
        match self.child.try_wait() {
            Ok(Some(status)) => status.to_string(),
            Ok(None) => "still running".to_string(),
            Err(error) => error.to_string(),
        }
    }
}

impl Drop for ExternalSession {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl InterfaceSession for ExternalSession {
    type Action = ExternalAction;
    type ActionTemplate = ExternalActionTemplate;
    type State = ExternalState;

    const STALL_TIMEOUT: Duration = Duration::from_secs(30);

    fn terminate(&mut self) -> Result<()> {
        if self.stdin.is_some() {
            // The driver may already have exited, in which case there is
            // no one left to tell.
            let _ = self.send(&BombadilMessage::Terminate);
            self.stdin = None;
        }
        let deadline = Instant::now() + TERMINATE_TIMEOUT;
        while Instant::now() < deadline {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
        }
        log::warn!("driver did not exit within {TERMINATE_TIMEOUT:?}, killing");
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<ExternalState>> {
        let error = |error: anyhow::Error| DriverEvent::Error(Arc::new(error));
        Some(match self.messages.recv_timeout(timeout) {
            Ok(Ok(DriverMessage::State { state })) => {
                DriverEvent::StateChanged(Arc::new(ExternalState {
                    timestamp: SystemTime::now(),
                    value: state,
                }))
            }
            Ok(Ok(DriverMessage::Error { message })) => error(anyhow!(message)),
            Ok(Ok(message)) => {
                error(anyhow!("unexpected message from driver: {message:?}"))
            }
            Ok(Err(cause)) => error(cause),
            Err(mpsc::RecvTimeoutError::Timeout) => DriverEvent::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => error(anyhow!(
                "driver closed its stdout ({})",
                self.exit_status()
            )),
        })
    }

    fn apply(
        &mut self,
        action: ExternalAction,
        _state: Arc<ExternalState>,
    ) -> Result<()> {
        self.send(&BombadilMessage::Apply { action: action.0 })
    }

    fn extract_snapshots(
        &mut self,
        state: Arc<ExternalState>,
        _last_action: Option<&ExternalAction>,
    ) -> Result<Vec<Snapshot>> {
        self.extractor.run(state.timestamp(), |context| {
            JsValue::from_json(&state.value, context)
                .map_err(|error| anyhow!("state is not valid JS: {error}"))
        })
    }

    fn state_timestamp(state: &ExternalState) -> SystemTime {
        state.timestamp
    }
}
