use std::process::Stdio;

use anyhow::Result;
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::Command,
};

#[tokio::main]
async fn main() -> Result<()> {
    let process = Command::new("btop")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(stdout) = process.stdout {
        let mut stdout = BufReader::new(stdout);
        loop {
            let mut buffer = Vec::with_capacity(1024 * 1024);
            let size = stdout.read_buf(&mut buffer).await?;
            if size > 0 {
                println!("output: {:?}", String::from_utf8(buffer)?);
            }
        }
    }

    Ok(())
}
