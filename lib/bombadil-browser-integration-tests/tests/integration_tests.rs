use anyhow::anyhow;
use axum::{
    Router,
    extract::Path,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use bombadil_schema::{Time, markup};
use rand::SeedableRng;
use std::io::Write;
use std::{
    collections::HashMap,
    fmt::Display,
    path::Path as FsPath,
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use tempfile::{NamedTempFile, TempDir};
use tokio::sync::Semaphore;
use tower_http::services::ServeDir;
use url::Url;

use bombadil::{specification::verifier::Specification, styled};
use bombadil_browser::{
    browser::{
        Browser, BrowserOptions, DebuggerOptions, DownloadBehavior, Emulation,
        LaunchOptions, actions::BrowserAction,
    },
    convert::ToSchema,
    cookie::BrowserCookie,
    runner,
    strategy::{TestStrategy, TraceWriter},
};

/// These tests are pretty heavy, and running too many parallel risks one browser get stuck and
/// causing a test to hang, so we limit parallelism.
static TEST_SEMAPHORE: Semaphore = Semaphore::const_new(16);
const TEST_TIMEOUT_SECONDS: u64 = 120;
const ARTIFACT_OBSERVATION_INTERVAL: Duration = Duration::from_millis(5);
// Chromium's chrome_browser_main_extra_parts_optimization_guide.cc places the
// active model store under DIR_USER_DATA, optimization_guide_constants.cc
// retains the old downloads/metadata/hint stores under the profile, and
// optimization_guide_on_device_model_installer.cc installs the on-device model
// component under OptGuideOnDeviceModel.
const OPTIMIZATION_GUIDE_ARTIFACT_ROOTS: [&str; 5] = [
    "optimization_guide_model_store",
    "OptGuideOnDeviceModel",
    "Default/optimization_guide_prediction_model_downloads",
    "Default/optimization_guide_model_metadata_store",
    "Default/optimization_guide_hint_cache_store",
];

static INIT: Once = Once::new();

struct ArtifactMonitor {
    handle: thread::JoinHandle<()>,
    observations: Arc<Mutex<Vec<Vec<String>>>>,
    stop: Arc<AtomicBool>,
}

impl ArtifactMonitor {
    fn start(snapshot: impl Fn() -> Vec<String> + Send + 'static) -> Self {
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observations_for_thread = Arc::clone(&observations);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(0);
        let handle = thread::spawn(move || {
            let mut ready_sender = Some(ready_sender);
            loop {
                let current_snapshot = snapshot();
                if !current_snapshot.is_empty() {
                    let mut observations =
                        observations_for_thread.lock().unwrap();
                    if observations.last() != Some(&current_snapshot) {
                        observations.push(current_snapshot);
                    }
                }

                if let Some(ready_sender) = ready_sender.take() {
                    let _ = ready_sender.send(());
                }
                if stop_for_thread.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(ARTIFACT_OBSERVATION_INTERVAL);
            }
        });
        ready_receiver.recv().unwrap();

        Self {
            handle,
            observations,
            stop,
        }
    }

    fn finish(self) -> Vec<Vec<String>> {
        self.stop.store(true, Ordering::Release);
        let thread_panicked = self.handle.join().is_err();
        let mut observations = self.observations.lock().unwrap().clone();
        if thread_panicked {
            observations.push(vec!["<artifact observer panicked>".to_string()]);
        }
        observations
    }

    fn wait_for_observation(&self, fragment: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .observations
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .any(|entry| entry.contains(fragment))
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(ARTIFACT_OBSERVATION_INTERVAL);
        }
    }
}

fn download_directory_snapshot(path: &FsPath) -> Vec<String> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            return vec![format!("<could not read directory: {error}>")];
        }
    };
    let mut snapshot = entries
        .map(|entry| match entry {
            Ok(entry) => {
                let name = entry.file_name().to_string_lossy().into_owned();
                match std::fs::symlink_metadata(entry.path()) {
                    Ok(metadata) => format!(
                        "{name}: {} bytes, file={}, directory={}, symlink={}",
                        metadata.len(),
                        metadata.is_file(),
                        metadata.is_dir(),
                        metadata.file_type().is_symlink()
                    ),
                    Err(error) => {
                        format!("{name}: disappeared before metadata: {error}")
                    }
                }
            }
            Err(error) => format!("<could not read entry: {error}>"),
        })
        .collect::<Vec<_>>();
    snapshot.sort();
    snapshot
}

fn download_directory_contents(path: &FsPath) -> std::io::Result<Vec<Vec<u8>>> {
    let entries =
        std::fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
    let mut contents = Vec::with_capacity(entries.len());
    for entry in entries {
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "download output is not a regular file: {}",
                    entry.path().display()
                ),
            ));
        }
        contents.push(std::fs::read(entry.path())?);
    }
    contents.sort();
    Ok(contents)
}

