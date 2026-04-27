use bombadil::tree::Tree;
use rand::{self};
use std::{io::Write, process::exit, time::Duration};

use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
};

use anyhow::Result;
use portable_pty::{
    Child, CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize,
    PtySystem,
};
use tokio::{join, time::timeout};
use tokio::{sync::mpsc::channel, time::sleep};

#[tokio::main]
async fn main() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions {
        cols: 120,
        rows: 24,
        max_scrollback: 10_000,
    })?;
    let (mut process, mut output) =
        PtyProcess::spawn("tetris", &["--nomenu"]).await?;
    let mut rng = rand::rng();

    sleep(Duration::from_millis(200)).await;

    loop {
        match timeout(Duration::from_micros(1), output.read()).await {
            Ok(result) => {
                if let Some(output) = result? {
                    terminal.vt_write(&output.into_bytes());

                    let mut render_state = RenderState::new()?;
                    let mut rows = RowIterator::new()?;
                    let mut cells = CellIterator::new()?;

                    let snapshot = render_state.update(&terminal)?;
                    let mut row_iter = rows.update(&snapshot)?;

                    let mut output = String::with_capacity(120 * 40 * 4);
                    while let Some(row) = row_iter.next() {
                        let mut cell_iter = cells.update(row)?;
                        while let Some(cell) = cell_iter.next() {
                            let graphemes: Vec<char> = cell.graphemes()?;
                            if graphemes.is_empty() {
                                output.push(' ');
                            } else {
                                for grapheme in graphemes {
                                    output.push(grapheme);
                                }
                            }
                        }
                        output.push('\n');
                    }

                    // Clear screen and rerender
                    print!("\x1B[2J\x1B[1;1H{output}");

                    let key = random_key(&mut rng)?;
                    process.write(key.as_bytes());
                } else {
                    let status = process.wait().await?;
                    println!(
                        "process finished with code {}",
                        status.exit_code()
                    );
                    exit(status.exit_code() as i32);
                }
            }
            Err(_elapsed) => {
                let key = random_key(&mut rng)?;
                process.write(key.as_bytes());
            }
        }
    }
}

fn random_key(rng: &mut impl rand::Rng) -> Result<&'static str> {
    let tree = Tree::Branch {
        branches: vec![
            (1, Tree::Leaf { value: "\r" }),
            (1, Tree::Leaf { value: " " }),
            (
                1000,
                Tree::Branch {
                    branches: vec![
                        (1, Tree::Leaf { value: "\x1B[A" }),
                        (1, Tree::Leaf { value: "\x1B[B" }),
                        (1, Tree::Leaf { value: "\x1B[C" }),
                        (1, Tree::Leaf { value: "\x1B[D" }),
                    ],
                },
            ),
        ],
    };
    Ok(tree.pick(rng)?)
}

struct PtyProcess {
    child: Box<dyn Child + Send + Sync>,
    input_write: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send + 'static>,
    reader: tokio::task::JoinHandle<()>,
}

impl PtyProcess {
    async fn spawn(command: &str, args: &[&str]) -> Result<(Self, PtyOutput)> {
        let pty_system = NativePtySystem::default();

        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        let child = pair.slave.spawn_command(cmd).unwrap();

        // Release any handles owned by the slave: we don't need it now
        // that we've spawned the child.
        drop(pair.slave);

        // Read the output in another thread.
        // This is important because it is easy to encounter a situation
        // where read/write buffers fill and block either your process
        // or the spawned process.
        let (output_write, output_read) = channel(1);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let reader = tokio::spawn(async move {
            let mut buffer = [0u8; 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let output = String::from_utf8_lossy(&buffer[..n]);
                        output_write.send(output.into()).await.unwrap();
                    }
                    Err(e) => {
                        eprintln!("Error reading from PTY: {}", e);
                        break;
                    }
                }
            }
        });

        // Obtain the writer.
        // When the writer is dropped, EOF will be sent to
        // the program that was spawned.
        // It is important to take the writer even if you don't
        // send anything to its stdin so that EOF can be
        // generated, otherwise you risk deadlocking yourself.
        let writer = pair.master.take_writer()?;

        Ok((
            Self {
                child,
                master: pair.master,
                input_write: writer,
                reader,
            },
            PtyOutput { output_read },
        ))
    }

    pub fn write(&mut self, input: &[u8]) {
        self.input_write.write_all(input).expect("write failed");
    }

    pub async fn wait(mut self) -> Result<ExitStatus> {
        // Wait for the child to complete
        let status = self.child.wait()?;

        // Take care to drop the master after our processes are
        // done, as some platforms get unhappy if it is dropped
        // sooner than that.
        drop(self.master);

        join!(self.reader).0?;

        Ok(status)
    }
}

struct PtyOutput {
    output_read: tokio::sync::mpsc::Receiver<String>,
}

impl PtyOutput {
    pub async fn read(&mut self) -> Result<Option<String>> {
        Ok(self.output_read.recv().await)
    }
}
