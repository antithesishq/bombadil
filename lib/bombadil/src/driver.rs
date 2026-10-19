use std::fmt::Display;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::{fmt::Debug, hash::Hasher};

use anyhow::{Context, Result};
use bombadil_schema::Time;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json as json;

use crate::runner::PropertyViolation;
use crate::{
    render::Format,
    specification::{domain::Snapshot, verifier::Verifier},
};

/// Convert a JSON value produced by a specification's action generator
/// into a validated action.
pub trait FromGeneratedAction: Sized {
    fn from_generated(value: json::Value) -> Result<Self>;
}

/// Identity conversion.
impl FromGeneratedAction for json::Value {
    fn from_generated(value: json::Value) -> Result<Self> {
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Default)]
pub struct RunId(pub u64);

impl Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A driver runs a user interface of some sort (the system under test).
pub trait InterfaceDriver {
    type Session: InterfaceSession;
    fn new_session(&self, run_id: RunId) -> Result<(Self::Session, Verifier)>;
}

pub trait RunState {
    fn timestamp(&self) -> Time;
}

pub trait ActionTemplate<Action> {
    fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        rng: &mut Rng,
    ) -> Action;

    fn accepts(&self, original: &Action) -> bool;

    fn category_hash<H: Hasher>(&self, hasher: &mut H);
}

pub trait OutputWriter<Session: InterfaceSession> {
    type TraceWriter: TraceWriter<Session>;
    fn trace_writer(&mut self, run_id: RunId) -> Result<Self::TraceWriter>;
}

pub trait TraceWriter<Session: InterfaceSession> {
    fn write(
        &mut self,
        state: &Session::State,
        last_action: Option<&Session::Action>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()>;

    /// Completes the trace after the last state, reporting any failure to
    /// write it. Called once by the runner when a run ends.
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

/// File name of a trace written before `trace.bin`.
const JSON_TRACE_FILE_NAME: &str = "trace.jsonl";
/// The directory a legacy browser trace's screenshots are in.
const JSON_TRACE_SCREENSHOTS_DIRECTORY: &str = "screenshots";

/// Whether `json_trace`, a legacy `trace.jsonl`, was written by the
/// browser driver: its first non-blank line is a browser state, with a
/// string URL and screenshot path, either nested in `state` (from the
/// version decoupling the browser and terminal drivers, after v0.5.0, on)
/// or at the top level (v0.5.0 and earlier, before any terminal driver).
/// Only such a trace came with a `screenshots` directory next to it,
/// which replacing the trace removes with it; next to any other trace,
/// like a terminal one, or one that can't be read, a `screenshots`
/// directory isn't bombadil's, and is kept.
///
/// Only the fields telling the drivers apart are read, so a browser trace
/// of an older version, with actions this one doesn't know, still counts.
/// Anything but a regular file (a FIFO, say) doesn't count, and isn't
/// opened.
fn is_legacy_browser_trace(json_trace: &Path) -> bool {
    use std::io::{BufRead, Read};
    #[derive(Deserialize)]
    #[serde(untagged)]
    #[allow(dead_code)]
    enum Entry {
        Nested { state: State },
        Flat(State),
    }
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct State {
        url: String,
        screenshot: String,
    }
    // A line longer than this isn't read in full, and so doesn't count.
    const LINE_BYTES_MAX: u64 = 64 << 20;
    if !std::fs::symlink_metadata(json_trace).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let Ok(file) = std::fs::File::open(json_trace) else {
        return false;
    };
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        match (&mut reader).take(LINE_BYTES_MAX).read_line(&mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) if line.trim().is_empty() => continue,
            Ok(_) => return json::from_str::<Entry>(&line).is_ok(),
        }
    }
}

