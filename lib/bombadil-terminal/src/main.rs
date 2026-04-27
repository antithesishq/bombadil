use bombadil::tree::Tree;
use owo_colors::OwoColorize;
use rand::{self};
use std::{io::Write, process::exit, time::Duration};

use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
    style::{PaletteIndex, RgbColor, Style, StyleColor, Underline},
};

use anyhow::Result;
use portable_pty::{
    Child, CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize,
    PtySystem,
};
use tokio::{
    join,
    sync::mpsc::channel,
    time::{Instant, sleep, timeout},
};

const COLUMN_COUNT: u16 = 120;
const ROW_COUNT: u16 = 32;
const CELL_COUNT: u16 = COLUMN_COUNT * ROW_COUNT;

#[tokio::main]
async fn main() -> Result<()> {
    let start = Instant::now();
    let mut terminal = Terminal::new(TerminalOptions {
        cols: COLUMN_COUNT,
        rows: ROW_COUNT,
        max_scrollback: 10_000,
    })?;
    let (mut process, mut output) = PtyProcess::spawn("btop", &[]).await?;
    let mut rng = rand::rng();
    let mut render_state_count = 0;
    let mut input_count = 0;

    sleep(Duration::from_millis(200)).await;

    let status = loop {
        match timeout(Duration::from_millis(1), output.read()).await {
            Ok(result) => {
                if let Some(data) = result? {
                    terminal.vt_write(&data.into_bytes());
                } else {
                    break process.wait().await?;
                }

                // Drain all remaining buffered output
                while let Some(data) = output.try_read() {
                    terminal.vt_write(&data.into_bytes());
                }

                let mut render_state = RenderState::new()?;
                let mut rows = RowIterator::new()?;
                let mut cells = CellIterator::new()?;

                let snapshot = render_state.update(&terminal)?;
                let mut row_iter = rows.update(&snapshot)?;

                let mut buf = String::with_capacity(CELL_COUNT as usize * 4);
                while let Some(row) = row_iter.next() {
                    let mut cell_iter = cells.update(row)?;
                    while let Some(cell) = cell_iter.next() {
                        let style = to_owo_style(cell.style()?);
                        let graphemes: Vec<char> = cell.graphemes()?;
                        let contents: String = if graphemes.is_empty() {
                            " ".into()
                        } else {
                            graphemes.iter().cloned().collect()
                        };
                        buf.push_str(&contents.style(style).to_string());
                    }
                    buf.push('\n');
                }

                render_state_count += 1;
                print!("\x1B[2J\x1B[1;1H{buf}");
                std::io::stdout().flush()?;
            }
            Err(_elapsed) => {
                if process.is_finished()? {
                    break process.wait().await?;
                }
                let key = random_key(&mut rng)?;
                process.write(key.as_bytes());
                input_count += 1;
            }
        }
    };

    let end = Instant::now();
    let duration = end - start;
    println!(
        "ran for {:.1} seconds, with {} inputs and {} renders ({} per second)",
        duration.as_secs_f64(),
        input_count,
        render_state_count,
        render_state_count as f64 / duration.as_secs_f64()
    );
    println!("process finished with code {}", status.exit_code());
    exit(status.exit_code() as i32);
}

fn to_owo_style(input: Style) -> owo_colors::Style {
    let mut style = owo_colors::Style::default();

    match input.fg_color {
        StyleColor::Rgb(color) => {
            style = style.truecolor(color.r, color.g, color.b);
        }
        StyleColor::Palette(PaletteIndex(palette_index)) => {
            let color = xterm_index_to_rgb(palette_index);
            style = style.truecolor(color.r, color.g, color.b);
        }
        StyleColor::None => {}
    }

    match input.bg_color {
        StyleColor::Rgb(color) => {
            style = style.on_truecolor(color.r, color.g, color.b);
        }
        StyleColor::Palette(PaletteIndex(palette_index)) => {
            let color = xterm_index_to_rgb(palette_index);
            style = style.on_truecolor(color.r, color.g, color.b);
        }
        StyleColor::None => {}
    }

    if input.italic {
        style = style.italic();
    }

    if input.bold {
        style = style.bold();
    }

    if input.underline != Underline::None {
        style = style.underline();
    }

    style
}

