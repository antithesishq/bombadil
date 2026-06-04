use std::{
    ffi::OsStr,
    io::{Read, Write},
};

use anyhow::Result;
use bombadil_schema::TerminalSize;
use bytes::Bytes;
use portable_pty::{
    Child, CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize,
    PtySystem,
};
use tokio::sync::mpsc::channel;

pub struct PtyProcess {
    child: Box<dyn Child + Send + Sync>,
    input_write: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send + 'static>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl PtyProcess {
    pub async fn spawn<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(
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
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let (output_write, output_read) = channel::<Bytes>(64);
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
                            if output_write.blocking_send(chunk).is_err() {
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

        Ok((
            Self {
                child,
                input_write: pair.master.take_writer()?,
                master: pair.master,
                reader: Some(reader),
            },
            PtyOutput { output_read },
        ))
    }

    pub fn write(&mut self, input: &[u8]) {
        if let Err(error) = self.input_write.write_all(input) {
            log::warn!("PTY write error: {error}");
        }
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<()> {
        self.master.resize(PtySize {
            cols: size.columns,
            rows: size.rows,
            ..Default::default()
        })?;
        Ok(())
    }

    pub async fn wait(mut self) -> Result<ExitStatus> {
        let status = self.child.wait()?;
        drop(self.master);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        Ok(status)
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill();
    }

    pub fn is_terminated(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }
}

pub struct PtyOutput {
    output_read: tokio::sync::mpsc::Receiver<Bytes>,
}

impl PtyOutput {
    pub async fn read(&mut self) -> Result<Option<Bytes>> {
        Ok(self.output_read.recv().await)
    }

    pub fn try_read(&mut self) -> Option<Bytes> {
        self.output_read.try_recv().ok()
    }
}
