use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use boa_engine::JsValue;
use bombadil::driver::{
    DriverEvent, InterfaceDriver, InterfaceSession, RunId, RunState,
};
use bombadil::specification::domain::Snapshot;
use bombadil::specification::extract::SnapshotExtractor;
use bombadil_schema::Time;
use serde_json as json;

use crate::action::{ExternalAction, ExternalActionTemplate};
use crate::protocol::{
    BombadilMessage, DriverMessage, PROTOCOL_VERSION, STALL_TIMEOUT_DEFAULT_MS,
};

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
            // Its own process group, so that stopping the session also stops
            // whatever the driver started.
            .process_group(0)
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
            stall_timeout: Duration::from_millis(STALL_TIMEOUT_DEFAULT_MS),
            last_timestamp: None,
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
    stall_timeout: Duration,
    last_timestamp: Option<SystemTime>,
}

#[derive(Debug)]
pub struct ExternalState {
    pub timestamp: SystemTime,
    pub value: json::Value,
    pub screenshot_png: Option<Arc<Vec<u8>>>,
    /// `false` if the driver reported the state before the app settled.
    pub settled: bool,
    /// Why the driver didn't perform the action before this state.
    pub not_applied: Option<String>,
}

impl RunState for ExternalState {
    fn timestamp(&self) -> Time {
        Time::from_system_time(self.timestamp)
    }
}

impl ExternalSession {
    fn handshake(&mut self) -> Result<()> {
        match self.messages.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok(Ok(DriverMessage::Hello {
                protocol,
                name,
                stall_timeout_ms,
            })) => {
                if protocol != PROTOCOL_VERSION {
                    bail!(
                        "driver `{name}` speaks protocol version {protocol}, \
                         but Bombadil speaks version {PROTOCOL_VERSION}"
                    );
                }
                if let Some(millis) = stall_timeout_ms {
                    self.stall_timeout = Duration::from_millis(millis);
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

    /// Stops what is left of the driver's process group: SIGTERM first,
    /// then SIGKILL after `TERMINATE_TIMEOUT`. Returns once the group is
    /// gone, so that what its processes held (devices, locks, ports) is
    /// free for the next session.
    fn stop_process_group(&mut self) {
        let group = -(self.child.id() as libc::pid_t);
        for signal in [libc::SIGTERM, libc::SIGKILL] {
            // SAFETY: `kill` has no memory safety requirements.
            unsafe { libc::kill(group, signal) };
            let deadline = Instant::now() + TERMINATE_TIMEOUT;
            loop {
                // Reap the driver itself; as a zombie it still counts as a
                // member of the group.
                let _ = self.child.try_wait();
                // SAFETY: as above; signal 0 only checks for existence.
                if unsafe { libc::kill(group, 0) } != 0 {
                    return;
                }
                if Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        log::warn!("driver process group {} did not exit", -group);
    }
}

impl Drop for ExternalSession {
    fn drop(&mut self) {
        self.stop_process_group();
    }
}

impl InterfaceSession for ExternalSession {
    type Action = ExternalAction;
    type ActionTemplate = ExternalActionTemplate;
    type State = ExternalState;

    fn stall_timeout(&self) -> Duration {
        self.stall_timeout
    }

    fn terminate(&mut self) -> Result<()> {
        if self.stdin.is_some() {
            // The driver may already have exited, in which case there is
            // no one left to tell.
            let _ = self.send(&BombadilMessage::Terminate);
            self.stdin = None;
        }
        let deadline = Instant::now() + TERMINATE_TIMEOUT;
        while self.child.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                log::warn!(
                    "driver did not exit within {TERMINATE_TIMEOUT:?}, stopping it"
                );
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        // Children the driver left behind would otherwise outlive Bombadil.
        self.stop_process_group();
        Ok(())
    }

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<ExternalState>> {
        let error = |error: anyhow::Error| DriverEvent::Error(Arc::new(error));
        Some(match self.messages.recv_timeout(timeout) {
            Ok(Ok(DriverMessage::State {
                state,
                screenshot_png,
                settled,
                not_applied,
                timestamp_us,
            })) => {
                let screenshot_png = match screenshot_png
                    .map(|encoded| {
                        let bytes = STANDARD
                            .decode(encoded)
                            .context("invalid base64 in screenshot_png")?;
                        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                            bail!("screenshot_png is not a PNG image");
                        }
                        if bytes.len()
                            > bombadil_trace::RECORD_PAYLOAD_BYTES_MAX
                        {
                            bail!(
                                "screenshot_png exceeds the trace payload limit"
                            );
                        }
                        Ok(Arc::new(bytes))
                    })
                    .transpose()
                {
                    Ok(image) => image,
                    Err(cause) => return Some(error(cause)),
                };
                let timestamp =
                    timestamp_us.map_or_else(SystemTime::now, |micros| {
                        SystemTime::UNIX_EPOCH + Duration::from_micros(micros)
                    });
                if let Some(last) = self.last_timestamp
                    && timestamp < last
                {
                    return Some(error(anyhow!(
                        "driver sent a state timestamped before the previous one"
                    )));
                }
                if not_applied.is_some() && self.last_timestamp.is_none() {
                    return Some(error(anyhow!(
                        "driver sent `not_applied` with its first state"
                    )));
                }
                self.last_timestamp = Some(timestamp);
                if !settled {
                    log::info!(
                        "driver reported a state before the app settled"
                    );
                }
                if let Some(reason) = &not_applied {
                    log::info!("driver did not apply the action: {reason}");
                }
                DriverEvent::StateChanged(Arc::new(ExternalState {
                    timestamp,
                    value: state,
                    screenshot_png,
                    settled,
                    not_applied,
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
