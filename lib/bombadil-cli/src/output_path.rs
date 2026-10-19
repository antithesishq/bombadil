use anyhow::{Result, bail};
use bombadil_trace::{TraceDriver, trace_driver};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Resolve a user-provided path (file or directory) to the trace
/// output directory. If the path points to a file, returns its parent.
pub fn resolve_trace_directory(path: &Path) -> PathBuf {
    if path.is_file() {
        file_directory(path)
    } else {
        path.to_path_buf()
    }
}

/// The directory of the file at `path`: `.` for a bare file name, whose
/// parent is empty and can't be canonicalized.
fn file_directory(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The browser trace file for a user-provided trace path: the file as
/// given, or a directory's `trace.jsonl`, refusing binary traces so they
/// aren't misread as JSON lines.
pub fn resolve_browser_trace_file(path: &Path) -> Result<PathBuf> {
    match trace_driver(path)? {
        None => {}
        Some(TraceDriver::Browser) => bail!(
            "{} is a binary browser trace, which this version can't read",
            path.display()
        ),
        Some(driver) => bail!(
            "{} holds a trace written by the {driver} driver, not a browser trace",
            path.display()
        ),
    }
    if path.is_dir() {
        Ok(path.join("trace.jsonl"))
    } else {
        Ok(path.to_path_buf())
    }
}

/// Resolve the output path for a test run. If the user didn't specify
/// one, create a temporary directory.
pub fn resolve_output_path(output_path: &Option<PathBuf>) -> Result<PathBuf> {
    match output_path {
        Some(path) => Ok(path.clone()),
        None => Ok(TempDir::with_prefix("bombadil_")?.keep().to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare file name is in the current directory, so it compares equal
    /// to an output path there.
    #[test]
    fn file_directory_of_a_bare_name_is_the_current_directory() {
        assert_eq!(file_directory(Path::new("trace.bin")), Path::new("."));
        assert_eq!(
            file_directory(Path::new("dir/trace.bin")),
            Path::new("dir")
        );
        assert_eq!(file_directory(Path::new("/trace.bin")), Path::new("/"));
    }

    /// A browser trace file is read as given, whatever its name; a
    /// directory's is its `trace.jsonl`.
    #[test]
    fn browser_trace_file_is_the_path_as_given() {
        let directory = tempfile::tempdir().unwrap();
        let renamed = directory.path().join("x.jsonl");
        std::fs::write(&renamed, "").unwrap();
        assert_eq!(resolve_browser_trace_file(&renamed).unwrap(), renamed);
        assert_eq!(
            resolve_browser_trace_file(directory.path()).unwrap(),
            directory.path().join("trace.jsonl")
        );
    }

    /// Binary traces aren't read as JSON lines: a terminal one, or a
    /// browser one, in a directory or passed directly.
    #[test]
    fn browser_trace_file_refuses_binary_traces() {
        for (driver, message) in [
            (
                TraceDriver::Terminal,
                "holds a trace written by the terminal driver, not a browser trace",
            ),
            (
                TraceDriver::Browser,
                "is a binary browser trace, which this version can't read",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let trace = directory.path().join(bombadil_trace::TRACE_FILE_NAME);
            let mut file = Vec::new();
            bombadil_trace::write_envelope(&mut file, driver).unwrap();
            std::fs::write(&trace, file).unwrap();
            for path in [directory.path(), trace.as_path()] {
                let error = resolve_browser_trace_file(path).err().unwrap();
                assert_eq!(
                    error.to_string(),
                    format!("{} {message}", path.display())
                );
            }
        }
    }
}