fn optimization_guide_profile_snapshot(path: &FsPath) -> Vec<String> {
    let mut snapshot = Vec::new();
    for relative_root in OPTIMIZATION_GUIDE_ARTIFACT_ROOTS {
        append_artifact_tree(path, &path.join(relative_root), &mut snapshot);
    }
    snapshot.sort();
    snapshot
}

#[test]
fn artifact_monitor_records_transient_entries_across_boundaries() {
    const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(1);

    let downloads_directory = TempDir::new().unwrap();
    let downloads_path = downloads_directory.path().to_path_buf();
    let download_monitor = ArtifactMonitor::start(move || {
        download_directory_snapshot(&downloads_path)
    });
    let transient_download = downloads_directory.path().join("transient.crx3");
    std::fs::write(&transient_download, b"synthetic download").unwrap();
    assert!(
        download_monitor
            .wait_for_observation("transient.crx3", OBSERVATION_TIMEOUT)
    );
    std::fs::remove_file(transient_download).unwrap();
    assert!(download_directory_snapshot(downloads_directory.path()).is_empty());
    let download_observations = download_monitor.finish();
    assert!(
        download_observations
            .iter()
            .flatten()
            .any(|entry| entry.contains("transient.crx3"))
    );

    let profile_directory = TempDir::new().unwrap();
    let profile_path = profile_directory.path().to_path_buf();
    let profile_monitor = ArtifactMonitor::start(move || {
        optimization_guide_profile_snapshot(&profile_path)
    });
    for (index, relative_root) in
        OPTIMIZATION_GUIDE_ARTIFACT_ROOTS.into_iter().enumerate()
    {
        let artifact_directory = profile_directory
            .path()
            .join(relative_root)
            .join(format!("00000000-0000-4000-8000-{index:012}"));
        std::fs::create_dir_all(&artifact_directory).unwrap();
        let artifact_name = if relative_root.contains("downloads") {
            "payload.crx3"
        } else {
            "model.tflite"
        };
        let artifact = artifact_directory.join(artifact_name);
        std::fs::write(&artifact, b"synthetic Optimization Guide artifact")
            .unwrap();
        let artifact_fragment = artifact
            .strip_prefix(profile_directory.path())
            .unwrap()
            .display()
            .to_string();
        assert!(
            profile_monitor
                .wait_for_observation(&artifact_fragment, OBSERVATION_TIMEOUT)
        );
        std::fs::remove_dir_all(profile_directory.path().join(relative_root))
            .unwrap();
    }
    assert!(
        optimization_guide_profile_snapshot(profile_directory.path())
            .is_empty()
    );
    let profile_observations = profile_monitor.finish();
    for relative_root in OPTIMIZATION_GUIDE_ARTIFACT_ROOTS {
        assert!(
            profile_observations
                .iter()
                .flatten()
                .any(|entry| entry.contains(relative_root)),
            "missing transient observation for {relative_root}"
        );
    }
}

fn append_artifact_tree(
    profile_root: &FsPath,
    path: &FsPath,
    snapshot: &mut Vec<String>,
) {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            snapshot.push(format!(
                "{}: could not read metadata: {error}",
                path.strip_prefix(profile_root).unwrap_or(path).display()
            ));
            return;
        }
    };
    let relative_path = path.strip_prefix(profile_root).unwrap_or(path);
    snapshot.push(format!(
        "{}: {} bytes, file={}, directory={}, symlink={}",
        relative_path.display(),
        metadata.len(),
        metadata.is_file(),
        metadata.is_dir(),
        metadata.file_type().is_symlink()
    ));

    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return;
    }
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            snapshot.push(format!(
                "{}: could not read directory: {error}",
                relative_path.display()
            ));
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(entry) => {
                append_artifact_tree(profile_root, &entry.path(), snapshot);
            }
            Err(error) => snapshot.push(format!(
                "{}: could not read entry: {error}",
                relative_path.display()
            )),
        }
    }
}

fn panic_with_artifact_evidence(
    downloads_directory: TempDir,
    user_data_directory: TempDir,
    failure: &str,
    download_observations: &[Vec<String>],
    profile_observations: &[Vec<String>],
) -> ! {
    let ledger_path = downloads_directory.path().join("observations.txt");
    let ledger = format!(
        "failure:\n{failure}\n\nobserved download directory states:\n{download_observations:#?}\n\nobserved Optimization Guide profile states:\n{profile_observations:#?}\n"
    );
    let ledger_error = std::fs::write(&ledger_path, ledger).err();
    let downloads_evidence_path = downloads_directory.keep();
    let profile_evidence_path = user_data_directory.keep();
    match ledger_error {
        Some(error) => panic!(
            "{failure}\n\ndownload evidence preserved at {}; Chrome profile evidence preserved at {}; could not write observation ledger: {error}",
            downloads_evidence_path.display(),
            profile_evidence_path.display()
        ),
        None => panic!(
            "{failure}\n\ndownload evidence preserved at {}; Chrome profile evidence preserved at {}",
            downloads_evidence_path.display(),
            profile_evidence_path.display()
        ),
    }
}

