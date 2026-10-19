use anyhow::{Result, bail};
use bombadil_trace::{TraceDriver, trace_driver};
use std::path::{Path, PathBuf};

/// Validate a browser trace file, without interpreting directories.
pub fn resolve_browser_trace_file(path: &Path) -> Result<PathBuf> {
    match trace_driver(path)? {
        Some(TraceDriver::Browser) => Ok(path.to_path_buf()),
        Some(driver) => bail!(
            "{} holds a trace written by the {driver} driver, not a browser trace",
            path.display()
        ),
        None => bail!("no browser trace at {}", path.display()),
    }
}

/// Choose an unused file path in system temp, without retaining an empty
/// placeholder that would conflict with the writer's create_new.
pub fn resolve_output_path(output_path: &Option<PathBuf>) -> Result<PathBuf> {
    match output_path {
        Some(path) if path.is_dir() => bail!(
            "output path must name a trace file, not a directory: {}",
            path.display()
        ),
        Some(path) => Ok(path.clone()),
        None => {
            let file = tempfile::Builder::new()
                .prefix("bombadil-")
                .suffix(".bin")
                .tempfile()?;
            let path = file.path().to_path_buf();
            file.close()?;
            Ok(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_files_and_directories_are_not_interchangeable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("arbitrary.data");
        let mut bytes = Vec::new();
        bombadil_trace::write_envelope(&mut bytes, TraceDriver::Browser)
            .unwrap();
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(resolve_browser_trace_file(&path).unwrap(), path);
        assert!(resolve_browser_trace_file(directory.path()).is_err());
        assert!(
            resolve_output_path(&Some(directory.path().to_path_buf())).is_err()
        );
        let default = resolve_output_path(&None).unwrap();
        assert!(!default.exists());
        assert_eq!(default.parent().unwrap(), std::env::temp_dir());
    }
}
