use axum::Router;
use std::{fmt::Display, time::Duration};
use tempfile::TempDir;
use tower_http::services::ServeDir;
use url::Url;

use antithesis_browser::{
    browser::BrowserOptions,
    runner::{Runner, Violation},
};

enum Expect {
    Error { substring: &'static str },
    Violation { substring: &'static str },
    Success,
}

impl Display for Expect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expect::Error { substring } => {
                write!(f, "expecting an error with substring {:?}", substring)
            }
            Expect::Violation { substring } => write!(
                f,
                "expecting a violation with substring {:?}",
                substring
            ),
            Expect::Success => write!(f, "expecting success"),
        }
    }
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
async fn run_browser_test(name: &str, expect: Expect, timeout: Duration) {
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

    let runner = Runner::new(
        origin,
        &BrowserOptions {
            headless: true,
            no_sandbox: false,
            user_data_directory: user_data_directory.path().to_path_buf(),
            width: 800,
            height: 600,
            proxy: None,
        },
    )
    .await
    .expect("run_test failed");

    let mut events = runner.start();

    let violation = async move {
        loop {
            match events.next().await {
                Ok(Some(
                    antithesis_browser::runner::RunEvent::NewTraceEntry {
                        entry: _,
                        violation,
                    },
                )) => {
                    if violation.is_some() {
                        return Ok(violation);
                    }
                }
                Ok(None) => return Ok(None),
                Err(err) => anyhow::bail!(err),
            }
        }
    };

    enum Outcome {
        Success,
        Violation(Violation),
        Error(anyhow::Error),
        Timeout,
    }

    impl Display for Outcome {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Outcome::Success => write!(f, "success"),
                Outcome::Violation(violation) => {
                    write!(f, "violation: {}", violation)
                }
                Outcome::Error(error) => {
                    write!(f, "error: {}", error)
                }
                Outcome::Timeout => write!(f, "timeout"),
            }
        }
    }

    let outcome = match tokio::time::timeout(timeout, violation).await {
        Ok(Ok(None)) => Outcome::Success,
        Ok(Ok(Some(violation))) => Outcome::Violation(violation),
        Ok(Err(error)) => Outcome::Error(error),
        Err(_elapsed) => Outcome::Timeout,
    };

    match (outcome, expect) {
        (Outcome::Violation(violation), Expect::Violation { substring }) => {
            if !violation.to_string().contains(substring) {
                panic!(
                    "expected violation message not found in: {}",
                    violation
                );
            }
        }
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
        Expect::Violation {
            substring: "oh no you pressed all of them",
        },
        Duration::from_secs(10),
    )
    .await;
}

#[tokio::test]
async fn test_links() {
    run_browser_test(
        "links",
        Expect::Violation {
            substring: "got 404 at localhost",
        },
        Duration::from_secs(5),
    )
    .await;
}

#[tokio::test]
async fn test_uncaught_exception() {
    run_browser_test(
        "uncaught-exception",
        Expect::Violation {
            substring: "oh no you pressed all of them",
        },
        Duration::from_secs(10),
    )
    .await;
}

#[tokio::test]
async fn test_unhandled_promise_rejection() {
    run_browser_test(
        "unhandled-promise-rejection",
        Expect::Violation {
            substring: "oh no you pressed all of them",
        },
        Duration::from_secs(10),
    )
    .await;
}

#[tokio::test]
async fn test_other_domain() {
    run_browser_test("other-domain", Expect::Success, Duration::from_secs(3))
        .await;
}

#[tokio::test]
async fn test_no_action_available() {
    run_browser_test(
        "no-action-available",
        Expect::Error {
            substring: "no fallback action available",
        },
        Duration::from_secs(3),
    )
    .await;
}
