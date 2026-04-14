use anyhow::anyhow;
use axum::{
    Router,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use std::io::Write;
use std::{
    fmt::Display,
    sync::Once,
    time::{Duration, SystemTime},
};
use tempfile::{NamedTempFile, TempDir};
use tokio::sync::Semaphore;
use tower_http::services::ServeDir;
use url::Url;

use bombadil::{
    browser::{
        Browser, BrowserOptions, DebuggerOptions, Emulation, LaunchOptions,
        actions::BrowserAction,
    },
    runner::Runner,
    specification::verifier::Specification,
    styled,
};
use bombadil_schema::markup;

/// These tests are pretty heavy, and running too many parallel risks one browser get stuck and
/// causing a test to hang, so we limit parallelism.
static TEST_SEMAPHORE: Semaphore = Semaphore::const_new(4);
const TEST_TIMEOUT_SECONDS: u64 = 120;

static INIT: Once = Once::new();

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

struct BrowserIntegrationTest<'a> {
    name: &'a str,
    expect: Expect,
    time_limit: Option<Duration>,
    specification: Option<&'a str>,
    grant_permissions: Vec<String>,
}

impl<'a> BrowserIntegrationTest<'a> {
    fn new(name: &'a str) -> Self {
        Self {
            name,
            expect: Expect::Success,
            time_limit: None,
            specification: None,
            grant_permissions: vec![],
        }
    }

    fn expect_error(mut self, substring: &'static str) -> Self {
        self.expect = Expect::Error { substring };
        self
    }

    fn time_limit(mut self, duration: Duration) -> Self {
        self.time_limit = Some(duration);
        self
    }

    fn specification(mut self, spec: &'a str) -> Self {
        self.specification = Some(spec);
        self
    }

    fn grant_permissions(mut self, permissions: Vec<String>) -> Self {
        self.grant_permissions = permissions;
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
            name,
            expect,
            time_limit,
            specification,
            grant_permissions,
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

        let app = Router::new()
            .route("/test-file", get(download_testfile))
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
            Some(spec) => {
                specification_file.write_all(spec.as_bytes()).unwrap();
                Specification {
                    module_specifier: specification_file
                        .path()
                        .display()
                        .to_string(),
                }
            }
            None => Specification {
                module_specifier: "@antithesishq/bombadil/defaults".to_string(),
            },
        };

        let downloads_directory = TempDir::new().unwrap();
        let runner = Runner::new(
            origin,
            specification,
            BrowserOptions {
                create_target: true,
                emulation: Emulation {
                    width: 800,
                    height: 600,
                    device_scale_factor: 2.0,
                },
                instrumentation: Default::default(),
                downloads_directory: downloads_directory.path().to_path_buf(),
                grant_permissions,
            },
            DebuggerOptions::Managed {
                launch_options: LaunchOptions {
                    headless: true,
                    no_sandbox: true,
                    user_data_directory: user_data_directory
                        .path()
                        .to_path_buf(),
                },
            },
        )
        .await
        .expect("run_test failed");

        log::info!("starting runner");

        struct TestObserver {
            collected_violations: Vec<String>,
            test_start: Option<std::time::SystemTime>,
            deadline: Option<SystemTime>,
        }

        impl bombadil::runner::RunObserver for TestObserver {
            type StopValue = ();

            async fn on_new_state(
                &mut self,
                state: &bombadil::browser::state::BrowserState,
                _last_action: Option<
                    &bombadil::browser::actions::BrowserAction,
                >,
                _snapshots: &[bombadil::specification::verifier::Snapshot],
                violations: &[bombadil::trace::PropertyViolation],
            ) -> anyhow::Result<bombadil::runner::ControlFlow<Self::StopValue>>
            {
                let test_start =
                    *self.test_start.get_or_insert(state.timestamp);
                if !violations.is_empty() {
                    for violation in violations {
                        let api_violation = violation.to_api();
                        let markup = markup::render_violation(&api_violation);
                        let rendered = styled::markup_to_styled(
                            &markup,
                            bombadil_schema::Time::from_system_time(test_start),
                        );
                        self.collected_violations.push(format!(
                            "{}:\n{}\n\n",
                            violation.name, rendered
                        ));
                    }
                    return Ok(bombadil::runner::ControlFlow::Stop(()));
                }

                if let Some(deadline) = self.deadline
                    && state.timestamp >= deadline
                {
                    log::info!("time limit reached, stopping");
                    return Ok(bombadil::runner::ControlFlow::Stop(()));
                }

                Ok(bombadil::runner::ControlFlow::Continue)
            }
        }

