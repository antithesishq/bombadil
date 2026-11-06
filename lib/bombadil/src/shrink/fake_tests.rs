use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::File,
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufWriter, Write},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, OnceLock, atomic::AtomicBool},
    time::{Duration, SystemTime},
};

use anyhow::Result;
use bombadil_schema::Time;
use hegel::{
    Generator, TestCase,
    generators::{floats, hashsets, integers, just, one_of, vecs},
};
use rand::{RngExt, SeedableRng, rngs::StdRng};
use serde::{Deserialize, Serialize};
use serde_json as json;
use tempfile::NamedTempFile;

use super::{
    ShrinkExitReason, ShrinkOptions, ShrinkSettings, TraceActions,
    load_trace_actions, shrink,
};
use crate::{
    driver::{
        ActionTemplate, DriverEvent, FromGeneratedAction, InterfaceDriver,
        InterfaceSession, RunId, RunState, TraceWriter,
    },
    render::Format,
    runner::{ControlFlow, PropertiesState, PropertyViolation, RunStrategy},
    specification::{bundler::bundle, domain::Snapshot, verifier::Verifier},
    tree::Tree,
};

const ALPHABET_SIZE: u8 = 8;
const PROPERTY_NAME: &str = "no_bug";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct FakeAction(u8);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Flakiness {
    Deterministic,
    Flaky(f64),
    Broken,
}

impl Flakiness {
    fn drop_probability(self) -> f64 {
        match self {
            Flakiness::Deterministic => 0.0,
            Flakiness::Flaky(probability) => probability,
            Flakiness::Broken => 1.0,
        }
    }
}

impl Format for FakeAction {
    fn format(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "action {}", self.0)
    }
}

impl ActionTemplate<FakeAction> for FakeAction {
    fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        _rng: &mut Rng,
    ) -> FakeAction {
        *self
    }

    fn accepts(&self, original: &FakeAction) -> bool {
        self == original
    }

    fn category_hash<H: Hasher>(&self, hasher: &mut H) {
        hasher.write_u8(self.0)
    }
}

impl FromGeneratedAction for FakeAction {
    fn from_generated(value: json::Value) -> Result<Self> {
        Ok(json::from_value(value)?)
    }
}

#[derive(Debug, Clone, Default)]
struct FakeState {
    applied: BTreeSet<u8>,
    step: u64,
}

impl FakeState {
    fn timestamp_system(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(self.step)
    }

    fn available(&self, action: u8) -> bool {
        action.is_multiple_of(2)
            || self.applied.iter().any(|applied| applied.is_multiple_of(2))
    }

    fn bug(&self, triggers: &BTreeSet<u8>) -> bool {
        triggers.is_subset(&self.applied)
    }
}

impl RunState for FakeState {
    fn timestamp(&self) -> Time {
        Time::from_system_time(self.timestamp_system())
    }
}

fn replay(sequence: &[u8], triggers: &BTreeSet<u8>) -> Option<Vec<u8>> {
    let mut state = FakeState::default();
    let mut executed = Vec::new();
    for &action in sequence {
        if state.bug(triggers) {
            break;
        }
        if state.available(action) {
            state.applied.insert(action);
            executed.push(action);
        }
    }
    state.bug(triggers).then_some(executed)
}

struct FakeDriver {
    bundle_code: &'static str,
    triggers: BTreeSet<u8>,
    flakiness: Flakiness,
    seed: u64,
}

impl InterfaceDriver for FakeDriver {
    type Session = FakeSession;

    fn new_session(&self, run_id: RunId) -> Result<(FakeSession, Verifier)> {
        let mut hasher = DefaultHasher::new();
        (self.seed, run_id.0).hash(&mut hasher);
        let state = FakeState::default();
        Ok((
            FakeSession {
                triggers: self.triggers.clone(),
                drop_probability: self.flakiness.drop_probability(),
                rng: StdRng::seed_from_u64(hasher.finish()),
                state: state.clone(),
                pending: Some(Arc::new(state)),
            },
            Verifier::new(self.bundle_code)?,
        ))
    }
}