/// Convert an xterm 256-color index (0–255) to (r, g, b).
pub fn xterm_index_to_rgb(idx: u8) -> RgbColor {
    let i = idx as u32;

    // 0–15: standard + bright ANSI colors
    const ANSI_0_15: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00), // 0  black
        (0xcd, 0x00, 0x00), // 1  red
        (0x00, 0xcd, 0x00), // 2  green
        (0xcd, 0xcd, 0x00), // 3  yellow
        (0x00, 0x00, 0xee), // 4  blue
        (0xcd, 0x00, 0xcd), // 5  magenta
        (0x00, 0xcd, 0xcd), // 6  cyan
        (0xe5, 0xe5, 0xe5), // 7  white (light gray)
        (0x7f, 0x7f, 0x7f), // 8  bright black (dark gray)
        (0xff, 0x00, 0x00), // 9  bright red
        (0x00, 0xff, 0x00), // 10 bright green
        (0xff, 0xff, 0x00), // 11 bright yellow
        (0x5c, 0x5c, 0xff), // 12 bright blue
        (0xff, 0x00, 0xff), // 13 bright magenta
        (0x00, 0xff, 0xff), // 14 bright cyan
        (0xff, 0xff, 0xff), // 15 bright white
    ];

    if i < 16 {
        let (r, g, b) = ANSI_0_15[i as usize];
        return RgbColor { r, g, b };
    }

    // 16–231: 6×6×6 color cube
    if (16..=231).contains(&i) {
        let c = i - 16;
        let r = c / 36;
        let g = (c % 36) / 6;
        let b = c % 6;

        // component 0..5 → actual 8-bit value
        fn level(n: u32) -> u8 {
            if n == 0 { 0 } else { (n * 40 + 55) as u8 }
        }

        return RgbColor {
            r: level(r),
            g: level(g),
            b: level(b),
        };
    }

    // 232–255: grayscale ramp, 24 steps
    // values from 8 to 238 in steps of 10
    let gray = 8 + (i - 232) * 10;
    RgbColor {
        r: gray as u8,
        g: gray as u8,
        b: gray as u8,
    }
}

fn random_key(rng: &mut impl rand::Rng) -> Result<&'static str> {
    let tree = Tree::Branch {
        branches: vec![
            (1, Tree::Leaf { value: "\r" }),
            (1, Tree::Leaf { value: " " }),
            (1, Tree::Leaf { value: "\x1B" }), // escape
            (1, Tree::Leaf { value: "\t" }),   // tab
            (
                10,
                Tree::Branch {
                    branches: vec![
                        (1, Tree::Leaf { value: "\x1B[A" }),
                        (1, Tree::Leaf { value: "\x1B[B" }),
                        (1, Tree::Leaf { value: "\x1B[C" }),
                        (1, Tree::Leaf { value: "\x1B[D" }),
                    ],
                },
            ),
            (
                10,
                Tree::Branch {
                    branches: vec![
                        (1, Tree::Leaf { value: "m" }), // mem
                        (1, Tree::Leaf { value: "n" }), // net
                        (1, Tree::Leaf { value: "p" }), // proc
                        (1, Tree::Leaf { value: "c" }), // cpu
                        (1, Tree::Leaf { value: "e" }), // tree view
                        (1, Tree::Leaf { value: "f" }), // filter
                        (1, Tree::Leaf { value: "r" }), // reverse sort
                        (1, Tree::Leaf { value: "s" }), // sort options
                        (1, Tree::Leaf { value: "h" }), // help
                        (1, Tree::Leaf { value: "/" }), // search
                        (1, Tree::Leaf { value: "+" }), // expand
                        (1, Tree::Leaf { value: "-" }), // collapse
                        (1, Tree::Leaf { value: "1" }), // preset 1
                        (1, Tree::Leaf { value: "2" }), // preset 2
                        (1, Tree::Leaf { value: "3" }), // preset 3
                        (1, Tree::Leaf { value: "4" }), // preset 4
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

        let pair = pty_system.openpty(PtySize {
            rows: ROW_COUNT,
            cols: COLUMN_COUNT,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).unwrap();

        // Release any handles owned by the slave: we don't need it now
        // that we've spawned the child.
        drop(pair.slave);

        // Read the output in another thread.
        // This is important because it is easy to encounter a situation
        // where read/write buffers fill and block either your process
        // or the spawned process.
        let (output_write, output_read) = channel(64);
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

    pub fn is_finished(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }
}

struct PtyOutput {
    output_read: tokio::sync::mpsc::Receiver<String>,
}

impl PtyOutput {
    pub async fn read(&mut self) -> Result<Option<String>> {
        Ok(self.output_read.recv().await)
    }

    pub fn try_read(&mut self) -> Option<String> {
        self.output_read.try_recv().ok()
    }
}
