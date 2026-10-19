use std::{borrow::Cow, path::PathBuf};

use anyhow::{Result, bail};
use bombadil::{
    driver::{OutputWriter, RunId, TraceWriter, remove_legacy_trace},
    specification::domain::Snapshot,
};
use bombadil_schema::browser::BrowserTraceEntry;
use bombadil_trace::{TraceDriver, TraceFileWriter};

use crate::{
    browser::{actions::BrowserAction, state::BrowserState},
    convert::ToSchema,
    driver::BrowserSession,
    trace::{PropertyViolation, TraceEntry},
};

/// File name of a browser trace written before `trace.bin`.
const JSON_TRACE_FILE_NAME: &str = "trace.jsonl";

pub struct FileOutputWriter {
    pub root_path: PathBuf,
    pub overwrite: bool,
}

impl FileOutputWriter {
    pub fn run_directory(&self, run_id: RunId) -> PathBuf {
        self.root_path.join("runs").join(format!("{}", run_id.0))
    }
}

impl OutputWriter<BrowserSession> for FileOutputWriter {
    type TraceWriter = FileTraceWriter;

    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter> {
        let run_path =
            self.root_path.join("runs").join(format!("{}", run_id.0));
        FileTraceWriter::initialize(run_path, self.overwrite)
    }
}

/// Writes a browser trace as `trace.bin`: each state's metadata is a
/// [`BrowserTraceEntry`], with the screenshot's format in its
/// `screenshot` field and the screenshot itself as the state's payload.
pub struct FileTraceWriter {
    trace: TraceFileWriter<BrowserTraceEntry>,
    last_transition_hash: Option<u64>,
}

impl FileTraceWriter {
    /// Creates `trace.bin` in `run_path`. Refuses if `run_path` already
    /// holds a trace, `trace.bin` or a legacy `trace.jsonl`, unless
    /// `overwrite` is set.
    ///
    /// When overwriting, a legacy `trace.jsonl`, and if it was a browser
    /// trace the `screenshots` directory next to it, are removed once
    /// `trace.bin` is created, so a failure to create it leaves them as
    /// they were (while both are there, `trace.bin` is the one read). A
    /// `screenshots` directory without a legacy browser trace isn't
    /// bombadil's, and is kept. Failing to remove the legacy files is
    /// only logged: the new trace is fine, and they are only stale (see
    /// [`remove_legacy_trace`]).
    pub fn initialize(run_path: PathBuf, overwrite: bool) -> Result<Self> {
        let json_trace = run_path.join(JSON_TRACE_FILE_NAME);
        if !overwrite && json_trace.try_exists()? {
            bail!(
                "{JSON_TRACE_FILE_NAME} already exists at {}. \
                 Use --output-path-overwrite to overwrite, or choose a \
                 different --output-path.",
                json_trace.display(),
            );
        }
        let trace = TraceFileWriter::create(
            &run_path,
            TraceDriver::Browser,
            overwrite,
        )?;
        // The new trace is fine, and what is left is only stale.
        if overwrite && let Err(error) = remove_legacy_trace(&run_path, false) {
            log::warn!("{error:#}");
        }
        Ok(FileTraceWriter {
            trace,
            last_transition_hash: None,
        })
    }
}

impl TraceWriter<BrowserSession> for FileTraceWriter {
    fn write(
        &mut self,
        state: &BrowserState,
        last_action: Option<&BrowserAction>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let entry = TraceEntry {
            timestamp: state.timestamp,
            url: Cow::Borrowed(&state.url),
            hash_previous: self.last_transition_hash,
            hash_current: state.transition_hash,
            action: last_action.map(Cow::Borrowed),
            screenshot: state.screenshot.format.extension(),
            snapshots: Cow::Borrowed(snapshots),
            violations: Cow::Borrowed(violations),
            resources: Cow::Borrowed(&state.resources),
        };
        self.last_transition_hash = state.transition_hash;
        // The screenshot is shared with the state, not copied.
        self.trace
            .write_state(entry.to_schema(), Some(state.screenshot.data.clone()))
    }

