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

use anyhow::{Result, anyhow, bail};
use cdp::{Connection, Method};
use cdp_protocol::cdp::{
    browser_protocol::{browser, page, target},
    js_protocol::runtime,
};

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Attach,
    Create,
}

impl TryFrom<&str> for Mode {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "attach" => Ok(Self::Attach),
            "create" => Ok(Self::Create),
            _ => bail!("invalid mode: {value}"),
        }
    }
}

fn main() -> Result<()> {
    let env = env_logger::Env::default().default_filter_or("info");
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .format_target(true)
        .init();

    let args = env::args().skip(1).take(2).collect::<Vec<String>>();
    let [mode, url] = args.as_slice() else {
        bail!("usage: basic <attach|create> <ws-devtools-url>");
    };
    let mode: Mode = mode.as_str().try_into()?;
    let connection = Connection::connect(url.as_str())?;

    let version =
        connection.send(browser::GetVersionParams::default(), None)?;
    println!("Version:  {:?}", version);

    let target_id = if mode == Mode::Attach {
        let targets = connection
            .send(
                target::GetTargetsParams {
                    filter: Some(target::TargetFilter::new(vec![
                        target::FilterEntry {
                            r#type: Some("page".into()),
                            exclude: None,
                        },
                    ])),
                },
                None,
            )?
            .target_infos;
        let Some(target) = targets.first() else {
            bail!("no target to attach to");
        };
        connection.send(
            target::AttachToTargetParams {
                target_id: target.target_id.clone(),
                flatten: Some(true),
            },
            None,
        )?;
        target.target_id.clone()
    } else {
        connection
            .send(target::CreateTargetParams::default(), None)?
            .target_id
    };

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
        connection.send(runtime::EnableParams::default(), Some(&session_id))?;
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

    let execution_context = connection
        .events
        .subscribe::<runtime::EventExecutionContextCreated>()
        .next()?
        .ok_or(anyhow!("no execution context"))?;
    log::info!("Got execution context {:?}", execution_context.context);

    let _ = connection
        .events
        .subscribe::<page::EventLoadEventFired>()
        .next()?
        .ok_or(anyhow!("no execution context"))?;
    log::info!("Got page load...");

    if mode == Mode::Create {
        let _ = connection.send(
            target::CloseTargetParams {
                target_id: target_id.clone(),
            },
            None,
        )?;
    }

    connection.close()?;

    println!("Residual events:");
    for event in connection.events.all() {
        println!("${}: {}", event.method_name(), event.params);
    }
    println!("Done.");

    Ok(())
}