struct FakeSession {
    triggers: BTreeSet<u8>,
    drop_probability: f64,
    rng: StdRng,
    state: FakeState,
    pending: Option<Arc<FakeState>>,
}

impl InterfaceSession for FakeSession {
    type Action = FakeAction;
    type ActionTemplate = FakeAction;
    type State = FakeState;

    const STALL_TIMEOUT: Duration = Duration::from_secs(1);

    fn terminate(&mut self) -> Result<()> {
        Ok(())
    }

    fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Option<DriverEvent<FakeState>> {
        match self.pending.take() {
            Some(state) => Some(DriverEvent::StateChanged(state)),
            None => {
                std::thread::sleep(timeout);
                Some(DriverEvent::Timeout)
            }
        }
    }

    fn apply(
        &mut self,
        action: FakeAction,
        _state: Arc<FakeState>,
    ) -> Result<()> {
        if !self.rng.random_bool(self.drop_probability) {
            self.state.applied.insert(action.0);
        }
        self.state.step += 1;
        self.pending = Some(Arc::new(self.state.clone()));
        Ok(())
    }

    fn extract_snapshots(
        &mut self,
        state: Arc<FakeState>,
        _last_action: Option<&FakeAction>,
    ) -> Result<Vec<Snapshot>> {
        Ok(vec![Snapshot {
            index: 0,
            name: None,
            value: json::json!(state.bug(&self.triggers)),
            time: state.timestamp(),
        }])
    }

    fn state_timestamp(state: &FakeState) -> SystemTime {
        state.timestamp_system()
    }
}

struct FakeStrategy {
    steps_max: u64,
}

impl RunStrategy<FakeSession> for FakeStrategy {
    type StopValue = ();

    fn on_new_state(
        &mut self,
        state: &FakeState,
        tree: Tree<FakeAction>,
        _last_action: Option<&FakeAction>,
        _snapshots: &[Snapshot],
        _properties: PropertiesState,
    ) -> Result<ControlFlow<(), FakeAction>> {
        if state.step >= self.steps_max {
            return Ok(ControlFlow::Stop(()));
        }
        match tree.values().first() {
            Some(&&action) => Ok(ControlFlow::Continue(action)),
            None => Ok(ControlFlow::Stop(())),
        }
    }

    fn deadline(&self) -> Option<Time> {
        None
    }

    fn on_time_limit(&mut self) -> Result<()> {
        Ok(())
    }

    fn on_stalled(&mut self) -> Result<()> {
        Ok(())
    }

    fn on_interrupted(&mut self) -> Result<()> {
        Ok(())
    }

    fn constrain(
        &self,
        state: &FakeState,
        tree: Tree<FakeAction>,
    ) -> Tree<FakeAction> {
        tree.filter(&|action| state.available(action.0))
    }
}

enum FakeTraceWriter {
    Directory(BufWriter<File>),
    File(bombadil_trace::TraceFileWriter<json::Value>),
}

impl FakeTraceWriter {
    fn new(directory: PathBuf) -> Result<Self> {
        Self::with_file_name(directory, "trace.jsonl")
    }

    fn with_file_name(directory: PathBuf, file_name: &str) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        Ok(Self::Directory(BufWriter::new(File::create(
            directory.join(file_name),
        )?)))
    }

    fn file(path: PathBuf) -> Result<Self> {
        Ok(Self::File(bombadil_trace::TraceFileWriter::create(
            &path,
            bombadil_trace::TraceDriver::Terminal,
            false,
        )?))
    }
}

