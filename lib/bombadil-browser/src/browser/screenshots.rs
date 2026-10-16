use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use base64::Engine;
use cdp::Binary;
use cdp_protocol::cdp::browser_protocol::target::SessionId;
use cdp_protocol::cdp::browser_protocol::{headless_experimental, page};
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

/// Interval between frames driven by the background loop.
const FRAME_LOOP_INTERVAL: Duration = Duration::from_millis(16);

/// Drives frames of a target created with `begin_frame_control` enabled.
///
/// A background loop requests frames at a fixed interval, as pages can depend
/// on rendering in many ways and block CDP calls. The loop is paused only
/// where frames would interfere, see [`FrameControl::paused`].
///
/// Chrome rejects a `beginFrame` while another is in flight ("Another
/// frame is pending"), so all frames for a target go through one lock.
#[derive(Clone)]
pub struct FrameControl {
    connection: cdp::Connection,
    session_id: SessionId,
    lock: Arc<Mutex<()>>,
    pauses: Arc<AtomicUsize>,
}

impl std::fmt::Debug for FrameControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameControl")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl FrameControl {
    pub fn new(connection: cdp::Connection, session_id: SessionId) -> Self {
        FrameControl {
            connection,
            session_id,
            lock: Arc::new(Mutex::new(())),
            pauses: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn begin_frame(
        &self,
        params: headless_experimental::BeginFrameParams,
    ) -> Result<headless_experimental::BeginFrameReturns> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| anyhow!("begin frame lock poisoned"))?;
        self.connection.send(params, Some(&self.session_id))
    }

    /// Start the background loop, running until `terminated` is set.
    pub fn start_background_loop(&self, terminated: Arc<AtomicBool>) {
        let this = self.clone();
        thread::spawn(move || {
            while !terminated.load(Ordering::SeqCst) {
                if let Err(error) = this.request_frame() {
                    if !terminated.load(Ordering::SeqCst) {
                        log::warn!("background frame loop stopped: {error:#}");
                    }
                    break;
                }
                thread::sleep(FRAME_LOOP_INTERVAL);
            }
        });
    }

    fn request_frame(&self) -> Result<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| anyhow!("begin frame lock poisoned"))?;
        // Checked while holding the lock, so that once `paused` has taken
        // the lock no new background loop frame can start until it's resumed.
        if self.pauses.load(Ordering::SeqCst) > 0 {
            return Ok(());
        }
        self.connection.send(
            headless_experimental::BeginFrameParams::default(),
            Some(&self.session_id),
        )?;
        Ok(())
    }

    /// Run `f` with the background loop paused. Explicit frame requests still go through.
    /// This is for discrete input events, as
    /// frames overlapping them can get stuck, and for settling and
    /// capturing, which drive their own frames.
    pub fn paused<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Resume<'a>(&'a AtomicUsize);
        impl Drop for Resume<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.pauses.fetch_add(1, Ordering::SeqCst);
        let _resume = Resume(&self.pauses);
        // Wait for any in-flight loop frame to finish.
        drop(self.lock.lock());
        f()
    }

    /// Request a single frame without capturing it. Returns whether the frame had
    /// "damage", which means something on-screen changed.
    pub fn frame(&self) -> Result<bool> {
        Ok(self
            .begin_frame(headless_experimental::BeginFrameParams::default())?
            .has_damage)
    }

    /// Request a single frame and capture it (if possible).
    pub fn capture(&self) -> Result<Option<Binary>> {
        let result = self.begin_frame(
            headless_experimental::BeginFrameParams::builder()
                .screenshot(
                    headless_experimental::ScreenshotParams::builder()
                        .format(SCREENSHOT_FORMAT)
                        .quality(SCREENSHOT_QUALITY)
                        .optimize_for_speed(true)
                        .build(),
                )
                .build(),
        )?;
        log::debug!("begin frame: has_damage={}", result.has_damage);
        Ok(result.screenshot_data)
    }
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
