//! Connection to the in-app Bombadil agent.
//!
//! The driver listens on an ephemeral localhost TCP port and passes the
//! address to the app under test via the `BOMBADIL_SWIFTUI_CONNECT`
//! environment variable. The agent embedded in the app connects back and
//! the two sides exchange newline-delimited JSON messages.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use bombadil_schema::swiftui::{ProcessExitStatus, SwiftUIAction, SwiftUINode};
use serde::{Deserialize, Serialize};

/// Environment variable the agent reads to find the driver.
pub const CONNECT_ENV_VAR: &str = "BOMBADIL_SWIFTUI_CONNECT";

/// Messages sent from the driver to the agent.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DriverMessage {
    /// Request the current accessibility tree. The agent replies with
    /// [`AgentMessage::State`] once the UI has settled.
    #[serde(rename_all = "camelCase")]
    GetState {
        /// How long (in milliseconds) the UI must stay unchanged before
        /// the agent considers it settled and replies.
        quiescence_millis: u64,
    },
    /// Apply an action to the UI. The agent replies with
    /// [`AgentMessage::Applied`] or [`AgentMessage::Error`].
    Apply { action: SwiftUIAction },
}

/// Messages sent from the agent to the driver.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentMessage {
    /// Sent once after connecting.
    #[serde(rename_all = "camelCase")]
    Hello {
        protocol_version: u32,
    },
    State {
        root: Option<SwiftUINode>,
    },
    Applied {},
    Error {
        message: String,
    },
}

pub const PROTOCOL_VERSION: u32 = 1;

/// What the driver should run/wait for as the system under test.
#[derive(Debug, Clone)]
pub enum SwiftUITarget {
    /// Spawn the given program and wait for its agent to connect.
    Spawn {
        program: String,
        arguments: Vec<String>,
    },
    /// Don't spawn anything; print the listen address and wait for an
    /// externally launched app (e.g. started from Xcode) to connect.
    Attach,
}

pub struct AgentConnection {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    child: Option<Child>,
    line: String,
}

impl AgentConnection {
    /// Bind a listener, launch the target (if any), and wait for the
    /// agent to connect and greet us.
    pub fn establish(
        target: &SwiftUITarget,
        connect_timeout: Duration,
    ) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .context("failed to bind agent listener")?;
        let address = listener.local_addr()?;

        let child = match target {
            SwiftUITarget::Spawn { program, arguments } => {
                let child = Command::new(program)
                    .args(arguments)
                    .env(CONNECT_ENV_VAR, address.to_string())
                    .stdin(Stdio::null())
                    .spawn()
                    .with_context(|| {
                        format!("failed to launch app: {program}")
                    })?;
                Some(child)
            }
            SwiftUITarget::Attach => {
                println!(
                    "waiting for agent; launch the app with {}={}",
                    CONNECT_ENV_VAR, address
                );
                None
            }
        };

        let stream = accept_with_timeout(&listener, connect_timeout).context(
            "the app never connected back to Bombadil. Make sure it \
                 links the BombadilAgent package and calls \
                 BombadilAgent.startIfRequested()",
        )?;
        stream.set_nodelay(true)?;

        let mut connection = Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            child,
            line: String::new(),
        };

        match connection.receive(connect_timeout)? {
            AgentMessage::Hello { protocol_version }
                if protocol_version == PROTOCOL_VERSION => {}
            AgentMessage::Hello { protocol_version } => bail!(
                "agent speaks protocol version {protocol_version}, \
                 this Bombadil speaks {PROTOCOL_VERSION}"
            ),
            other => bail!("expected hello from agent, got {other:?}"),
        }

        Ok(connection)
    }

    pub fn send(&mut self, message: &DriverMessage) -> Result<()> {
        let mut bytes = serde_json::to_vec(message)?;
        bytes.push(b'\n');
        self.writer
            .write_all(&bytes)
            .context("failed to write to agent")?;
        Ok(())
    }

    /// Read the next message, waiting up to `timeout`.
    pub fn receive(&mut self, timeout: Duration) -> Result<AgentMessage> {
        self.reader.get_ref().set_read_timeout(Some(timeout))?;
        self.line.clear();
        let read = self
            .reader
            .read_line(&mut self.line)
            .context("failed to read from agent (timeout or disconnect)")?;
        if read == 0 {
            bail!("agent closed the connection");
        }
        serde_json::from_str(&self.line).with_context(|| {
            format!("malformed agent message: {}", self.line.trim_end())
        })
    }

    /// Exit status of the spawned app, if it was spawned and has exited.
    pub fn exit_status(&mut self) -> Result<Option<ProcessExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        match child.try_wait()? {
            None => Ok(None),
            Some(status) => Ok(Some(to_process_exit_status(status))),
        }
    }

    pub fn kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn to_process_exit_status(
    status: std::process::ExitStatus,
) -> ProcessExitStatus {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|s| s.to_string())
    };
    #[cfg(not(unix))]
    let signal = None;
    ProcessExitStatus {
        signal,
        code: status.code().unwrap_or(0).unsigned_abs(),
    }
}

fn accept_with_timeout(
    listener: &TcpListener,
    timeout: Duration,
) -> Result<TcpStream> {
    // `TcpListener` has no accept timeout; poll a non-blocking listener
    // instead so a missing agent fails with a clear error.
    listener.set_nonblocking(true)?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(anyhow!(
                        "no agent connection within {timeout:?}"
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.into()),
        }
    }
}
