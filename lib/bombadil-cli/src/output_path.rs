use anyhow::{Result, bail};
use bombadil_trace::{TraceDriver, trace_driver};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Resolve a user-provided path (file or directory) to the trace
/// output directory. If the path points to a file, returns its parent.
pub fn resolve_trace_directory(path: &Path) -> PathBuf {
    if path.is_file() {
        path.parent()
            .expect("trace path has no parent")
            .to_path_buf()
    } else {
        path.to_path_buf()
    }
}

/// The browser trace file (`trace.jsonl`) for a user-provided trace path
/// (file or directory), refusing other drivers' binary traces so they
/// aren't misread.
pub fn resolve_browser_trace_file(path: &Path) -> Result<PathBuf> {
    match trace_driver(path)? {
        None | Some(TraceDriver::Browser) => {}
        Some(driver) => bail!(
            "{} holds a {driver} trace, not a browser trace",
            path.display()
        ),
    }
    Ok(resolve_trace_directory(path).join("trace.jsonl"))
}

/// Resolve the output path for a test run. If the user didn't specify
/// one, create a temporary directory.
pub fn resolve_output_path(output_path: &Option<PathBuf>) -> Result<PathBuf> {
    match output_path {
        Some(path) => Ok(path.clone()),
        None => Ok(TempDir::with_prefix("bombadil_")?.keep().to_path_buf()),
    }
}