    fn finish(&mut self) -> Result<()> {
        self.trace.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The directory a legacy browser trace's screenshots are in.
    const JSON_TRACE_SCREENSHOTS_DIRECTORY: &str = "screenshots";
    use bombadil_trace::TRACE_FILE_NAME;

    /// The first line of a legacy browser `trace.jsonl`, after a blank one.
    const LEGACY_BROWSER_TRACE: &str = concat!(
        "\n",
        r#"{"timestamp":0,"action":null,"state":{"url":"http://localhost/","hash_previous":null,"hash_current":1,"screenshot":"screenshots/0.webp","resources":{}},"snapshots":[],"violations":[]}"#,
        "\n"
    );

    /// The first line of a legacy terminal `trace.jsonl`.
    const LEGACY_TERMINAL_TRACE: &str = r#"{"timestamp":0,"action":{"TypeText":{"text":"{\"url\":\"u\",\"screenshot\":\"s\"}"}},"state":{"grid":{"cells":[{"Occupied":{"contents":"u","wide":false,"style":{"foreground_color":"None","background_color":"None","underline_color":"None","underline":"None","attributes":0}}}],"size":{"columns":1,"rows":1}},"scrollback":{"cells":[],"size":{"columns":1,"rows":0}},"scroll_offset":0,"cursor":{"position":{"column":0,"row":0},"visible":true,"blinking":false,"visual_style":"Block","color":"None"},"exit_status":null},"snapshots":[],"violations":[]}"#;

    fn initialize(run_path: &std::path::Path, overwrite: bool) -> Result<()> {
        FileTraceWriter::initialize(run_path.to_path_buf(), overwrite).map(drop)
    }

    /// An existing trace, binary or legacy JSON lines, is refused without
    /// overwriting, naming its full path, and left as it was.
    #[test]
    fn refuses_an_existing_trace() {
        for existing in [TRACE_FILE_NAME, JSON_TRACE_FILE_NAME] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(existing);
            std::fs::write(&path, "previous").unwrap();
            let error = initialize(directory.path(), false).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "{existing} already exists at {}. Use \
                     --output-path-overwrite to overwrite, or choose a \
                     different --output-path.",
                    path.display()
                )
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous");
            let other = if existing == TRACE_FILE_NAME {
                JSON_TRACE_FILE_NAME
            } else {
                TRACE_FILE_NAME
            };
            assert!(!directory.path().join(other).exists());
        }
    }

    /// Overwriting replaces a binary trace, and removes a legacy JSON
    /// lines trace and its screenshots, so the directory holds one trace.
    #[test]
    fn overwriting_replaces_binary_and_legacy_traces() {
        let directory = tempfile::tempdir().unwrap();
        let binary_trace = directory.path().join(TRACE_FILE_NAME);
        let json_trace = directory.path().join(JSON_TRACE_FILE_NAME);
        let screenshots =
            directory.path().join(JSON_TRACE_SCREENSHOTS_DIRECTORY);
        std::fs::write(&binary_trace, "previous").unwrap();
        std::fs::write(&json_trace, LEGACY_BROWSER_TRACE).unwrap();
        std::fs::create_dir(&screenshots).unwrap();
        std::fs::write(screenshots.join("1.webp"), "image").unwrap();

        initialize(directory.path(), true).unwrap();
        assert_eq!(
            bombadil_trace::trace_driver(&binary_trace).unwrap(),
            Some(TraceDriver::Browser)
        );
        assert!(!json_trace.exists());
        assert!(!screenshots.exists());
    }

    /// Failing to create the trace when overwriting leaves a legacy trace
    /// and its screenshots as they were.
    #[test]
    fn failing_to_create_keeps_a_legacy_trace() {
        let directory = tempfile::tempdir().unwrap();
        let json_trace = directory.path().join(JSON_TRACE_FILE_NAME);
        let screenshots =
            directory.path().join(JSON_TRACE_SCREENSHOTS_DIRECTORY);
        std::fs::write(&json_trace, LEGACY_BROWSER_TRACE).unwrap();
        std::fs::create_dir(&screenshots).unwrap();
        std::fs::write(screenshots.join("1.webp"), "image").unwrap();
        // A trace can't be renamed over a non-empty directory.
        std::fs::create_dir_all(
            directory.path().join(TRACE_FILE_NAME).join("x"),
        )
        .unwrap();
        initialize(directory.path(), true).unwrap_err();
        assert_eq!(
            std::fs::read_to_string(&json_trace).unwrap(),
            LEGACY_BROWSER_TRACE
        );
        assert!(screenshots.join("1.webp").is_file());
    }

    /// A `screenshots` directory without a legacy trace isn't bombadil's,
    /// e.g. screenshots extracted with `bombadil trace --screenshots`, and
    /// is kept when overwriting.
    #[test]
    fn overwriting_keeps_screenshots_without_a_legacy_trace() {
        let directory = tempfile::tempdir().unwrap();
        let screenshots =
            directory.path().join(JSON_TRACE_SCREENSHOTS_DIRECTORY);
        std::fs::create_dir(&screenshots).unwrap();
        std::fs::write(screenshots.join("0.jpeg"), "image").unwrap();
        initialize(directory.path(), true).unwrap();
        assert!(screenshots.join("0.jpeg").is_file());
    }

    /// A `screenshots` directory next to a legacy trace that isn't a
    /// browser's, like a terminal trace, or one that can't be read, isn't
    /// bombadil's, and is kept, while the trace is removed.
    #[test]
    fn overwriting_keeps_screenshots_next_to_another_legacy_trace() {
        for legacy in [LEGACY_TERMINAL_TRACE, "previous"] {
            let directory = tempfile::tempdir().unwrap();
            let json_trace = directory.path().join(JSON_TRACE_FILE_NAME);
            std::fs::write(&json_trace, legacy).unwrap();
            let screenshots =
                directory.path().join(JSON_TRACE_SCREENSHOTS_DIRECTORY);
            std::fs::create_dir(&screenshots).unwrap();
            std::fs::write(screenshots.join("mine.png"), "image").unwrap();
            initialize(directory.path(), true).unwrap();
            assert!(!json_trace.exists());
            assert!(screenshots.join("mine.png").is_file(), "{legacy}");
        }
    }

    /// A directory in `screenshots` that can't be emptied (mode 555) while
    /// held, which dropping allows again (mode 755), even if a test fails.
    /// `None`, noted on stderr, if permissions don't stop removing it, as for
    /// root.
    #[cfg(unix)]
    struct LockedScreenshots(std::path::PathBuf);

    #[cfg(unix)]
    impl LockedScreenshots {
        fn new(screenshots: &std::path::Path) -> Option<Self> {
            use std::os::unix::fs::PermissionsExt;
            let path = screenshots.join("l");
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("0.webp"), "image").unwrap();
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(0o555),
            )
            .unwrap();
            let locked = LockedScreenshots(path);
            if std::fs::remove_file(locked.0.join("0.webp")).is_ok() {
                eprintln!(
                    "skipped: permissions don't stop removing files here"
                );
                return None;
            }
            Some(locked)
        }
    }

    #[cfg(unix)]
    impl Drop for LockedScreenshots {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o755);
            let _ = std::fs::set_permissions(&self.0, mode);
        }
    }

    /// A legacy browser trace whose `screenshots` can't be removed is kept
    /// with them, only logged, and removed by overwriting again once they
    /// can be.
    #[cfg(unix)]
    #[test]
    fn overwriting_retries_screenshots_it_fails_to_remove() {
        let directory = tempfile::tempdir().unwrap();
        let json_trace = directory.path().join(JSON_TRACE_FILE_NAME);
        let screenshots =
            directory.path().join(JSON_TRACE_SCREENSHOTS_DIRECTORY);
        std::fs::write(&json_trace, LEGACY_BROWSER_TRACE).unwrap();
        let Some(locked) = LockedScreenshots::new(&screenshots) else {
            return;
        };
        initialize(directory.path(), true).unwrap();
        assert_eq!(
            bombadil_trace::trace_driver(directory.path()).unwrap(),
            Some(TraceDriver::Browser)
        );
        assert!(json_trace.is_file());
        drop(locked);
        initialize(directory.path(), true).unwrap();
        assert!(!json_trace.exists());
        assert!(!screenshots.exists());
    }

    /// Failing to remove a legacy trace once the new one is created is
    /// only logged. (A directory isn't a browser trace, so `screenshots`
    /// next to it isn't bombadil's, and is kept anyway.)
    #[test]
    fn failing_to_remove_a_legacy_trace_keeps_the_new_trace() {
        let directory = tempfile::tempdir().unwrap();
        // A non-empty directory can't be removed as a file.
        let json_trace = directory.path().join(JSON_TRACE_FILE_NAME);
        std::fs::create_dir_all(json_trace.join("x")).unwrap();
        let screenshots =
            directory.path().join(JSON_TRACE_SCREENSHOTS_DIRECTORY);
        std::fs::create_dir(&screenshots).unwrap();
        initialize(directory.path(), true).unwrap();
        assert_eq!(
            bombadil_trace::trace_driver(directory.path()).unwrap(),
            Some(TraceDriver::Browser)
        );
        assert!(json_trace.is_dir());
        assert!(screenshots.is_dir());
    }
}
