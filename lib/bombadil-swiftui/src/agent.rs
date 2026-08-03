//! Connection to the in-app Bombadil agent.
//!
//! The driver listens on an ephemeral localhost TCP port and passes the
//! address to the app under test via the `BOMBADIL_SWIFTUI_CONNECT`
//! environment variable. The agent embedded in the app connects back and
//! the two sides exchange newline-delimited JSON messages.

use std::io::{BufRead, BufReader, Read as _, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use bombadil_schema::swiftui::{ProcessExitStatus, SwiftUIAction, SwiftUINode};
use serde::{Deserialize, Serialize};

/// Environment variable the agent reads to find the driver.
pub const CONNECT_ENV_VAR: &str = "BOMBADIL_SWIFTUI_CONNECT";

const MAXIMUM_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

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
        root: SwiftUINode,
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

/// Owns a spawned app process and guarantees that it does not outlive
/// a failed launch or a dropped driver.
struct SpawnedApp {
    child: Option<Child>,
}

impl SpawnedApp {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn exit_status(&mut self) -> Result<Option<ProcessExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        Ok(child.try_wait()?.map(to_process_exit_status))
    }

    fn terminate(&mut self) -> Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child.try_wait()?.is_none() {
            child.kill().context("failed to terminate spawned app")?;
            child.wait().context("failed to reap spawned app")?;
        }
        self.child = None;
        Ok(())
    }
}

impl Drop for SpawnedApp {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

pub struct AgentConnection {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    child: Option<SpawnedApp>,
    line: Vec<u8>,
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
                Some(SpawnedApp::new(child))
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
            line: Vec::new(),
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
        if bytes.len() > MAXIMUM_MESSAGE_BYTES {
            bail!("message exceeds {MAXIMUM_MESSAGE_BYTES} bytes");
        }
        bytes.push(b'\n');
        self.writer
            .write_all(&bytes)
            .context("failed to write to agent")?;
        Ok(())
    }

    /// Read the next message, waiting up to `timeout`.
    pub fn receive(&mut self, timeout: Duration) -> Result<AgentMessage> {
        self.reader.get_ref().set_read_timeout(Some(timeout))?;
        read_line_bounded(
            &mut self.reader,
            &mut self.line,
            MAXIMUM_MESSAGE_BYTES,
        )
        .context("failed to read from agent (timeout or disconnect)")?;
        serde_json::from_slice(&self.line).context("malformed agent message")
    }

    /// Exit status of the spawned app, if it was spawned and has exited.
    pub fn exit_status(&mut self) -> Result<Option<ProcessExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        child.exit_status()
    }

    /// Like `exit_status`, but keeps polling for up to `grace`. When
    /// the app quits, the socket reports EOF a beat before the process
    /// is reapable; without the grace period a clean exit surfaces as
    /// a misleading "agent closed the connection" error.
    pub fn exit_status_within(
        &mut self,
        grace: Duration,
    ) -> Result<Option<ProcessExitStatus>> {
        if self.child.is_none() {
            return Ok(None);
        }
        let deadline = Instant::now() + grace;
        loop {
            if let Some(status) = self.exit_status()? {
                return Ok(Some(status));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            std::thread::sleep(remaining.min(Duration::from_millis(20)));
        }
    }

    pub fn terminate(&mut self) -> Result<()> {
        if let Some(child) = self.child.as_mut() {
            child.terminate()?;
        }
        Ok(())
    }
}

fn read_line_bounded(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    maximum_bytes: usize,
) -> Result<()> {
    line.clear();
    let read_limit = maximum_bytes
        .checked_add(2)
        .context("maximum message size is too large")?;
    let read = reader.take(read_limit as u64).read_until(b'\n', line)?;
    if read == 0 {
        bail!("agent closed the connection");
    }
    if line.last() != Some(&b'\n') {
        if line.len() > maximum_bytes {
            bail!("message exceeds {maximum_bytes} bytes");
        }
        bail!("agent closed the connection during a message");
    }
    line.pop();
    if line.len() > maximum_bytes {
        bail!("message exceeds {maximum_bytes} bytes");
    }
    Ok(())
}

impl Drop for AgentConnection {
    fn drop(&mut self) {
        let _ = self.terminate();
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
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| anyhow!("connect timeout is too large"))?;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::read_line_bounded;

    #[test]
    fn reads_a_bounded_protocol_line() {
        let mut reader = Cursor::new(b"12345678\nnext\n");
        let mut line = Vec::new();

        read_line_bounded(&mut reader, &mut line, 8).unwrap();

        assert_eq!(line, b"12345678");
    }

    #[test]
    fn rejects_an_oversized_protocol_line_without_reading_it_all() {
        let mut reader = Cursor::new(vec![b'x'; 1_024]);
        let mut line = Vec::new();

        let error = read_line_bounded(&mut reader, &mut line, 8).unwrap_err();

        assert!(error.to_string().contains("exceeds 8 bytes"));
        assert_eq!(line.len(), 10);
    }

    #[test]
    fn rejects_a_truncated_protocol_line() {
        let mut reader = Cursor::new(b"partial");
        let mut line = Vec::new();

        let error = read_line_bounded(&mut reader, &mut line, 8).unwrap_err();

        assert!(error.to_string().contains("during a message"));
    }
}