/// Removes a legacy `trace.jsonl` in `directory`, if there is one: first,
/// if it is a browser trace (its first non-blank line is a browser
/// state, as any version wrote it), the `screenshots` directory next to
/// it, unless `keep_screenshots`, and then the trace itself.
///
/// Failing to remove `screenshots` is only logged, and keeps the trace,
/// so the next overwrite still tells it is a browser trace and tries
/// again; meanwhile a `trace.bin` next to it is the trace read. Failing
/// to remove the trace itself is returned, for the caller to report or
/// log.
pub fn remove_legacy_trace(
    directory: &Path,
    keep_screenshots: bool,
) -> Result<()> {
    let json_trace = directory.join(JSON_TRACE_FILE_NAME);
    let screenshots = directory.join(JSON_TRACE_SCREENSHOTS_DIRECTORY);
    if !keep_screenshots
        && is_legacy_browser_trace(&json_trace)
        && screenshots.is_dir()
        && let Err(error) = std::fs::remove_dir_all(&screenshots)
    {
        log::warn!(
            "failed to remove the stale {}, so {} is kept too: {error}",
            screenshots.display(),
            json_trace.display()
        );
        return Ok(());
    }
    match std::fs::remove_file(&json_trace) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| {
                format!("failed to remove {}", json_trace.display())
            })
        }
        _ => Ok(()),
    }
}

impl<Session: InterfaceSession> TraceWriter<Session>
    for Box<dyn TraceWriter<Session>>
{
    fn write(
        &mut self,
        state: &<Session as InterfaceSession>::State,
        last_action: Option<&<Session as InterfaceSession>::Action>,
        snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        (**self).write(state, last_action, snapshots, violations)
    }

    fn finish(&mut self) -> Result<()> {
        (**self).finish()
    }
}

pub struct NoopOutputWriter;

impl<Session: InterfaceSession> OutputWriter<Session> for NoopOutputWriter {
    type TraceWriter = NoopTraceWriter;

    fn trace_writer(&mut self, _: RunId) -> Result<Self::TraceWriter> {
        Ok(NoopTraceWriter)
    }
}

pub struct NoopTraceWriter;

impl<Session: InterfaceSession> TraceWriter<Session> for NoopTraceWriter {
    fn write(
        &mut self,
        _state: &Session::State,
        _last_action: Option<&Session::Action>,
        _snapshots: &[Snapshot],
        _violations: &[PropertyViolation],
    ) -> Result<()> {
        Ok(())
    }
}

pub trait InterfaceSession {
    type Action: Clone + Debug + Serialize + DeserializeOwned + Format;
    type ActionTemplate: Clone
        + Debug
        + Serialize
        + DeserializeOwned
        + Format
        + FromGeneratedAction
        + PartialEq
        + ActionTemplate<Self::Action>;
    type State: RunState + Debug;

    /// Amount of time we'll wait for `next_event` to return an event
    /// before considering the driver stalled.
    const STALL_TIMEOUT: Duration;

    fn terminate(&mut self) -> Result<()>;

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<Self::State>>;

    fn apply(
        &mut self,
        action: Self::Action,
        state: Arc<Self::State>,
    ) -> Result<()>;

    fn extract_snapshots(
        &mut self,
        state: Arc<Self::State>,
        last_action: Option<&Self::Action>,
    ) -> Result<Vec<Snapshot>>;

    fn state_timestamp(state: &Self::State) -> SystemTime;
}

#[derive(Debug, Clone)]
pub enum DriverEvent<S> {
    StateChanged(Arc<S>),
    Error(Arc<anyhow::Error>),
    Timeout,
}

/// A directory in `screenshots` that can't be emptied (mode 555) while
/// held, which dropping allows again (mode 755), even if a test fails.
/// `None`, noted on stderr, if permissions don't stop removing it, as for
/// root.
#[cfg(all(test, unix))]
pub(crate) struct LockedScreenshots(std::path::PathBuf);

#[cfg(all(test, unix))]
impl LockedScreenshots {
    pub(crate) fn new(screenshots: &std::path::Path) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        let path = screenshots.join("l");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("0.webp"), "image").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o555))
            .unwrap();
        let locked = LockedScreenshots(path);
        if std::fs::remove_file(locked.0.join("0.webp")).is_ok() {
            eprintln!("skipped: permissions don't stop removing files here");
            return None;
        }
        Some(locked)
    }
}

