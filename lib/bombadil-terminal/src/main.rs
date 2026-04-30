use bombadil::tree::Tree;
use clap::Parser;
use owo_colors::OwoColorize;
use rand::{self, SeedableRng};
use std::{ffi::OsStr, io::Write, process::exit, time::Duration};

use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
    style::{PaletteIndex, RgbColor, Style, StyleColor, Underline},
    terminal::ScrollViewport,
};

use anyhow::{Result, bail};
use portable_pty::{
    Child, CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize,
    PtySystem,
};
use tokio::{
    join,
    sync::mpsc::channel,
    time::{Instant, sleep, timeout},
};

/// Property-based testing for terminal UIs
#[derive(Parser)]
#[command(name = "bombadil-terminal", version, about, long_about=None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Test the given program and arguments
    Test {
        // The command to run for each test case
        #[clap(trailing_var_arg = true)]
        command: Vec<String>,
        // How many test cases to run (invocations of command)
        #[arg(long, default_value = "1")]
        test_count: u64,
        /// Random generator seed
        #[arg(long)]
        seed: Option<u64>,
        // Whether to append render output (otherwise clear screen before every render)
        #[arg(long, default_value_t = false)]
        render_append: bool,
    },
}

#[derive(Debug, Copy, Clone)]
struct Size {
    column_count: u16,
    row_count: u16,
}

impl Size {
    pub fn cell_count(&self) -> u16 {
        self.column_count * self.row_count
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Test {
            command,
            test_count,
            seed,
            render_append,
        } => {
            if let Some(seed) = seed {
                if let Err(error) =
                    test_program(seed, render_append, &command).await
                {
                    eprintln!(
                        "\n\ntest failed: {error}\n\nreproduced from seed: {seed}"
                    );
                    exit(1);
                }
            } else {
                for _ in 1..=test_count {
                    let seed = rand::random();
                    match test_program(seed, render_append, &command).await {
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!(
                                "\n\ntest failed: {error}\n\nreproduce with seed: {seed}"
                            );
                            exit(1);
                        }
                    }
                }
            }
        }
    }
}

async fn test_program(
    seed: u64,
    render_append: bool,
    command: &[String],
) -> Result<()> {
    let start = Instant::now();
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    let mut size_current: Size = random_size(&mut rng);
    let mut terminal = Terminal::new(TerminalOptions {
        cols: size_current.column_count,
        rows: size_current.row_count,
        max_scrollback: 1_000,
    })?;

    let (program, args) = match command {
        [program, args @ ..] => (program, args),
        _ => bail!("command has no program"),
    };
    let (mut process, mut output) =
        PtyProcess::spawn(size_current, program, args).await?;

    let mut render_state_count = 0;
    let render_timeout = Duration::from_secs(10);
    let mut render_last_at = Instant::now();
    let mut last_action = None;
    let mut action_count = 0;

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

                let mut buf = String::with_capacity(
                    size_current.cell_count() as usize * 4,
                );
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
                render_last_at = Instant::now();
                if render_append {
                    println!("\n");
                } else {
                    print!("\x1B[2J\x1B[1;1H");
                }
                println!("{buf}");
            }
            Err(_elapsed) => {
                if process.is_finished()? {
                    break process.wait().await?;
                }

                // If we've gone too long without a render, treat as “stuck UI”
                if render_last_at.elapsed() > render_timeout {
                    bail!(
                        "no render for {:?} despite continued input; program likely stuck",
                        render_timeout
                    );
                }

                let action = random_action(&terminal, &mut rng)?;
                match action {
                    Action::TypeChar(char) => {
                        let mut buffer = [0u8; 4];
                        process.write(char.encode_utf8(&mut buffer).as_bytes());
                    }
                    Action::TypeString(string) => {
                        process.write(string.as_bytes());
                    }
                    Action::Scroll(scroll_viewport) => {
                        terminal.scroll_viewport(scroll_viewport);
                    }
                    Action::Resize(size) => {
                        size_current = size;
                        terminal.resize(
                            size.column_count,
                            size.row_count,
                            0,
                            0,
                        )?;
                        process.resize(size)?;
                    }
                }
                last_action = Some(action);
                action_count += 1;

                if render_append {
                    println!();
                } else {
                    // For redrawing the status only
                    print!("\x1b[2K\x1b[1G");
                }
            }
        }

        print!(
            "Scroll offset: {}\tSize: {}/{}\tLast action: {:?}",
            terminal.scrollbar().expect("no scrollbar").offset,
            size_current.column_count,
            size_current.row_count,
            last_action
        );
        std::io::stdout().flush()?;
    };

    let end = Instant::now();
    let duration = end - start;
    println!(
        "\n\nran for {:.1} seconds, with {} actions and {} renders ({} per second)",
        duration.as_secs_f64(),
        action_count,
        render_state_count,
        render_state_count as f64 / duration.as_secs_f64()
    );
    if !status.success() {
        bail!("process finished with code {}", status.exit_code());
    }
    Ok(())
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

