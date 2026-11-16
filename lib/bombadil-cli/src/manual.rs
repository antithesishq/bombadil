use std::{
    io::{self, IsTerminal, Write},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, ensure};

pub fn show(driver: &str) -> Result<()> {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let filename = if interactive {
        "bombadil.1"
    } else {
        "bombadil-manual.txt"
    };
    let url =
        format!("https://antithesishq.github.io/bombadil/{driver}/{filename}");
    let manual = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .get(&url)
        .send()
        .with_context(|| format!("failed to download manual from {url}"))?
        .error_for_status()?
        .bytes()?;

    if interactive {
        let mut file = tempfile::Builder::new()
            .prefix(&format!("bombadil-{driver}-"))
            .suffix(".1")
            .tempfile()?;
        file.write_all(&manual)?;
        file.flush()?;
        let status = Command::new("man")
            .arg("-l")
            .arg(file.path())
            .status()
            .context("failed to launch man")?;
        ensure!(status.success(), "man exited with {status}");
    } else if let Err(error) = io::stdout().lock().write_all(&manual)
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        return Err(error.into());
    }
    Ok(())
}
