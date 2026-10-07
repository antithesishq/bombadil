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
    screenshots::{BeginFrameControl, ScreencastFrame, screencast_frames},
    state::Exception,
};

pub struct QuiescedState {
    pub elapsed: Duration,
    pub frame: Option<Arc<ScreencastFrame>>,
}

/// Await the next quiesced browser state. This tries to be as fast as
/// possible and only await pending work in the JS event queue, and
/// the subsequent rendered frame.
///
/// With begin frame control, instead of waiting for a
/// requestAnimationFrame and the screencast, we drive frames until
/// rendering settles and capture the last one directly.
pub fn next_state<F: FnOnce(Result<QuiescedState>) + Send + 'static>(
    connection: cdp::Connection,
    session_id: SessionId,
    begin_frame_control: Option<BeginFrameControl>,
    on_quiesced: F,
) -> Result<()> {
    if let Some(begin_frame_control) = begin_frame_control {
        return next_state_begin_frame(begin_frame_control, on_quiesced);
    }

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

/// Upper bound on frames driven while settling, as pages with continuous
/// animations never stop producing damage.
const SETTLE_FRAMES_MAX: usize = 3;

/// Drive frames until one has no damage, i.e. rendering has settled. A
/// screenshot frame always has damage, and leaves damage behind for the
/// next frame too, so these frames are not captured.
fn settle(begin_frame_control: &BeginFrameControl) -> Result<()> {
    for frames in 1..=SETTLE_FRAMES_MAX {
        if !begin_frame_control.frame()? {
            log::debug!("settled after {frames} frame(s)");
            return Ok(());
        }
    }
    log::debug!(
        "not settled after {SETTLE_FRAMES_MAX} frames, capturing anyway"
    );
    Ok(())
}

fn next_state_begin_frame<F: FnOnce(Result<QuiescedState>) + Send + 'static>(
    begin_frame_control: BeginFrameControl,
    on_quiesced: F,
) -> Result<()> {
    let start = SystemTime::now();
    thread::spawn(move || {
        let run = || -> Result<QuiescedState> {
            // A failed frame shouldn't stall the state machine, so we
            // still report quiescence and let capture fall back to the
            // latest frame. The pump is paused so that its frames don't
            // compete with settling and capturing.
            let data = begin_frame_control.paused(|| {
                if let Err(error) = settle(&begin_frame_control) {
                    log::error!("begin frame failed: {error:#}");
                }
                match begin_frame_control.capture() {
                    Ok(data) => data,
                    Err(error) => {
                        log::error!("begin frame failed: {error:#}");
                        None
                    }
                }
            });
            let timestamp = SystemTime::now();
            if data.is_none() {
                log::debug!("begin frame produced no screenshot");
            }
            Ok(QuiescedState {
                elapsed: timestamp.duration_since(start).unwrap_or_default(),
                frame: data
                    .map(|data| Arc::new(ScreencastFrame { timestamp, data })),
            })
        };
        on_quiesced(run());
    });
    Ok(())
}