impl TraceWriter<FakeSession> for FakeTraceWriter {
    fn write(
        &mut self,
        _state: &FakeState,
        last_action: Option<&FakeAction>,
        _snapshots: &[Snapshot],
        violations: &[PropertyViolation],
    ) -> Result<()> {
        let names: Vec<_> = violations
            .iter()
            .map(|violation| json::json!({ "name": violation.name }))
            .collect();
        let metadata =
            json::json!({ "action": last_action, "violations": names });
        match self {
            Self::Directory(writer) => {
                json::to_writer(&mut *writer, &metadata)?;
                writeln!(writer)?;
                Ok(())
            }
            Self::File(writer) => writer.write_state(metadata, None),
        }
    }

    fn finish(&mut self) -> Result<()> {
        match self {
            Self::Directory(writer) => Ok(writer.flush()?),
            Self::File(writer) => writer.finish(),
        }
    }
}

fn bundle_code() -> &'static str {
    static BUNDLE_CODE: OnceLock<String> = OnceLock::new();
    BUNDLE_CODE.get_or_init(|| {
        let alphabet: Vec<u8> = (0..ALPHABET_SIZE).collect();
        let specification = format!(
            r#"
            import {{ actions, always, extract }} from "@antithesishq/bombadil";
            export const _actions = actions(() => {alphabet:?});
            const bug = extract((state) => state);
            export const {PROPERTY_NAME} = always(() => !bug.current);
            "#
        );
        let mut file = NamedTempFile::with_suffix(".ts").unwrap();
        file.write_all(specification.as_bytes()).unwrap();
        bundle(".", &file.path().display().to_string()).unwrap()
    })
}

#[hegel::test(test_cases = 50)]
fn shrinks_to_failing_sequence(tc: TestCase) {
    let actions = || integers::<u8>().max_value(ALPHABET_SIZE - 1);
    let triggers: HashSet<u8> =
        tc.draw(hashsets(actions()).min_size(1).max_size(3));
    let triggers: BTreeSet<u8> = triggers.into_iter().collect();
    let noise: Vec<u8> = tc.draw(vecs(actions()).max_size(30));
    let tail: Vec<u8> = tc.draw(vecs(actions()).max_size(5));
    let flakiness: Flakiness = tc.draw(
        one_of([
            just(Flakiness::Deterministic).boxed(),
            floats()
                .min_value(0.05)
                .max_value(0.5)
                .map(Flakiness::Flaky)
                .boxed(),
            just(Flakiness::Broken).boxed(),
        ])
        .print_as_debug(),
    );
    let parallelism: usize = tc.draw(integers().min_value(1).max_value(4));
    let seed: u64 = tc.draw(integers());

    let original: Vec<u8> = noise
        .iter()
        .copied()
        .chain([0])
        .chain(triggers.iter().copied())
        .collect();
    let executed = replay(&original, &triggers).expect("bug is triggered");
    let trace = TraceActions {
        actions: executed
            .iter()
            .chain(&tail)
            .map(|&action| FakeAction(action))
            .collect(),
        violations_earliest: BTreeMap::from([(
            PROPERTY_NAME.to_string(),
            executed.len(),
        )]),
    };

    let steps_max = executed.len() as u64 + 1;
    let output = tempfile::tempdir().unwrap();
    let result = shrink(ShrinkOptions {
        driver: Arc::new(FakeDriver {
            bundle_code: bundle_code(),
            triggers: triggers.clone(),
            flakiness,
            seed,
        }),
        strategy: move || FakeStrategy { steps_max },
        trace_writer: FakeTraceWriter::new,
        trace,
        property_name: None,
        output_path: output.path().to_path_buf(),
        output_path_overwrite: false,
        parallelism: NonZeroUsize::new(parallelism).unwrap(),
        interrupted: Arc::new(AtomicBool::new(false)),
        time_limit: Some(Duration::from_secs(60)),
        settings: ShrinkSettings {
            calibration_runs: NonZeroUsize::new(10).unwrap(),
            ..ShrinkSettings::default()
        },
    });

    let assert_not_reproduced = |error: anyhow::Error| {
        assert!(
            format!("{error:#}").contains("did not reproduce"),
            "unexpected error: {error:#}"
        );
        assert!(
            !output.path().join("trace.jsonl").exists(),
            "no trace should be written"
        );
        tc.event("calibration did not reproduce");
    };
    let result = match (flakiness, result) {
        (Flakiness::Broken, Ok(result)) => {
            panic!("shrinking a trace that never reproduces: {result:?}")
        }
        (Flakiness::Broken, Err(error)) => {
            assert_not_reproduced(error);
            return;
        }
        (Flakiness::Flaky(_), Err(error)) => {
            assert_not_reproduced(error);
            return;
        }
        (Flakiness::Deterministic, Err(error)) => {
            panic!("shrinking failed: {error:#}")
        }
        (_, Ok(result)) => result,
    };
    tc.event_value("run count", result.run_count as f64);
    tc.event_value(
        "calibration hit rate",
        result.calibration.hit_count as f64
            / result.calibration.run_count as f64,
    );

    assert_eq!(result.exit_reason, ShrinkExitReason::Minimal);
    assert_eq!(result.property_name, PROPERTY_NAME);
    assert_eq!(result.trace_length_truncated, executed.len());
    assert!(result.trace_length_shrunk <= result.trace_length_truncated);
    assert!(result.confirmation.hit_count >= 1);
    assert!(
        !output.path().join(".shrink-attempts").exists(),
        "attempts directory should be removed"
    );

    let shrunk =
        load_trace_actions::<FakeAction>(&output.path().join("trace.jsonl"))
            .unwrap();
    let shrunk_actions: Vec<u8> =
        shrunk.actions.iter().map(|action| action.0).collect();
    assert_eq!(shrunk_actions.len(), result.trace_length_shrunk);
    assert_eq!(
        shrunk.violations_earliest.get(PROPERTY_NAME),
        Some(&shrunk_actions.len()),
        "the shrunk trace should end at its violation"
    );
    assert!(
        replay(&shrunk_actions, &triggers).is_some(),
        "shrunk sequence {shrunk_actions:?} doesn't trigger {triggers:?}"
    );

    if flakiness == Flakiness::Deterministic {
        assert_eq!(
            replay(&shrunk_actions, &triggers).as_ref(),
            Some(&shrunk_actions),
            "every shrunk action should be executed"
        );
        for index in 0..shrunk_actions.len() {
            let mut smaller = shrunk_actions.clone();
            smaller.remove(index);
            assert!(
                replay(&smaller, &triggers).is_none(),
                "{shrunk_actions:?} is not minimal, {smaller:?} also fails"
            );
        }
    }
}

