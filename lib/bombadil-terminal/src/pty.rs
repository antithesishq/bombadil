use std::{
    ffi::OsStr,
    io::{Read, Write},
    sync::mpsc,
    time::Instant,
};

use anyhow::Result;
use bombadil_schema::terminal::TerminalSize;
use bytes::Bytes;
use portable_pty::{
    Child, CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize,
    PtySystem,
};

pub struct PtyProcess {
    child: Option<Box<dyn Child + Send + Sync>>,
    input_tx: Option<mpsc::SyncSender<Vec<u8>>>,
    master: Option<Box<dyn MasterPty + Send + 'static>>,
    reader: Option<std::thread::JoinHandle<()>>,
    writer: Option<std::thread::JoinHandle<()>>,
    input_dropped_at: Option<Instant>,
}

const INPUT_QUEUE_CAPACITY: usize = 128;

impl PtyProcess {
    pub fn spawn<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(
        size: TerminalSize,
        command: &str,
        args: I,
    ) -> Result<(Self, PtyOutput)> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: size.rows,
            cols: size.columns,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        cmd.cwd(std::env::current_dir()?);
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let (output_write, output_read) = mpsc::sync_channel::<Bytes>(64);
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("couldn't clone master reader");
        // The PTY reader uses sync IO and must be run on a dedicated OS thread.
        let reader = std::thread::Builder::new()
            .name("bombadil-pty-reader".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = Bytes::copy_from_slice(&buffer[..n]);
                            if output_write.send(chunk).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            log::warn!("PTY read error: {error}");
                            break;
                        }
                    }
                }
            })?;

        let mut input_write = pair.master.take_writer()?;
        let (input_tx, input_rx) =
            mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE_CAPACITY);

        // Writes to the master go through this dedicated thread so the
        // driver never blocks on the pty's input buffer.
        let writer = std::thread::Builder::new()
            .name("bombadil-pty-writer".to_string())
            .spawn(move || {
                while let Ok(bytes) = input_rx.recv() {
                    if let Err(error) = input_write.write_all(&bytes) {
                        log::warn!("PTY write error: {error}");
                        break;
                    }
                    if let Err(error) = input_write.flush() {
                        log::warn!("PTY flush error: {error}");
                        break;
                    }
                }
            })?;

        Ok((
            Self {
                child: Some(child),
                input_tx: Some(input_tx),
                master: Some(pair.master),
                reader: Some(reader),
                writer: Some(writer),
                input_dropped_at: None,
            },
            PtyOutput { output_read },
        ))
    }

    pub fn write(&mut self, input: &[u8]) {
        let Some(tx) = self.input_tx.as_ref() else {
            return;
        };
        match tx.try_send(input.to_vec()) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(dropped)) => {
                self.input_dropped_at = Some(Instant::now());
                log::warn!(
                    "PTY input queue full, dropped {} bytes",
                    dropped.len()
                );
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                log::warn!("PTY writer thread has exited");
            }
        }
    }

    pub fn last_input_dropped(&self) -> Option<Instant> {
        self.input_dropped_at
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<()> {
        let master = self
            .master
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("pty is not running"))?;
        master.resize(PtySize {
            cols: size.columns,
            rows: size.rows,
            ..Default::default()
        })?;
        Ok(())
    }

    pub fn wait(mut self) -> Result<ExitStatus> {
        let mut child = self
            .child
            .take()
            .ok_or_else(|| anyhow::anyhow!("pty is not running"))?;
        let status = child.wait()?;
        self.master.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        Ok(status)
    }

    pub fn kill(&mut self) {
        self.cleanup();
    }

    pub fn exit_status(&mut self) -> Result<Option<ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        Ok(child.try_wait()?)
    }

    fn cleanup(&mut self) {
        drop(self.input_tx.take());
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.master.take();
        // Detach reader and writer rather than joining, and let them exit
        // on their own.
        drop(self.reader.take());
        drop(self.writer.take());
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        self.cleanup();
    }
}

pub struct PtyOutput {
    output_read: mpsc::Receiver<Bytes>,
}

pub enum ReadResult {
    Chunk(Bytes),
    Empty,
    Ended,
}

impl PtyOutput {
    /// Block for up to `timeout` waiting for the next chunk. Returns
    /// `Empty` only if no chunk arrived in that window — used by the
    /// driver to drain output to quiescence.
    pub fn read_until(&mut self, timeout: std::time::Duration) -> ReadResult {
        use ReadResult::*;
        match self.output_read.recv_timeout(timeout) {
            Ok(bytes) => Chunk(bytes),
            Err(mpsc::RecvTimeoutError::Timeout) => Empty,
            Err(mpsc::RecvTimeoutError::Disconnected) => Ended,
        }
    }
}