fn setup() {
    INIT.call_once(|| {
        let env = env_logger::Env::default().default_filter_or("debug");
        env_logger::Builder::from_env(env)
            .format_timestamp_millis()
            .format_target(true)
            .is_test(true)
            .filter_module("html5ever", log::LevelFilter::Warn)
            // Until we hav a fix for https://github.com/mattsse/chromiumoxide/issues/287
            .filter_module("chromiumoxide::browser", log::LevelFilter::Error)
            .init();
    });
}

enum Expect {
    Error {
        substring: &'static str,
        forbidden_substrings: Vec<&'static str>,
    },
    Success,
}

impl Display for Expect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expect::Error { substring, .. } => {
                write!(f, "expecting an error with substring {:?}", substring)
            }
            Expect::Success => write!(f, "expecting success"),
        }
    }
}

struct BrowserIntegrationTest<'a> {
    seed: u64,
    name: &'a str,
    expect: Expect,
    expect_empty_downloads: bool,
    time_limit: Option<Duration>,
    specification: Option<&'a str>,
    grant_permissions: Vec<String>,
    extra_headers: HashMap<String, String>,
    cookies: Vec<BrowserCookie>,
    download_behavior: DownloadBehavior,
    expected_download_contents: Option<Vec<Vec<u8>>>,
}

impl<'a> BrowserIntegrationTest<'a> {
    fn new(name: &'a str) -> Self {
        Self {
            seed: rand::random(),
            name,
            expect: Expect::Success,
            expect_empty_downloads: false,
            time_limit: None,
            specification: None,
            grant_permissions: vec![],
            extra_headers: HashMap::new(),
            cookies: vec![],
            download_behavior: DownloadBehavior::AllowAndName,
            expected_download_contents: None,
        }
    }

    #[allow(dead_code, reason = "can be overridden to reproduce failures")]
    fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    fn expect_error(mut self, substring: &'static str) -> Self {
        self.expect = Expect::Error {
            substring,
            forbidden_substrings: vec![],
        };
        self
    }

    fn expect_error_without(
        mut self,
        substring: &'static str,
        forbidden_substrings: &[&'static str],
    ) -> Self {
        self.expect = Expect::Error {
            substring,
            forbidden_substrings: forbidden_substrings.to_vec(),
        };
        self
    }

    fn expect_empty_downloads(mut self) -> Self {
        self.expect_empty_downloads = true;
        self
    }

    fn time_limit(mut self, duration: Duration) -> Self {
        self.time_limit = Some(duration);
        self
    }

    fn specification(mut self, specification: &'a str) -> Self {
        self.specification = Some(specification);
        self
    }

    fn grant_permissions(mut self, permissions: Vec<String>) -> Self {
        self.grant_permissions = permissions;
        self
    }

    fn extra_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.extra_headers = headers;
        self
    }

    fn cookies(mut self, cookies: Vec<BrowserCookie>) -> Self {
        self.cookies = cookies;
        self
    }

    fn download_behavior(mut self, behavior: DownloadBehavior) -> Self {
        self.download_behavior = behavior;
        self
    }

    fn expect_download_contents(mut self, contents: &[u8]) -> Self {
        self.expected_download_contents = Some(vec![contents.to_vec()]);
        self
    }

    fn expect_no_downloads(mut self) -> Self {
        self.expected_download_contents = Some(vec![]);
        self
    }

