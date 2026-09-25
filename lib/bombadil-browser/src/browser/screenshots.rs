use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use std::{sync::Arc, time::SystemTime};

use anyhow::{Result, anyhow};
use base64::Engine;
use cdp::Binary;
use cdp_protocol::cdp::browser_protocol::page;
use cdp_protocol::cdp::browser_protocol::target::SessionId;
use crossbeam_channel as mpmc;

use crate::browser::state::{Screenshot, ScreenshotFormat};

pub const SCREENSHOT_QUALITY: u8 = 50;
pub const SCREENSHOT_FORMAT: ScreenshotFormat = ScreenshotFormat::Jpeg;

pub fn screenshot_capture(
    connection: &cdp::Connection,
    session_id: &SessionId,
    width: u16,
    height: u16,
) -> Result<Screenshot> {
    let result = connection.send(
        page::CaptureScreenshotParams {
            format: Some(SCREENSHOT_FORMAT.into()),
            quality: Some(SCREENSHOT_QUALITY.into()),
            clip: Some(page::Viewport {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
                scale: 1.0,
            }),
            from_surface: None,
            capture_beyond_viewport: Some(false),
            optimize_for_speed: Some(true),
        },
        Some(session_id),
    )?;

    let data = base64::prelude::BASE64_STANDARD
        .decode(result.data)
        .map_err(|e| anyhow!("screenshot base64 decode failed: {e}"))?;
    Ok(Screenshot {
        format: SCREENSHOT_FORMAT,
        data,
    })
}

pub fn screencast_start(
    connection: &cdp::Connection,
    session_id: &SessionId,
    width: u16,
    height: u16,
) -> Result<()> {
    let frames = connection.events.subscribe::<page::EventScreencastFrame>();

    connection.send(
        page::StartScreencastParams::builder()
            .format(SCREENSHOT_FORMAT)
            .quality(SCREENSHOT_QUALITY)
            .max_width(width)
            .max_height(height)
            .every_nth_frame(1)
            .send_last_frame(true)
            .build(),
        Some(session_id),
    )?;

    let connection = connection.clone();
    let session_id = session_id.clone();
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<()> {
                log::debug!("screencast: listener started");
                while let Some(event) = frames.next()? {
                    log::debug!(
                        "screencast: frame received (session_id={}), acking",
                        event.session_id
                    );
                    connection.post(
                        page::ScreencastFrameAckParams::new(event.session_id),
                        Some(&session_id),
                    )?;
                }
                log::debug!("screencast: listener ended");
                Ok(())
            },
        ));
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => error,
            Err(_) => anyhow!("screencast worker panicked"),
        };
        log::error!("screencast worker failed: {error:#}");
    });

    Ok(())
}

#[derive(Clone, Debug)]
pub struct ScreencastFrame {
    pub timestamp: SystemTime,
    pub data: Binary,
}

pub fn screencast_frames(
    connection: &cdp::Connection,
) -> Result<mpmc::Receiver<Arc<ScreencastFrame>>> {
    let (tx, rx) = mpmc::bounded::<Arc<ScreencastFrame>>(32);
    let frames = connection.events.subscribe::<page::EventScreencastFrame>();

    thread::spawn(move || {
        while let Ok(Some(event)) = frames.next() {
            let Some(timestamp) = event.metadata.timestamp else {
                log::warn!("ignoring screencast frame without timestamp");
                continue;
            };
            let timestamp =
                UNIX_EPOCH + Duration::from_secs_f64(*timestamp.inner());
            if tx
                .send(Arc::new(ScreencastFrame {
                    data: event.data,
                    timestamp,
                }))
                .is_err()
            {
                break;
            }
        }
    });

    Ok(rx)
}
