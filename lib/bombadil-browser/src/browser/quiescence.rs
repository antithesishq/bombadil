use anyhow::{Result, anyhow, bail};
use cdp_protocol::cdp::{
    browser_protocol::target::SessionId,
    js_protocol::runtime::{self},
};
use serde_json as json;
use std::{
    sync::Arc,
    thread,
    time::{Duration, SystemTime},
};

use crate::browser::{
    screenshots::{ScreencastFrame, screencast_frames},
    state::Exception,
};

pub struct QuiescedState {
    pub elapsed: Duration,
    pub frame: Option<Arc<ScreencastFrame>>,
}

/// Await the next quiesced browser state. This tries to be as fast as
/// possible and only await pending work in the JS event queue, and
/// the subsequent rendered frame.
pub fn next_state<F: FnOnce(Result<QuiescedState>) + Send + 'static>(
    connection: cdp::Connection,
    session_id: SessionId,
    on_quiesced: F,
) -> Result<()> {
    let start = SystemTime::now();
    let frames_rx = screencast_frames(&connection)?;

    thread::spawn(move || {
        let run = || -> Result<QuiescedState> {
            let evaluation = runtime::EvaluateParams::builder()
                .await_promise(true)
                .return_by_value(true)
                .timeout(runtime::TimeDelta::new(50))
                .expression(
                    r#"
    (async () => {
      const start = performance.now();
      return await new Promise((resolve) => requestAnimationFrame(() => resolve(performance.now() - start)));
    })()
    "#,
                )
                .build().map_err(|_| anyhow!( "failed building evaluate method call for"))?;

            let result = connection.send(evaluation, Some(&session_id))?;
            if log::log_enabled!(log::Level::Debug)
                && let Some(value) = result.result.value
                && let Ok(millis) = json::from_value::<f64>(value)
            {
                log::debug!(
                    "requestAnimationFrame completion took {millis:.2}ms"
                );
            }

            if let Some(exception_details) = result.exception_details {
                bail!(
                    "quiescence timer evaluation failed: {:?}",
                    Exception::from_exception_details(exception_details, None)
                );
            }

            let mut frame_latest = None;
            while let Ok(frame) = frames_rx.try_recv() {
                // Unfortunately there's no robust way to compare timestamps
                // of screencast frames from Chrome with our local clock. We
                // can't be sure these frames are before or after the action
                // we took in case the system clocks get skewed.
                if frame.timestamp.duration_since(start).is_ok() {
                    frame_latest = Some(frame);
                }
            }

            let frame_selected =
                match frames_rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(frame) => Some(frame),
                    Err(_) => {
                        log::debug!("timed out waiting for screencast frame");
                        frame_latest
                    }
                };

            let elapsed =
                SystemTime::now().duration_since(start).unwrap_or_default();
            Ok(QuiescedState {
                elapsed,
                frame: frame_selected,
            })
        };

        on_quiesced(run());
    });
    Ok(())
}