    /// Run a named browser test with a given expectation.
    ///
    /// Spins up two web servers: one on a random port P, and one on port P + 1, in order to
    /// facitiliate multi-domain tests.
    ///
    /// The test starts at:
    ///
    ///     http://localhost:{P}/tests/{name}.
    ///
    /// Which means that every named test case directory should have an index.html file.
    async fn run(self) {
        let Self {
            seed,
            name,
            expect,
            expect_empty_downloads,
            time_limit,
            specification,
            grant_permissions,
            extra_headers,
            cookies,
            download_behavior,
            mut expected_download_contents,
        } = self;
        setup();
        let _permit = TEST_SEMAPHORE.acquire().await.unwrap();
        log::info!("starting browser test");
        let test_dir = format!("{}/tests", env!("CARGO_MANIFEST_DIR"));

        async fn download_testfile() -> Response {
            let content = "test file contents";
            (
                StatusCode::OK,
                [
                    (
                        header::CONTENT_DISPOSITION,
                        "attachment; filename=\"test-file\"",
                    ),
                    (header::CONTENT_TYPE, "application/octet-stream"),
                ],
                content,
            )
                .into_response()
        }

        async fn secret_handler(
            Path(path): Path<String>,
            headers: HeaderMap,
        ) -> Response {
            let authorized = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                == Some("Bearer bombadil");
            if !authorized {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            match path.as_str() {
                "app.js" => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/javascript")],
                    "var el = document.createElement('div'); \
                     el.id = 'secret-loaded'; \
                     document.body.appendChild(el);",
                )
                    .into_response(),
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }

        let app = Router::new()
            .route("/test-file", get(download_testfile))
            .route("/secret/{*path}", get(secret_handler))
            .fallback_service(ServeDir::new(&test_dir));
        let app_other = app.clone();

        let (listener, listener_other, port) = loop {
            let listener =
                tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let listener_other =
                if let Ok(listener_other) = tokio::net::TcpListener::bind(
                    format!("127.0.0.1:{}", addr.port() + 1),
                )
                .await
                {
                    listener_other
                } else {
                    continue;
                };
            break (listener, listener_other, addr.port());
        };

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::spawn(async move {
            axum::serve(listener_other, app_other).await.unwrap();
        });

        let origin =
            Url::parse(&format!("http://localhost:{}/{}", port, name,))
                .unwrap();
        let user_data_directory = TempDir::new().unwrap();

        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        let specification = match specification {
            Some(source) => {
                specification_file.write_all(source.as_bytes()).unwrap();
                Specification {
                    module_specifier: specification_file
                        .path()
                        .display()
                        .to_string(),
                }
            }
            None => Specification {
                module_specifier: "@antithesishq/bombadil/browser/defaults"
                    .to_string(),
            },
        };

        let downloads_directory = TempDir::new().unwrap();
        let download_monitor = expect_empty_downloads.then(|| {
            let path = downloads_directory.path().to_path_buf();
            ArtifactMonitor::start(move || download_directory_snapshot(&path))
        });
        let profile_monitor = expect_empty_downloads.then(|| {
            let path = user_data_directory.path().to_path_buf();
            ArtifactMonitor::start(move || {
                optimization_guide_profile_snapshot(&path)
            })
        });
        let browser_options = BrowserOptions {
            create_target: true,
            emulation: Emulation {
                width: 800,
                height: 600,
                device_scale_factor: 1.0,
            },
            instrumentation: Default::default(),
            download_behavior,
            downloads_directory: downloads_directory.path().to_path_buf(),
            grant_permissions,
            extra_headers,
            cookies,
        };
        let debugger_options = DebuggerOptions::Managed {
            launch_options: LaunchOptions {
                headless: true,
                no_sandbox: true,
                user_data_directory: user_data_directory.path().to_path_buf(),
            },
        };

        #[derive(Default)]
        struct ViolationsCollectingWriter {
            violations: Vec<bombadil::runner::PropertyViolation>,
        }

        impl TraceWriter for ViolationsCollectingWriter {
            fn write(
                &mut self,
                _state: &bombadil_browser::browser::state::BrowserState,
                _last_action: Option<&BrowserAction>,
                _snapshots: &[bombadil::specification::domain::Snapshot],
                violations: &[bombadil::runner::PropertyViolation],
            ) -> anyhow::Result<()> {
                self.violations.extend_from_slice(violations);
                Ok(())
            }
        }

        let output_path = TempDir::new().unwrap();
        let output_path_buf = output_path.path().to_path_buf();
        let writer = ViolationsCollectingWriter::default();

        enum Outcome {
            Success,
            Error(anyhow::Error),
        }

        impl Display for Outcome {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Outcome::Success => write!(f, "success"),
                    Outcome::Error(error) => {
                        write!(f, "error: {}", error)
                    }
                }
            }
        }

        log::info!("starting runner with infrastructure safety timeout");
        // The driver and runner are synchronous (the browser runs on its own
        // worker thread/runtime), so build and run them on a blocking thread.
        let run_handle = tokio::task::spawn_blocking(move || {
            let runner = runner::launch(
                origin.clone(),
                specification,
                browser_options,
                debugger_options,
            )
            .expect("run_test failed");

            let test_start = SystemTime::now();
            let deadline = time_limit.map(|duration| test_start + duration);

            let mut strategy = TestStrategy {
                rng: rand::prelude::StdRng::seed_from_u64(seed),
                test_start: Some(Time::from_system_time(test_start)),
                deadline,
                mode: bombadil_browser::strategy::TestMode::RandomWalk,
                writer,
                exit_on_violation: true,
                origin,
                output_path: output_path_buf,
                violations_count: 0,
            };

            match runner.run(&mut strategy) {
                Err(error) => Outcome::Error(error),
                Ok(_) if strategy.violations_count == 0 => Outcome::Success,
                Ok(_) => {
                    let violations: Vec<String> = strategy
                        .writer
                        .violations
                        .iter()
                        .map(|violation| {
                            let markup = markup::render_violation(
                                &violation.to_schema(),
                            );
                            let rendered = styled::markup_to_styled(
                                &markup,
                                Time::from_system_time(test_start),
                            );
                            format!("{}:\n{}\n\n", violation.name, rendered)
                        })
                        .collect();
                    Outcome::Error(anyhow!(
                        "violations:\n\n{}",
                        violations.join("")
                    ))
                }
            }
        });