/// Shrinks a short deterministic trace into `output`, with the driver
/// writing its trace as `trace_file`.
fn shrink_into(
    output: &std::path::Path,
    overwrite: bool,
    trace_file: &'static str,
) -> Result<super::ShrinkResult> {
    shrink_with_writer(output, overwrite, move |directory| {
        FakeTraceWriter::with_file_name(directory, trace_file)
    })
}

fn shrink_with_writer(
    output: &std::path::Path,
    overwrite: bool,
    trace_writer: impl Fn(PathBuf) -> Result<FakeTraceWriter>
    + Send
    + Sync
    + 'static,
) -> Result<super::ShrinkResult> {
    shrink_actions_with_writer(output, overwrite, trace_writer, &[0, 1])
}

fn shrink_actions_with_writer(
    output: &std::path::Path,
    overwrite: bool,
    trace_writer: impl Fn(PathBuf) -> Result<FakeTraceWriter>
    + Send
    + Sync
    + 'static,
    original: &[u8],
) -> Result<super::ShrinkResult> {
    let triggers = BTreeSet::from([1]);
    let executed = replay(original, &triggers).expect("bug is triggered");
    let trace = TraceActions {
        actions: executed.iter().map(|&action| FakeAction(action)).collect(),
        violations_earliest: BTreeMap::from([(
            PROPERTY_NAME.to_string(),
            executed.len(),
        )]),
    };
    let steps_max = executed.len() as u64 + 1;
    shrink(ShrinkOptions {
        driver: Arc::new(FakeDriver {
            bundle_code: bundle_code(),
            triggers,
            flakiness: Flakiness::Deterministic,
            seed: 0,
        }),
        strategy: move || FakeStrategy { steps_max },
        trace_writer,
        trace,
        property_name: None,
        output_path: output.to_path_buf(),
        output_path_overwrite: overwrite,
        parallelism: NonZeroUsize::new(1).unwrap(),
        interrupted: Arc::new(AtomicBool::new(false)),
        time_limit: Some(Duration::from_secs(60)),
        settings: ShrinkSettings {
            calibration_runs: NonZeroUsize::new(2).unwrap(),
            ..ShrinkSettings::default()
        },
    })
}

