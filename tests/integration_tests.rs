use std::time::Duration;

use axum::Router;
use tempfile::TempDir;
use tokio::time::error::Elapsed;
use tower_http::services::ServeDir;
use url::Url;

use antithesis_browser::{browser::BrowserOptions, runner::run_test};

enum Expect {
    Error { substring: &'static str },
    Success,
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

    let result = tokio::time::timeout(
        timeout,
        run_test(
            origin,
            &BrowserOptions {
                headless: true,
                no_sandbox: false,
                user_data_directory: user_data_directory.path().to_path_buf(),
                width: 800,
                height: 600,
                proxy: None,
            },
        ),
    )
    .await;
    match (result, expect) {
        (Ok(Err(err)), Expect::Error { substring }) => {
            if !err.to_string().contains(substring) {
                panic!("expected error message not found in: {}", err);
            }
        }
        (Ok(Err(err)), Expect::Success) => {
            panic!("unexpected error: {}", err);
        }
        (Ok(Ok(_)), Expect::Success) => {}
        (Ok(Ok(_)), Expect::Error { .. }) => {
            panic!("expected error but got success",);
        }
        (Err(Elapsed { .. }), Expect::Success) => {}
        (Err(Elapsed { .. }), Expect::Error { .. }) => {
            panic!("expected error but got timeout")
        }
    }
}

#[tokio::test]
async fn test_console_error() {
    run_browser_test(
        "console-error",
        Expect::Error {
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
        Expect::Error {
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
        Expect::Error {
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
        Expect::Error {
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
