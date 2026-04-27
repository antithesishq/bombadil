use rand::{self, seq::IndexedRandom};
use std::process::Stdio;

use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
};

use anyhow::Result;
use anyhow::anyhow;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[tokio::main]
async fn main() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions {
        cols: 120,
        rows: 24,
        max_scrollback: 10_000,
    })?;

    let process = Command::new("btop")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .stdin(Stdio::piped())
        .spawn()?;

    let mut rng = rand::rng();

    if let (Some(stdout), Some(mut stdin)) = (process.stdout, process.stdin) {
        let mut stdout = BufReader::new(stdout);
        loop {
            let mut buffer = Vec::with_capacity(1024 * 1024);
            let size = stdout.read_buf(&mut buffer).await?;
            if size > 0 {
                terminal.vt_write(&buffer);
            }

            let key = random_key(&mut rng)?;
            stdin.write_all(key.as_bytes()).await?;

            let mut render_state = RenderState::new()?;
            let mut rows = RowIterator::new()?;
            let mut cells = CellIterator::new()?;

            let snapshot = render_state.update(&terminal)?;
            print!("\x1B[2J\x1B[1;1H");
            let mut row_iter = rows.update(&snapshot)?;

            while let Some(row) = row_iter.next() {
                let mut cell_iter = cells.update(row)?;
                while let Some(cell) = cell_iter.next() {
                    let graphemes: Vec<char> = cell.graphemes()?;
                    if graphemes.is_empty() {
                        print!(" ");
                    } else {
                        for grapheme in graphemes {
                            print!("{}", grapheme);
                        }
                    }
                }
                println!();
            }
            println!("pressing key: {key:?}");
        }
    }

    Ok(())
}

fn random_key(rng: &mut impl rand::Rng) -> Result<&'static str> {
    let keys = ["\n", "\x1B[A", "\x1B[B", "\x1B[C", "\x1B[D"];
    keys.choose(rng).cloned().ok_or(anyhow!(""))
}
