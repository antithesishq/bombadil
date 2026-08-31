//! Example including request/response and events.
//!
//! Prerequisites: run Chromium yourself with the devtools port open, e.g.
//!
//! ```sh
//! chromium --new-window --user-data-dir=$(mktemp -d) --headless=new --remote-debugging-port=9222 --no-crashpad --disable-background-networking --disable-component-update --disable-domain-reliability --no-pings --disable-crash-reporter
//! ```
//!
//! Then in another terminal:
//!
//! ```sh
//! cargo run --example basic -- $(curl -s http://127.0.0.1:9222/json/version | jq -r .webSocketDebuggerUrl)
//! ```

use std::env;

use anyhow::{Result, bail};
use cdp::{Connection, Method};
use cdp_protocol::cdp::browser_protocol::{browser, page, target};

fn main() -> Result<()> {
    let env = env_logger::Env::default().default_filter_or("info");
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .format_target(true)
        .init();

    let Some(url) = env::args().nth(1) else {
        bail!("usage: basic <ws-devtools-url>");
    };

    let connection = Connection::connect(url.as_str())?;

    let version =
        connection.send(browser::GetVersionParams::default(), None)?;
    println!("Version:  {:?}", version);

    let target_id = connection
        .send(target::CreateTargetParams::default(), None)?
        .target_id;

    let session_id = connection
        .send(
            target::AttachToTargetParams {
                target_id: target_id.clone(),
                flatten: Some(true),
            },
            None,
        )?
        .session_id;

    let _ =
        connection.send(page::EnableParams::default(), Some(&session_id))?;

    let _ = connection.send(
        page::NavigateParams {
            url: "https://en.wikipedia.org".into(),
            referrer: None,
            transition_type: None,
            frame_id: None,
            referrer_policy: None,
        },
        Some(&session_id),
    )?;

    let _ = connection
        .events
        .subscribe::<page::EventLoadEventFired>()
        .next()?;
    log::info!("Got page load...");

    let _ = connection.send(
        target::CloseTargetParams {
            target_id: target_id.clone(),
        },
        None,
    )?;

    connection.close()?;

    println!("Residual events:");
    for event in connection.events.all() {
        println!("${}: {}", event.method_name(), event.params);
    }
    println!("Done.");

    Ok(())
}