#[derive(Debug, Copy, Clone)]
enum Action {
    TypeChar(char),
    TypeString(&'static str),
    Scroll(ScrollViewport),
    Resize(Size),
}

fn random_action(
    terminal: &Terminal,
    rng: &mut impl rand::Rng,
) -> Result<Action> {
    let tree = Tree::Branch {
        branches: vec![
            (20, random_key()),
            (1, random_scroll(terminal)),
            (
                1,
                Tree::Leaf {
                    value: Action::Resize(random_size(rng)),
                },
            ),
        ],
    };
    let tree = tree.prune().expect("no actions available");
    Ok(*tree.pick(rng)?)
}

fn random_key() -> Tree<Action> {
    use Action::*;
    Tree::Branch {
        branches: vec![
            (
                1,
                Tree::Leaf {
                    value: TypeChar('\r'),
                },
            ),
            (
                1,
                Tree::Leaf {
                    value: TypeChar('\x1B'), // Escape
                },
            ),
            (
                1,
                Tree::Leaf {
                    value: TypeChar('\t'),
                },
            ),
            (
                1,
                Tree::Branch {
                    branches: vec![
                        (
                            1,
                            Tree::Leaf {
                                value: TypeString("\x1B[A"),
                            },
                        ),
                        (
                            1,
                            Tree::Leaf {
                                value: TypeString("\x1B[B"),
                            },
                        ),
                        (
                            1,
                            Tree::Leaf {
                                value: TypeString("\x1B[C"),
                            },
                        ),
                        (
                            1,
                            Tree::Leaf {
                                value: TypeString("\x1B[D"),
                            },
                        ),
                    ],
                },
            ),
            // ASCII printable range
            (
                1,
                Tree::Branch {
                    branches: (32..=127)
                        .map(|b| {
                            (
                                1,
                                Tree::Leaf {
                                    value: TypeChar(char::from(b)),
                                },
                            )
                        })
                        .collect(),
                },
            ),
        ],
    }
}

fn random_scroll(terminal: &Terminal) -> Tree<Action> {
    let mut branches = vec![];

    if let Ok(scrollbar) = terminal.scrollbar() {
        if scrollbar.total > scrollbar.len {
            branches.push((
                1,
                Tree::Leaf {
                    value: Action::Scroll(ScrollViewport::Bottom),
                },
            ));
        }

        if scrollbar.offset > 0 {
            branches.push((
                1,
                Tree::Leaf {
                    value: Action::Scroll(ScrollViewport::Top),
                },
            ));
        }
    }

    Tree::Branch { branches }
}

fn random_size(rng: &mut impl rand::Rng) -> Size {
    let column_count = rng.random_range(80..180);
    let row_count = rng.random_range(16..96);
    Size {
        column_count,
        row_count,
    }
}

struct PtyProcess {
    child: Box<dyn Child + Send + Sync>,
    input_write: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send + 'static>,
    reader: tokio::task::JoinHandle<()>,
}

impl PtyProcess {
    async fn spawn<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(
        size: Size,
        command: &str,
        args: I,
    ) -> Result<(Self, PtyOutput)> {
        let pty_system = NativePtySystem::default();

        let pair = pty_system.openpty(PtySize {
            rows: size.row_count,
            cols: size.column_count,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let (output_write, output_read) = channel(64);
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("couldn't clone master reader");
        let reader = tokio::spawn(async move {
            let mut buffer = [0u8; 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let output = String::from_utf8_lossy(&buffer[..n]);
                        output_write
                            .send(output.into())
                            .await
                            .expect("failed to send output");
                    }
                    Err(e) => {
                        eprintln!("Error reading from PTY: {}", e);
                        break;
                    }
                }
            }
        });

        Ok((
            Self {
                child,
                input_write: pair.master.take_writer()?,
                master: pair.master,
                reader,
            },
            PtyOutput { output_read },
        ))
    }

    pub fn write(&mut self, input: &[u8]) {
        self.input_write.write_all(input).expect("write failed");
    }

    pub fn resize(&mut self, size: Size) -> Result<()> {
        self.master.resize(PtySize {
            cols: size.column_count,
            rows: size.row_count,
            ..Default::default()
        })
    }

    pub async fn wait(mut self) -> Result<ExitStatus> {
        let status = self.child.wait()?;
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
