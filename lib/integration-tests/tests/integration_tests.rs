use anyhow::anyhow;
use axum::Router;
use std::io::Write;
use std::{fmt::Display, sync::Once, time::Duration};
use tempfile::{NamedTempFile, TempDir};
use tokio::sync::Semaphore;
use tower_http::services::ServeDir;
use url::Url;

use bombadil::{
    browser::{
        Browser, BrowserOptions, DebuggerOptions, Emulation, LaunchOptions,
        actions::BrowserAction,
    },
    runner::{Runner, RunnerOptions},
    specification::verifier::Specification,
};
use bombadil_schema::{markup, text};

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

/// These tests are pretty heavy, and running too many parallel risks one browser get stuck and
/// causing a timeout, so we limit parallelism.
static TEST_SEMAPHORE: Semaphore = Semaphore::const_new(4);
const TEST_TIMEOUT_SECONDS: u64 = 120;

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
async fn run_browser_test(
    name: &str,
    expect: Expect,
    timeout: Duration,
    specification: Option<&str>,
) {
    setup();
    let _permit = TEST_SEMAPHORE.acquire().await.unwrap();
    log::info!("starting browser test");
    let test_dir = format!("{}/tests", env!("CARGO_MANIFEST_DIR"));
    let app = Router::new().fallback_service(ServeDir::new(&test_dir));
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

    let runner = Runner::new(
        origin,
        specification,
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
            instrumentation: Default::default(),
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

    log::info!("starting runner");

    struct TestObserver {
        collected_violations: Vec<String>,
        test_start: Option<std::time::SystemTime>,
    }

    impl bombadil::runner::RunObserver for TestObserver {
        type StopValue = ();

        async fn on_new_state(
            &mut self,
            state: &bombadil::browser::state::BrowserState,
            _last_action: Option<&bombadil::browser::actions::BrowserAction>,
            _snapshots: &[bombadil::specification::verifier::Snapshot],
            violations: &[bombadil::trace::PropertyViolation],
        ) -> anyhow::Result<bombadil::runner::ControlFlow<Self::StopValue>>
        {
            let test_start = *self.test_start.get_or_insert(state.timestamp);
            if !violations.is_empty() {
                for violation in violations {
                    let api_violation = violation.to_api();
                    let markup = markup::render_violation(&api_violation);
                    let rendered = text::markup_to_text(&markup, test_start);
                    self.collected_violations
                        .push(format!("{}:\n{}\n\n", violation.name, rendered));
                }
            }
            Ok(bombadil::runner::ControlFlow::Continue)
        }
    }

    let mut observer = TestObserver {
        collected_violations: Vec::new(),
        test_start: None,
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

    log::info!("starting timeout");
    let outcome =
        match tokio::time::timeout(timeout, runner.run(&mut observer)).await {
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
            Err(_elapsed) => Outcome::Timeout,
        };

    log::info!("checking outcome");
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
            substring: "noConsoleErrors",
        },
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_links() {
    run_browser_test(
        "links",
        Expect::Error {
            substring: "noHttpErrorCodes",
        },
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
        None,
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
            substring: "noUncaughtExceptions",
        },
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
        None,
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
            substring: "noUnhandledPromiseRejections",
        },
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_other_domain() {
    run_browser_test(
        "other-domain",
        Expect::Success,
        Duration::from_secs(5),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_action_within_iframe() {
    run_browser_test(
        "action-within-iframe",
        Expect::Success,
        Duration::from_secs(5),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_no_action_available() {
    run_browser_test(
        "no-action-available",
        Expect::Error {
            substring: "no actions available",
        },
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_back_from_non_html() {
    run_browser_test(
        "back-from-non-html",
        Expect::Success,
        Duration::from_secs(30),
        Some(
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
        ),
    )
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
    run_browser_test(
        "random-text-input",
        Expect::Success,
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
        Some(
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
        ),
    )
    .await;
}

#[tokio::test]
async fn test_counter_state_machine() {
    run_browser_test(
        "counter-state-machine",
        Expect::Success,
        Duration::from_secs(3),
        Some(
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
        ),
    )
    .await;
}

#[tokio::test]
async fn test_time_extractor() {
    run_browser_test(
        "time-extractor",
        Expect::Success,
        Duration::from_secs(10),
        Some(
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
        ),
    )
    .await;
}

#[tokio::test]
async fn test_extractor_exception_stack_trace() {
    run_browser_test(
        "extractor-exception",
        Expect::Error {
            substring: "\n    at throwingFunction",
        },
        Duration::from_secs(5),
        Some(
            r##"
import { extract } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

function throwingFunction() {
  throw new Error("extractor stack trace test");
}

const bad = extract((state) => throwingFunction());
"##,
        ),
    )
    .await;
}

#[tokio::test]
async fn test_wait_action() {
    run_browser_test(
        "wait-action",
        Expect::Success,
        Duration::from_secs(3),
        Some(
            r#"
import { actions, extract, always } from "@antithesishq/bombadil";

export const waits = actions(() => ["Wait"]);

const counterValue = extract((state) => {
  const element = state.document.body.querySelector("\#counter");
  return parseInt(element?.textContent ?? "0", 10);
});

export const counterNeverChanges = always(() => counterValue.current === 0);
"#,
        ),
    )
    .await;
}

#[tokio::test]
async fn test_double_click() {
    run_browser_test(
        "double-click",
        Expect::Success,
        Duration::from_secs(5),
        Some(
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
        ),
    )
    .await;
}

#[tokio::test]
async fn test_extractor_guard() {
    run_browser_test(
        "extractor-guard",
        Expect::Error {
            substring: "Cannot access cell.current from within an extractor",
        },
        Duration::from_secs(5),
        Some(
            r##"
import { actions, extract } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/defaults/actions";

// First extractor
const foo = extract((state) => state.document.title);

// Second extractor tries to access the first - this should fail
const bar = extract((state) => foo.current);
"##,
        ),
    )
    .await;
}

#[tokio::test]
async fn test_module_script() {
    run_browser_test(
        "module-script",
        Expect::Success,
        Duration::from_secs(5),
        Some(
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
        ),
    )
    .await;
}

#[tokio::test]
async fn test_snapshot_references_in_violation() {
    run_browser_test(
        "snapshot-references",
        Expect::Error {
            substring: "pageValue = 1",
        },
        Duration::from_secs(TEST_TIMEOUT_SECONDS),
        Some(
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
        ),
    )
    .await;
}

#[tokio::test]
async fn test_module_script_external() {
    run_browser_test(
        "module-script-external",
        Expect::Success,
        Duration::from_secs(5),
        Some(
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
        ),
    )
    .await;
}