#[cfg(all(test, unix))]
impl Drop for LockedScreenshots {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::Permissions::from_mode(0o755);
        let _ = std::fs::set_permissions(&self.0, mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first line of a legacy browser trace, nested as written after
    /// v0.5.0.
    const NESTED: &str = r#"{"timestamp":0,"action":null,"state":{"url":"http://localhost/","hash_previous":null,"hash_current":1,"screenshot":"screenshots/0.webp","resources":{}},"snapshots":[],"violations":[]}"#;

    /// The first line of a legacy terminal trace, as written before
    /// `trace.bin`, of typed text (shown in the grid) naming the browser's
    /// fields.
    const TERMINAL: &str = r#"{"timestamp":0,"action":{"TypeText":{"text":"{\"url\":\"u\",\"screenshot\":\"s\"}"}},"state":{"grid":{"cells":[{"Occupied":{"contents":"u","wide":false,"style":{"foreground_color":"None","background_color":"None","underline_color":"None","underline":"None","attributes":0}}}],"size":{"columns":1,"rows":1}},"scrollback":{"cells":[],"size":{"columns":1,"rows":0}},"scroll_offset":0,"cursor":{"position":{"column":0,"row":0},"visible":true,"blinking":false,"visual_style":"Block","color":"None"},"exit_status":null},"snapshots":[],"violations":[]}"#;

    #[test]
    fn tells_a_legacy_browser_trace() {
        let flat = r#"{"timestamp":0,"url":"http://localhost/","hash_previous":null,"hash_current":1,"action":null,"screenshot":"screenshots/0.webp","snapshots":[],"violations":[],"resources":{}}"#;
        let blank_first = format!("\n  \n{NESTED}\n");
        let cases: &[(&[u8], bool)] = &[
            (NESTED.as_bytes(), true),
            (blank_first.as_bytes(), true),
            (flat.as_bytes(), true),
            (TERMINAL.as_bytes(), false),
            (br#"{"state":{"url":"u"}}"#, false),
            (br#"{"state":{"screenshot":"s"}}"#, false),
            (br#"{"state":{"url":"u","screenshot":1}}"#, false),
            (br#"{"url":"u","screenshot":1}"#, false),
            (b"\xff\xfe{}\n", false),
            (b"previous", false),
            (b"", false),
        ];
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(JSON_TRACE_FILE_NAME);
        for (contents, expected) in cases {
            std::fs::write(&path, contents).unwrap();
            assert_eq!(
                is_legacy_browser_trace(&path),
                *expected,
                "{}",
                String::from_utf8_lossy(contents)
            );
        }
        assert!(!is_legacy_browser_trace(&directory.path().join("missing")));

        // Not a regular file: not opened, so a FIFO doesn't hang.
        #[cfg(unix)]
        {
            let fifo = directory.path().join("fifo");
            let made = std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .is_ok_and(|status| status.success());
            assert!(made || !cfg!(target_os = "linux"), "mkfifo failed");
            if made {
                let (sender, receiver) = std::sync::mpsc::channel();
                let checked = fifo.clone();
                std::thread::spawn(move || {
                    sender.send(is_legacy_browser_trace(&checked))
                });
                let timeout = std::time::Duration::from_secs(5);
                match receiver.recv_timeout(timeout) {
                    Ok(result) => assert!(!result),
                    Err(_) => {
                        // Unblocks the reader opening it.
                        let _ =
                            std::fs::OpenOptions::new().write(true).open(&fifo);
                        panic!("opened a FIFO, and hung");
                    }
                }
            }
            std::fs::write(&path, NESTED).unwrap();
            let link = directory.path().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(!is_legacy_browser_trace(&link));
        }
    }

    /// A browser trace's `screenshots` that can't be removed is only
    /// logged, and keeps the trace, so overwriting again, once it can be
    /// removed, removes both; `keep_screenshots` keeps them.
    #[cfg(unix)]
    #[test]
    fn removes_a_legacy_trace_after_its_screenshots() {
        let directory = tempfile::tempdir().unwrap();
        let json_trace = directory.path().join(JSON_TRACE_FILE_NAME);
        let screenshots =
            directory.path().join(JSON_TRACE_SCREENSHOTS_DIRECTORY);
        std::fs::write(&json_trace, NESTED).unwrap();
        let Some(locked) = LockedScreenshots::new(&screenshots) else {
            return;
        };
        let locked_path = locked.0.clone();
        remove_legacy_trace(directory.path(), false).unwrap();
        assert!(json_trace.is_file());
        assert!(locked_path.is_dir());
        drop(locked);
        remove_legacy_trace(directory.path(), true).unwrap();
        assert!(!json_trace.exists());
        assert!(locked_path.is_dir());
        std::fs::write(&json_trace, NESTED).unwrap();
        remove_legacy_trace(directory.path(), false).unwrap();
        assert!(!json_trace.exists());
        assert!(!screenshots.exists());
        // Nothing to remove.
        remove_legacy_trace(directory.path(), false).unwrap();
    }
}