#[test]
fn shrinks_to_arbitrary_files_without_touching_siblings() {
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("arbitrary.custom");
    let sibling = output.path().join("trace.jsonl");
    std::fs::write(&sibling, b"kept").unwrap();
    for overwrite in [false, true] {
        shrink_with_writer(&path, overwrite, FakeTraceWriter::file).unwrap();
        let mut reader =
            bombadil_trace::open_trace::<json::Value>(&path).unwrap();
        assert!(reader.by_ref().count() > 0);
        assert!(reader.finished());
        assert_eq!(std::fs::read(&sibling).unwrap(), b"kept");
    }
    let before = std::fs::read(&path).unwrap();
    assert!(shrink_with_writer(&path, false, FakeTraceWriter::file).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 2);
    let missing = output.path().join("missing");
    assert!(
        shrink_with_writer(&missing.join("run"), false, FakeTraceWriter::file)
            .is_err()
    );
    assert!(!missing.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn shrinks_to_name_max_files() {
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("x".repeat(255));
    for overwrite in [false, true] {
        shrink_with_writer(&path, overwrite, FakeTraceWriter::file).unwrap();
        assert!(path.is_file());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn shrinks_across_filesystems_when_available() {
    use std::os::unix::fs::MetadataExt;
    let attempts = tempfile::tempdir().unwrap();
    let other = match tempfile::TempDir::new_in("/dev/shm") {
        Ok(other) => other,
        Err(error) => {
            eprintln!(
                "skipping cross-filesystem coverage: /dev/shm is unavailable: {error}"
            );
            return;
        }
    };
    if attempts.path().metadata().unwrap().dev()
        == other.path().metadata().unwrap().dev()
    {
        eprintln!(
            "skipping cross-filesystem coverage: system temp and /dev/shm share a device"
        );
        return;
    }
    if let Err(error) =
        std::fs::write(other.path().join("writable-probe"), b"probe")
    {
        eprintln!(
            "skipping cross-filesystem coverage: fixture is not writable: {error}"
        );
        return;
    }
    let path = other.path().join("cross-filesystem.custom");
    shrink_with_writer(&path, false, FakeTraceWriter::file).unwrap();
    assert!(path.is_file());
    // The still-directory-based browser also publishes independent attempts.
    let browser = other.path().join("browser");
    shrink_with_writer(&browser, false, |attempt| {
        let writer = FakeTraceWriter::new(attempt.clone())?;
        let screenshots = attempt.join("screenshots");
        std::fs::create_dir(&screenshots)?;
        std::fs::write(screenshots.join("0.png"), b"image")?;
        Ok(writer)
    })
    .unwrap();
    assert!(browser.join("trace.jsonl").is_file());
    assert_eq!(
        std::fs::read(browser.join("screenshots/0.png")).unwrap(),
        b"image"
    );
}

#[test]
fn file_publication_does_not_clobber_a_concurrent_creator() {
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("race.custom");
    let target = path.clone();
    let error = shrink_with_writer(&path, false, move |attempt| {
        std::fs::write(&target, b"concurrent creator")?;
        FakeTraceWriter::file(attempt)
    })
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"concurrent creator");
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 1);
}

#[test]
fn attempts_are_independent_and_cleaned_after_success_and_failure() {
    for (file, fail) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let output = tempfile::tempdir().unwrap();
        let path = output.path().join("result");
        let attempts = Arc::new(std::sync::Mutex::new(BTreeSet::new()));
        let recorded = attempts.clone();
        let result = shrink_with_writer(&path, false, move |attempt| {
            recorded
                .lock()
                .unwrap()
                .insert(attempt.parent().unwrap().to_path_buf());
            if fail {
                anyhow::bail!("injected attempt failure");
            }
            if file {
                FakeTraceWriter::file(attempt)
            } else {
                FakeTraceWriter::new(attempt)
            }
        });
        assert_eq!(result.is_err(), fail);
        let attempts = attempts.lock().unwrap();
        assert!(!attempts.is_empty());
        for attempt in attempts.iter() {
            assert!(!attempt.starts_with(output.path()));
            assert!(!attempt.exists(), "{} was not cleaned", attempt.display());
        }
    }
}

/// Browser refusal is limited to its JSON trace, not independent files.
#[test]
fn refuses_an_existing_trace_without_overwriting() {
    let output = tempfile::tempdir().unwrap();
    std::fs::write(output.path().join("trace.jsonl"), "previous").unwrap();
    let error = shrink_into(output.path(), false, "trace.jsonl")
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .starts_with("trace.jsonl already exists at"),
        "{error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(output.path().join("trace.jsonl")).unwrap(),
        "previous"
    );
}

