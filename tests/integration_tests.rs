use anyhow::anyhow;
use axum::Router;
use std::{fmt::Display, path::PathBuf, sync::Once, time::Duration};
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tower_http::services::ServeDir;
use url::Url;

use bombadil::{
    browser::{
        Browser, BrowserOptions, DebuggerOptions, Emulation, LaunchOptions,
        actions::BrowserAction,
    },
    runner::{RunEvent, Runner, RunnerOptions},
    specification::{render::render_violation, verifier::Specification},
};

enum Expect {
    Error { substring: &'static str },
    Success,
}

impl Display for Expect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expect::Error { substring } => {
                write!(f, "expecting an error with substring {:?}", substring)
            }
            Expect::Success => write!(f, "expecting success"),
        }
    }
}

static INIT: Once = Once::new();

fn setup() {
    INIT.call_once(|| {
        let env = env_logger::Env::default().default_filter_or("warn");
        env_logger::Builder::from_env(env)
            .format_timestamp_millis()
            .format_target(true)
            .is_test(true)
            // Until we hav a fix for https://github.com/mattsse/chromiumoxide/issues/287
            .filter_module("chromiumoxide::browser", log::LevelFilter::Error)
            .init();
    });
}

/// These tests are pretty heavy, and running too many parallel risks one browser get stuck and
/// causing a timeout, so we limit parallelism.
static TEST_SEMAPHORE: Semaphore = Semaphore::const_new(2);
const TEST_TIMEOUT_SECONDS: u64 = 10;

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
async fn run_browser_test(name: &str, expect: Expect, timeout: Duration) {
    setup();
    let _permit = TEST_SEMAPHORE.acquire().await.unwrap();
    let app = Router::new().fallback_service(ServeDir::new("./tests"));
    let app_other = app.clone();

    let (listener, listener_other, port) = loop {
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let listener_other = if let Ok(listener_other) =
            tokio::net::TcpListener::bind(format!(
                "127.0.0.1:{}",
                addr.port() + 1
            ))
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
        Url::parse(&format!("http://localhost:{}/{}", port, name,)).unwrap();
    let user_data_directory = TempDir::new().unwrap();

    let default_specification = Specification::from_string(
        r#"export * from "@antithesishq/bombadil/defaults";"#,
        PathBuf::from("fake.ts").as_path(),
    )
    .unwrap();

    let runner = Runner::new(
        origin,
        default_specification,
        RunnerOptions {
            stop_on_violation: true,
        },
        BrowserOptions {
            create_target: true,
            emulation: Emulation {
                width: 800,
                height: 600,
                device_scale_factor: 2.0,
            },
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
    .expect("run_test failed");

    let mut events = runner.start();

    let result = async {
        loop {
            match events.next().await {
                Ok(Some(RunEvent::NewState { violations, .. })) => {
                    if !violations.is_empty() {
                        break Err(anyhow!(
                            "violations:\n\n{}",
                            violations
                                .iter()
                                .map(|violation| format!(
                                    "{}:\n{}\n\n",
                                    violation.name,
                                    render_violation(&violation.violation)
                                ))
                                .collect::<String>()
                        ));
                    }
                }
                Ok(None) => break events.shutdown().await,
                Err(err) => {
                    log::error!("next event error: {}", err);
                    break events.shutdown().await;
                }
            }
        }
    };

    enum Outcome {
        Success,
        Error(anyhow::Error),
        Timeout,
    }

    impl Display for Outcome {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Outcome::Success => write!(f, "success"),
                Outcome::Error(error) => {
                    write!(f, "error: {}", error)
                }
                Outcome::Timeout => write!(f, "timeout"),
            }
        }
    }

    let outcome = match tokio::time::timeout(timeout, result).await {
        Ok(Ok(())) => Outcome::Success,
        Ok(Err(error)) => Outcome::Error(error),
        Err(_elapsed) => Outcome::Timeout,
    };

    match (outcome, expect) {
        (Outcome::Error(error), Expect::Error { substring }) => {
            if !error.to_string().contains(substring) {
                panic!("expected error message not found in: {}", error);
            }
        }
        (Outcome::Success, Expect::Success) => {}
        (Outcome::Timeout, Expect::Success) => {}
        (outcome, expect) => {
            panic!("{} but got {}", expect, outcome);
        }
    }
}

#[tokio::test]
async fn test_console_error() {
    run_browser_test(
        "console-error",
        Expect::Error {
            // TODO: restore assertion to "oh no you pressed too much" when we print relevant
            // cells again
            substring: "no_console_errors",
        },
        Duration::from_secs(10),
    )
    .await;
}

#[tokio::test]
async fn test_links() {
    run_browser_test(
        "links",
        Expect::Error {
            substring: "no_http_error_codes",
        },
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
    )
    .await;
}

#[tokio::test]
async fn test_uncaught_exception() {
    run_browser_test(
        "uncaught-exception",
        Expect::Error {
            // TODO: restore assertion to "oh no you pressed too much" when we print relevant
            // cells again
            substring: "no_uncaught_exceptions",
        },
        Duration::from_secs(10),
    )
    .await;
}

#[tokio::test]
async fn test_unhandled_promise_rejection() {
    run_browser_test(
        "unhandled-promise-rejection",
        Expect::Error {
            // TODO: restore assertion to "oh no you pressed too much" when we print relevant
            // cells again
            substring: "no_unhandled_promise_rejections",
        },
        Duration::from_secs(10),
    )
    .await;
}

#[tokio::test]
async fn test_other_domain() {
    run_browser_test(
        "other-domain",
        Expect::Success,
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
    )
    .await;
}

#[tokio::test]
async fn test_action_within_iframe() {
    run_browser_test(
        "action-within-iframe",
        Expect::Success,
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
    )
    .await;
}

#[tokio::test]
async fn test_no_action_available() {
    run_browser_test(
        "no-action-available",
        Expect::Error {
            substring: "no fallback action available",
        },
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
    )
    .await;
}

#[tokio::test]
async fn test_browser_lifecycle() {
    setup();
    let app = Router::new().fallback_service(ServeDir::new("./tests"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let origin =
        Url::parse(&format!("http://localhost:{}/console-error", port,))
            .unwrap();
    log::info!("running test server on {}", &origin);
    let user_data_directory = TempDir::new().unwrap();

    let mut browser = Browser::new(
        origin,
        BrowserOptions {
            create_target: true,
            emulation: Emulation {
                width: 800,
                height: 600,
                device_scale_factor: 2.0,
            },
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

    match browser.next_event().await.unwrap() {
        bombadil::browser::BrowserEvent::StateChanged(state) => {
            assert_eq!(state.title, "Console Error");
        }
        bombadil::browser::BrowserEvent::Error(error) => {
            panic!("unexpected browser error: {}", error)
        }
    }

    browser.apply(BrowserAction::Reload).await.unwrap();

    match browser.next_event().await.unwrap() {
        bombadil::browser::BrowserEvent::StateChanged(state) => {
            assert_eq!(state.title, "Console Error");
        }
        bombadil::browser::BrowserEvent::Error(error) => {
            panic!("unexpected browser error: {}", error)
        }
    }

    log::info!("just changing for CI");
    browser.terminate().await.unwrap();
}
