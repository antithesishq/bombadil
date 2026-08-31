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

    let (mut conn, events) = Connection::connect(url.as_str())?;

    let version = conn.send(browser::GetVersionParams::default(), None)?;
    println!("Version:  {:?}", version);

    let target_id = conn
        .send(target::CreateTargetParams::default(), None)?
        .target_id;

    let session_id = conn
        .send(
            target::AttachToTargetParams {
                target_id: target_id.clone(),
                flatten: Some(true),
            },
            None,
        )?
        .session_id;

    let _ = conn.send(page::EnableParams::default(), Some(&session_id))?;

    let _ = conn.send(
        page::NavigateParams {
            url: "https://en.wikipedia.org".into(),
            referrer: None,
            transition_type: None,
            frame_id: None,
            referrer_policy: None,
        },
        Some(&session_id),
    )?;

    while let event = events.recv()?
        && event.method_name() != "loadEventFired"
    {
        log::info!("Awaiting page load...");
    }

    let _ = conn.send(
        target::CloseTargetParams {
            target_id: target_id.clone(),
        },
        None,
    )?;

    println!("Residual events:");
    while let Ok(event) = events.try_recv() {
        println!("${}: {}", event.method_name(), event.params);
    }
    println!("Done.");

    Ok(())
}