        let outcome = match tokio::time::timeout(
            Duration::from_secs(TEST_TIMEOUT_SECONDS),
            run_handle,
        )
        .await
        {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(join_error)) => {
                Err(format!("runner task panicked: {join_error}"))
            }
            Err(_elapsed) => Err(format!(
                "test infrastructure timeout — test hung for {}s",
                TEST_TIMEOUT_SECONDS
            )),
        };
        let download_observations = download_monitor
            .map(ArtifactMonitor::finish)
            .unwrap_or_default();
        let profile_observations = profile_monitor
            .map(ArtifactMonitor::finish)
            .unwrap_or_default();

        log::info!("checking outcome");
        let outcome_failure = match (outcome, expect) {
            (Err(failure), _) => Some(failure),
            (
                Ok(Outcome::Error(error)),
                Expect::Error {
                    substring,
                    forbidden_substrings,
                },
            ) => {
                let message = error.to_string();
                if !message.contains(substring) {
                    Some(format!(
                        "expected error message {:?} not found in:\n\n{}\n\ntry reproducing by adding .seed({})",
                        substring, error, seed
                    ))
                } else if let Some(forbidden) = forbidden_substrings
                    .iter()
                    .find(|forbidden| message.contains(**forbidden))
                {
                    Some(format!(
                        "error message exposed forbidden download detail {:?}: {}",
                        forbidden, message
                    ))
                } else {
                    None
                }
            }
            (Ok(Outcome::Success), Expect::Success) => None,
            (Ok(outcome), expect) => Some(format!(
                "{} but got {}\n\ntry reproducing by adding .seed({})",
                expect, outcome, seed
            )),
        };

        let mut failures = outcome_failure.into_iter().collect::<Vec<_>>();
        if !download_observations.is_empty() {
            failures.push(format!(
                "managed Chrome created transient or retained download entries: {download_observations:#?}"
            ));
        }
        if !profile_observations.is_empty() {
            failures.push(format!(
                "managed Chrome created transient or retained Optimization Guide profile entries: {profile_observations:#?}"
            ));
        }
        if let Some(ref mut expected) = expected_download_contents {
            expected.sort();
            match download_directory_contents(downloads_directory.path()) {
                Ok(actual) if actual != *expected => failures.push(format!(
                    "download contents differ: expected {expected:?}, got {actual:?}"
                )),
                Ok(_) => {}
                Err(error) => failures.push(format!(
                    "could not verify terminal download contents: {error}"
                )),
            }
        }
        if !failures.is_empty() {
            let failure = failures.join("\n\n");
            if expect_empty_downloads {
                panic_with_artifact_evidence(
                    downloads_directory,
                    user_data_directory,
                    &failure,
                    &download_observations,
                    &profile_observations,
                );
            }
            panic!("{failure}");
        }
    }
}

#[tokio::test]
async fn test_console_error() {
    BrowserIntegrationTest::new("console-error")
        .expect_error("oh no you pressed too much")
        .run()
        .await;
}

#[tokio::test]
async fn test_links() {
    BrowserIntegrationTest::new("links")
        .expect_error("404")
        .run()
        .await;
}

#[tokio::test]
async fn test_uncaught_exception() {
    BrowserIntegrationTest::new("uncaught-exception")
        .expect_error("oh no you pressed too much")
        .run()
        .await;
}

#[tokio::test]
async fn test_unhandled_promise_rejection() {
    BrowserIntegrationTest::new("unhandled-promise-rejection")
        .expect_error("oh no you pressed too much")
        .run()
        .await;
}

#[tokio::test]
async fn test_other_domain() {
    BrowserIntegrationTest::new("other-domain")
        .time_limit(Duration::from_secs(5))
        .run()
        .await;
}

#[tokio::test]
async fn test_action_within_iframe() {
    BrowserIntegrationTest::new("action-within-iframe")
        .time_limit(Duration::from_secs(5))
        .run()
        .await;
}

#[tokio::test]
async fn test_no_action_available() {
    BrowserIntegrationTest::new("no-action-available")
        .expect_error("no actions available")
        .run()
        .await;
}