#[test]
fn browser_publication_preserves_an_independent_terminal_trace() {
    let output = tempfile::tempdir().unwrap();
    let binary = output.path().join("trace.bin");
    let mut writer = bombadil_trace::TraceFileWriter::create(
        &binary,
        bombadil_trace::TraceDriver::Terminal,
        false,
    )
    .unwrap();
    writer
        .write_state(json::json!({"action": null}), None)
        .unwrap();
    writer.finish().unwrap();
    let before = std::fs::read(&binary).unwrap();
    for overwrite in [false, true] {
        shrink_into(output.path(), overwrite, "trace.jsonl").unwrap();
        assert_eq!(std::fs::read(&binary).unwrap(), before);
        assert!(shrink_into(output.path(), false, "trace.jsonl").is_err());
        assert_eq!(std::fs::read(&binary).unwrap(), before);
    }
}

#[cfg(unix)]
#[test]
fn failed_replacement_copy_preserves_the_published_browser_result() {
    let output = tempfile::tempdir().unwrap();
    let target = output.path().to_path_buf();
    let original = Arc::new(std::sync::Mutex::new(None));
    let captured = original.clone();
    let error = shrink_actions_with_writer(
        output.path(),
        false,
        move |attempt| {
            let published = target.join("trace.jsonl");
            let replacing = published.is_file();
            if replacing {
                *captured.lock().unwrap() = Some(std::fs::read(&published)?);
            }
            let writer = FakeTraceWriter::new(attempt.clone())?;
            let screenshots = attempt.join("screenshots");
            std::fs::create_dir(&screenshots)?;
            std::fs::write(
                screenshots.join("0.png"),
                if replacing { &b"new"[..] } else { &b"old"[..] },
            )?;
            if replacing {
                // A deterministic copy failure on any filesystem, after the
                // initial calibration result has already been published.
                std::os::unix::fs::symlink(
                    "missing",
                    screenshots.join("broken.png"),
                )?;
            }
            Ok(writer)
        },
        &[0, 2, 1],
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(
        std::fs::read(output.path().join("trace.jsonl")).unwrap(),
        original.lock().unwrap().as_ref().unwrap().as_slice()
    );
    assert_eq!(
        std::fs::read(output.path().join("screenshots/0.png")).unwrap(),
        b"old"
    );
    assert!(!output.path().join("screenshots/broken.png").exists());
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 2);
}
