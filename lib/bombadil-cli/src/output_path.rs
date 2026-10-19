use anyhow::{Result, bail};
use bombadil_trace::{TRACE_FILE_NAME, TraceDriver, trace_driver};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::browser_trace::{BrowserTraceFile, JSON_TRACE_FILE_NAME};

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

/// The browser trace at a user-provided trace path: the file as given, or
/// a run directory's `trace.bin`, or else its legacy `trace.jsonl`.
/// Refuses other drivers' binary traces so they aren't misread, and a
/// path with no trace.
pub fn resolve_browser_trace_file(path: &Path) -> Result<BrowserTraceFile> {
    let in_directory = |name: &str| {
        if path.is_dir() {
            path.join(name)
        } else {
            path.to_path_buf()
        }
    };
    match trace_driver(path)? {
        Some(TraceDriver::Browser) => {
            Ok(BrowserTraceFile::Binary(in_directory(TRACE_FILE_NAME)))
        }
        Some(driver) => bail!(
            "{} holds a trace written by the {driver} driver, not a browser trace",
            path.display()
        ),
        None => {
            let file = in_directory(JSON_TRACE_FILE_NAME);
            if !file.is_file() {
                bail!("no browser trace at {}", path.display());
            }
            Ok(BrowserTraceFile::Json(file))
        }
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

    /// Writes a binary trace by `driver` with no states at `path`.
    fn write_binary_trace(path: &Path, driver: TraceDriver) {
        let mut file = Vec::new();
        bombadil_trace::write_envelope(&mut file, driver).unwrap();
        std::fs::write(path, file).unwrap();
    }

    /// A browser trace file is read as given, whatever its name; a
    /// directory's is its `trace.bin`, or else its legacy `trace.jsonl`.
    #[test]
    fn browser_trace_file_is_the_path_as_given() {
        let directory = tempfile::tempdir().unwrap();
        let renamed = directory.path().join("x.jsonl");
        std::fs::write(&renamed, "").unwrap();
        assert_eq!(
            resolve_browser_trace_file(&renamed).unwrap(),
            BrowserTraceFile::Json(renamed.clone())
        );
        let json_trace = directory.path().join(JSON_TRACE_FILE_NAME);
        std::fs::write(&json_trace, "").unwrap();
        assert_eq!(
            resolve_browser_trace_file(directory.path()).unwrap(),
            BrowserTraceFile::Json(json_trace)
        );

        let binary_trace = directory.path().join(TRACE_FILE_NAME);
        write_binary_trace(&binary_trace, TraceDriver::Browser);
        assert_eq!(
            resolve_browser_trace_file(directory.path()).unwrap(),
            BrowserTraceFile::Binary(binary_trace)
        );
        let renamed = directory.path().join("run-1.bin");
        write_binary_trace(&renamed, TraceDriver::Browser);
        assert_eq!(
            resolve_browser_trace_file(&renamed).unwrap(),
            BrowserTraceFile::Binary(renamed.clone())
        );
    }

    /// Other drivers' binary traces aren't read as browser traces, in a
    /// directory or passed directly, and a path without a trace is
    /// refused.
    #[test]
    fn browser_trace_file_refuses_other_traces() {
        let directory = tempfile::tempdir().unwrap();
        let trace = directory.path().join(TRACE_FILE_NAME);
        write_binary_trace(&trace, TraceDriver::Terminal);
        for path in [directory.path(), trace.as_path()] {
            let error = resolve_browser_trace_file(path).err().unwrap();
            assert_eq!(
                error.to_string(),
                format!(
                    "{} holds a trace written by the terminal driver, not a \
                     browser trace",
                    path.display()
                )
            );
        }

        let empty = tempfile::tempdir().unwrap();
        let missing = empty.path().join("missing.jsonl");
        for path in [empty.path(), missing.as_path()] {
            let error = resolve_browser_trace_file(path).err().unwrap();
            assert_eq!(
                error.to_string(),
                format!("no browser trace at {}", path.display())
            );
        }
    }
}