#[tokio::test]
async fn test_back_from_non_html() {
    BrowserIntegrationTest::new("back-from-non-html")
        .time_limit(Duration::from_secs(30))
        .specification(
            r#"
import { now, next, eventually } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks, back } from "@antithesishq/bombadil/browser/defaults/actions";

const contentType = extract((state) => state.document.contentType);

export const navigatesBackFromNonHtml = eventually(
  now(() => contentType.current === "text/html")
    .and(next(
      now(() => contentType.current !== "text/html")
        .and(next(
          now(() => contentType.current === "text/html")
        ))
    ))
).within(20, "seconds");
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_browser_lifecycle() {
    setup();
    let test_dir = format!("{}/tests", env!("CARGO_MANIFEST_DIR"));
    let app = Router::new().fallback_service(ServeDir::new(&test_dir));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let origin =
        Url::parse(&format!("http://localhost:{}/console-error", port,))
            .unwrap();
    log::info!("running test server on {}", origin);
    let user_data_directory = TempDir::new().unwrap();

    let downloads_directory = TempDir::new().unwrap();
    let mut browser = Browser::new(
        origin,
        BrowserOptions {
            create_target: true,
            emulation: Emulation {
                width: 800,
                height: 600,
                device_scale_factor: 1.0,
            },
            instrumentation: Default::default(),
            download_behavior: DownloadBehavior::AllowAndName,
            downloads_directory: downloads_directory.path().to_path_buf(),
            grant_permissions: vec![],
            extra_headers: Default::default(),
            cookies: vec![],
        },
        DebuggerOptions::Managed {
            launch_options: LaunchOptions {
                headless: true,
                no_sandbox: true,
                user_data_directory: user_data_directory.path().to_path_buf(),
            },
        },
    )
    .await
    .unwrap();

    browser.initiate().await.unwrap();

    let state = match browser.next_event().await.unwrap() {
        bombadil_browser::browser::BrowserEvent::StateChanged(state) => {
            assert_eq!(state.title, "Console Error");
            state
        }
        bombadil_browser::browser::BrowserEvent::Error(error) => {
            panic!("unexpected browser error: {}", error)
        }
    };

    browser
        .apply(BrowserAction::Reload, Arc::new(state))
        .unwrap();

    match browser.next_event().await.unwrap() {
        bombadil_browser::browser::BrowserEvent::StateChanged(state) => {
            assert_eq!(state.title, "Console Error");
        }
        bombadil_browser::browser::BrowserEvent::Error(error) => {
            panic!("unexpected browser error: {}", error)
        }
    }

    log::info!("just changing for CI");
    browser.terminate().await.unwrap();
}

#[tokio::test]
async fn test_random_text_input() {
    BrowserIntegrationTest::new("random-text-input")
        .specification(
            r#"
import { now, eventually } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks, inputs } from "@antithesishq/bombadil/browser/defaults/actions";

const inputValue = extract((state) => {
  const input = state.document.querySelector("\#text-input");
  return input ? input.value : "";
});

export const inputEventuallyHasText = eventually(
  () => inputValue.current.length > 0
).within(10, "seconds");
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_textarea_backspace() {
    BrowserIntegrationTest::new("textarea-backspace")
        .specification(
            r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/browser";

export const backspaces = actions(() => [{ PressKey: { code: 8 } }]);

const editorValue = extract((state) => {
  const editor = state.document.querySelector("\#editor");
  return editor ? editor.value : "";
});

export const editorEventuallyEmpty = eventually(
  () => editorValue.current === ""
).within(10, "seconds");
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_counter_state_machine() {
    BrowserIntegrationTest::new("counter-state-machine")
        .time_limit(Duration::from_secs(3))
        .specification(
            r#"
import { now, next, always } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const counterValue = extract((state) => {
  const element = state.document.body.querySelector("\#counter");
  return parseInt(element?.textContent ?? "0", 10);
});

const unchanged = now(() => {
  const current = counterValue.current;
  return next(() => counterValue.current === current);
});

const increment = now(() => {
  const current = counterValue.current;
  return next(() => counterValue.current === current + 1);
});

const decrement = now(() => {
  const current = counterValue.current;
  return next(() => counterValue.current === current - 1);
});

export const counterStateMachine = always(unchanged.or(increment).or(decrement));
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_resource_leak_detected() {
    BrowserIntegrationTest::new("resource-leak")
        .time_limit(Duration::from_secs(8))
        .expect_error("noDomLeak")
        .specification(
            r#"
import { noResourceLeak } from "@antithesishq/bombadil/browser/extras/resources";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

export const noDomLeak = noResourceLeak({
  metric: "dom_nodes",
  growthLimit: 150,
  windowMillis: 1000,
});
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_no_resource_leak() {
    BrowserIntegrationTest::new("no-resource-leak")
        .time_limit(Duration::from_secs(8))
        .specification(
            r#"
import { noResourceLeak } from "@antithesishq/bombadil/browser/extras/resources";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

export const noDomLeak = noResourceLeak({
  metric: "dom_nodes",
  growthLimit: 150,
  windowMillis: 1000,
});
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_extractor_exception_stack_trace() {
    BrowserIntegrationTest::new("extractor-exception")
        .expect_error("\n    at throwingFunction")
        .specification(
            r##"
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

function throwingFunction() {
  throw new Error("extractor stack trace test");
}

const bad = extract((state) => throwingFunction());
"##,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_wait_action() {
    BrowserIntegrationTest::new("wait-action")
        .time_limit(Duration::from_secs(3))
        .specification(
            r#"
import { always } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/browser";

export const waits = actions(() => ["Wait"]);

const counterValue = extract((state) => {
  const element = state.document.body.querySelector("\#counter");
  return parseInt(element?.textContent ?? "0", 10);
});

export const counterNeverChanges = always(() => counterValue.current === 0);
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_managed_chrome_avoids_background_optimization_hints_download() {
    BrowserIntegrationTest::new("wait-action")
        .time_limit(Duration::from_secs(60))
        .specification(
            r#"
import { always } from "@antithesishq/bombadil";
import { actions } from "@antithesishq/bombadil/browser";

export const waits = actions(() => ["Wait"]);
export const staysRunning = always(() => true);
"#,
        )
        .expect_empty_downloads()
        .run()
        .await;
}

#[tokio::test]
async fn test_double_click() {
    BrowserIntegrationTest::new("double-click")
        .time_limit(Duration::from_secs(5))
        .specification(
            r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract, getFingerprint } from "@antithesishq/bombadil/browser";

const counterValue = extract((state) => {
  const element = state.document.body.querySelector("\#counter");
  return parseInt(element?.textContent ?? "0", 10);
});

const fingerprint = extract((state) => {
  return getFingerprint(state.document.getElementById( "double-click-target"));
});

export const doubleClicks = actions(() => [
  {
    DoubleClick: {
      fingerprint: fingerprint.current,
      point: { x: 400, y: 300 },
      delayMillis: 100,
    },
  },
]);

export const counterIncreases = eventually(() => counterValue.current > 0);
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_extractor_guard() {
    BrowserIntegrationTest::new("extractor-guard")
        .expect_error("Cannot access cell.current from within an extractor")
        .specification(
            r##"
import { actions, extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

// First extractor
const foo = extract((state) => state.document.title);

// Second extractor tries to access the first - this should fail
const bar = extract((state) => foo.current);
"##,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_module_script() {
    BrowserIntegrationTest::new("module-script")
        .time_limit(Duration::from_secs(5))
        .specification(
            r##"
import { now } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const outputText = extract((state) => {
  const output = state.document.querySelector("#output");
  return output ? output.textContent : "";
});

export const moduleLoaded = now(() => {
  return outputText.current === "ES module loaded successfully";
});
"##,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_snapshot_references_in_violation() {
    BrowserIntegrationTest::new("snapshot-references")
        .expect_error("pageValue =")
        .specification(
            r#"
import { always } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const pageValue = extract((state) => {
  return parseInt(
    state.document.querySelector("\#value")?.textContent ?? "0", 10
  );
});

export const valueShouldStayZero = always(
  () => pageValue.current === 0
);
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_module_script_external() {
    BrowserIntegrationTest::new("module-script-external")
        .time_limit(Duration::from_secs(5))
        .specification(
            r##"
import { now } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const outputText = extract((state) => {
  const output = state.document.querySelector("#output");
  return output ? output.textContent : "";
});

export const moduleLoaded = now(() => {
  return outputText.current === "External ES module loaded successfully";
});
"##,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_time_limit() {
    BrowserIntegrationTest::new("time-limit")
        .time_limit(Duration::from_secs(5))
        .specification(
            r#"
import { always } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";
export const neverDone = always(() => true);
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_file_download() {
    BrowserIntegrationTest::new("file-download")
        .time_limit(Duration::from_secs(10))
        .expect_download_contents(b"test file contents")
        .specification(
            r#"
import { eventually } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const messageText = extract((state) => {
  const message = state.document.querySelector("\#message");
  return message ? message.textContent : "";
});

export const downloadCompletes = eventually(
  () => messageText.current === "you have downloaded the file"
);
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_file_download_deny() {
    BrowserIntegrationTest::new("file-download-deny")
        .time_limit(Duration::from_secs(10))
        .download_behavior(DownloadBehavior::Deny)
        .expect_error_without(
            "download request denied by configured browser policy",
            &["/test-file", "http://localhost:"],
        )
        .expect_empty_downloads()
        .expect_no_downloads()
        .run()
        .await;
}

#[tokio::test]
async fn test_file_picker() {
    let test_file = NamedTempFile::new().unwrap();
    std::fs::write(test_file.path(), b"test file content").unwrap();
    let file_path = test_file.path().display();

    let specification = format!(
        r#"
import {{ eventually }} from "@antithesishq/bombadil";
import {{ actions, extract, weighted }} from "@antithesishq/bombadil/browser";
export {{ clicks }} from "@antithesishq/bombadil/browser/defaults/actions";

const statusText = extract((state) => {{
  const status = state.document.querySelector("\#status");
  return status ? status.textContent : "";
}});

const fileIsSet = extract((state) => {{
  const input = state.document.querySelector("\#file-input");
  return input && input.files && input.files.length > 0;
}});

export const fileActions = actions(() => {{
  if (fileIsSet.current) return [];
  return [
    {{
      SetFileInputFiles: {{
        selector: "\#file-input",
        files: ["{file_path}"],
      }},
    }},
  ];
}});

export const fileUploaded = eventually(
  () => statusText.current === "you have uploaded a file"
).within(20, "seconds");
"#,
    );

    BrowserIntegrationTest::new("file-picker")
        .time_limit(Duration::from_secs(30))
        .specification(&specification)
        .run()
        .await;
}

#[tokio::test]
async fn test_granted_permissions() {
    BrowserIntegrationTest::new("granted-permissions")
        .time_limit(Duration::from_secs(5))
        .specification(
            r##"
import { now } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const notificationPermission = extract((state) => {
  const element = state.document.querySelector("#notification-permission");
  return element ? element.textContent : "";
});

const geolocationPermission = extract((state) => {
  const element = state.document.querySelector("#geolocation-permission");
  return element ? element.textContent : "";
});

export const notificationsGranted = now(() => {
  return notificationPermission.current === "notifications: granted";
});

export const geolocationGranted = now(() => {
  return geolocationPermission.current === "geolocation: granted";
});
"##,
        )
        .grant_permissions(vec![
            "notifications".to_string(),
            "geolocation".to_string(),
        ])
        .run()
        .await;
}

#[tokio::test]
async fn test_extra_headers() {
    BrowserIntegrationTest::new("fetch-headers")
        .extra_headers(HashMap::from([(
            "Authorization".to_string(),
            "Bearer bombadil".to_string(),
        )]))
        .time_limit(Duration::from_secs(15))
        .specification(
            r#"
import { eventually } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const loaded = extract((state) => {
  return state.document.querySelector('#secret-loaded') !== null;
});

export const secretResourceLoaded = eventually(
  () => loaded.current === true
).within(10, "seconds");
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_cookies() {
    BrowserIntegrationTest::new("fetch-headers")
        .cookies(vec![BrowserCookie::parse("session=bombadil").unwrap()])
        .time_limit(Duration::from_secs(15))
        .specification(
            r#"
import { eventually } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const cookieSet = extract((state) => {
  return state.document.cookie.includes("session=bombadil");
});

export const sessionCookiePresent = eventually(
  () => cookieSet.current === true
).within(10, "seconds");
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_cookie_domain() {
    BrowserIntegrationTest::new("cookie-domain")
        .cookies(vec![
            BrowserCookie::parse("session=bombadil; Domain=localhost").unwrap(),
        ])
        .time_limit(Duration::from_secs(15))
        .specification(
            r##"
import { eventually } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const cookieOk = extract((state) => {
  const el = state.document.querySelector("#cookie-ok");
  return el != null && (el as HTMLElement).offsetParent !== null;
});

export const sessionCookieOnOtherPort = eventually(
  () => cookieOk.current === true
).within(10, "seconds");
"##,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_confirm_dialog() {
    BrowserIntegrationTest::new("confirm-dialog")
        .time_limit(Duration::from_secs(5))
        .specification(
            r#"
import { now } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const message = extract((state) => {
  const element = state.document.querySelector("\#message");
  return element ? element.textContent : "";
});

export const dialogWasAccepted = now(
  () => message.current === "dialog accepted"
);
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_disabled_clicks() {
    BrowserIntegrationTest::new("disabled-clicks")
        .expect_error("no actions available")
        .specification(
            r#"
import { always } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

export const keepRunning = always(() => true);
"#,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_mouse_drag() {
    BrowserIntegrationTest::new("mouse-drag")
        .time_limit(Duration::from_secs(5))
        .specification(
            r##"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/browser";

const status = extract((state) => {
  const element = state.document.body.querySelector("#status");
  return element?.textContent ?? "";
});

export const drag = actions(() => [
  {
    MouseDrag: {
      from: { x: 100, y: 200 },
      to: { x: 400, y: 200 },
      steps: 5,
      delayMillis: 10,
    },
  },
]);

export const wasDragged = eventually(() => status.current === "dragged");
"##,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_set_viewport() {
    BrowserIntegrationTest::new("set-viewport")
        .time_limit(Duration::from_secs(5))
        .specification(
            r##"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/browser";

const size = extract((state) => {
  const element = state.document.body.querySelector("#size");
  return element?.textContent ?? "";
});

export const resize = actions(() => [
  { SetViewport: { width: 1024, height: 768 } },
]);

export const viewportApplied = eventually(() => size.current === "1024x768");
"##,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_custom_element_slot() {
    BrowserIntegrationTest::new("custom-element-slot")
        .time_limit(Duration::from_secs(5))
        .specification(
            r##"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/browser";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";

const isDone = extract((state) => {
  const element = state.document.getElementById("result");
  return element?.textContent === "Done";
});

export const eventuallyDone = eventually(() => isDone.current);
"##,
        )
        .run()
        .await;
}

#[tokio::test]
async fn test_custom_action() {
    BrowserIntegrationTest::new("custom-action")
        .time_limit(Duration::from_secs(5))
        .specification(
            r##"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract, registerCustomAction } from "@antithesishq/bombadil/browser";

const counter = extract((state) => {
  const element = state.document.getElementById("counter");
  return parseInt(element?.textContent ?? "0", 10);
});

const result = extract((state) => {
  const element = state.document.getElementById("result");
  return element?.textContent ?? "";
});

const doubleCounter = registerCustomAction("doubleCounter", async () => {
  const resultElement = document.getElementById("result");
  if (resultElement) {
    resultElement.textContent = (counter.current * 2).toString();
  }
});

export const _actions = actions(() => {
  if (result.current === "") {
    return [doubleCounter()];
  }
  return ["Wait"];
});

export const counterDoubled = eventually(() =>
  result.current === "10"
).within(5, "seconds");
"##,
        )
        .run()
        .await;
}