        let deadline = time_limit.map(|d| SystemTime::now() + d);

        let mut observer = TestObserver {
            collected_violations: Vec::new(),
            test_start: None,
            deadline,
        };

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
        let outcome = match tokio::time::timeout(
            Duration::from_secs(TEST_TIMEOUT_SECONDS),
            runner.run(&mut observer),
        )
        .await
        {
            Ok(Ok(_)) => {
                if observer.collected_violations.is_empty() {
                    Outcome::Success
                } else {
                    Outcome::Error(anyhow!(
                        "violations:\n\n{}",
                        observer.collected_violations.join("")
                    ))
                }
            }
            Ok(Err(error)) => Outcome::Error(error),
            Err(_elapsed) => panic!(
                "test infrastructure timeout — test hung for {}s",
                TEST_TIMEOUT_SECONDS
            ),
        };

        log::info!("checking outcome");
        match (outcome, expect) {
            (Outcome::Error(error), Expect::Error { substring }) => {
                if !error.to_string().contains(substring) {
                    panic!(
                        "expected error message {:?} not found in:\n\n{}",
                        substring, error
                    );
                }
            }
            (Outcome::Success, Expect::Success) => {}
            (outcome, expect) => {
                panic!("{} but got {}", expect, outcome);
            }
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
import { extract, now, next, eventually } from "@antithesishq/bombadil";
export { clicks, back } from "@antithesishq/bombadil/defaults/actions";

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
    log::info!("running test server on {}", &origin);
    let user_data_directory = TempDir::new().unwrap();

    let downloads_directory = TempDir::new().unwrap();
    let mut browser = Browser::new(
        origin,
        BrowserOptions {
            create_target: true,
            emulation: Emulation {
                width: 800,
                height: 600,
                device_scale_factor: 2.0,
            },
            instrumentation: Default::default(),
            downloads_directory: downloads_directory.path().to_path_buf(),
            grant_permissions: vec![],
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

    browser
        .apply(BrowserAction::Reload, Duration::from_millis(500))
        .unwrap();

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

#[tokio::test]
async fn test_random_text_input() {
    BrowserIntegrationTest::new("random-text-input")
        .specification(
            r#"
import { extract, now, eventually } from "@antithesishq/bombadil";
export { clicks, inputs } from "@antithesishq/bombadil/defaults/actions";

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
async fn test_counter_state_machine() {
    BrowserIntegrationTest::new("counter-state-machine")
        .time_limit(Duration::from_secs(3))
        .specification(
            r#"
import { extract, now, next, always } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
async fn test_time_extractor() {
    BrowserIntegrationTest::new("time-extractor")
        .time_limit(Duration::from_secs(10))
        .specification(
            r##"
import { actions, extract, now, eventually, time } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

const myTime = extract((state) => time.current);

// Property: time is a reasonable value (after year 2020)
export const timeIsReasonable = now(() => {
  const start = myTime.current;
  return eventually(() =>
      myTime.current > start
  );
});
"##,
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
import { extract } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
import { actions, extract, always } from "@antithesishq/bombadil";

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
async fn test_double_click() {
    BrowserIntegrationTest::new("double-click")
        .time_limit(Duration::from_secs(5))
        .specification(
            r#"
import { actions, extract, eventually } from "@antithesishq/bombadil";

const counterValue = extract((state) => {
  const element = state.document.body.querySelector("\#counter");
  return parseInt(element?.textContent ?? "0", 10);
});

export const doubleClicks = actions(() => [
  {
    DoubleClick: {
      name: "double-click-target",
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
import { actions, extract } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
import { extract, now } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
import { extract, always } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
import { extract, now } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
export { clicks } from "@antithesishq/bombadil/defaults/actions";
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
        .specification(
            r#"
import { extract, eventually } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
async fn test_file_picker() {
    let test_file = NamedTempFile::new().unwrap();
    std::fs::write(test_file.path(), b"test file content").unwrap();
    let file_path = test_file.path().display();

    let spec = format!(
        r#"
import {{ actions, extract, eventually, weighted }} from "@antithesishq/bombadil";
export {{ clicks }} from "@antithesishq/bombadil/defaults/actions";

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
).within(5, "seconds");
"#,
    );

    BrowserIntegrationTest::new("file-picker")
        .time_limit(Duration::from_secs(10))
        .specification(&spec)
        .run()
        .await;
}

#[tokio::test]
async fn test_granted_permissions() {
    BrowserIntegrationTest::new("granted-permissions")
        .time_limit(Duration::from_secs(5))
        .specification(
            r##"
import { extract, now } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
