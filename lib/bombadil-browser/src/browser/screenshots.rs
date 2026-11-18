use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::SystemTime;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use cdp::{Binary, PendingResponse};
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
        data: data.into(),
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
/// frame is pending"), so all frames for a target go through one lock. A
/// frame whose response timed out is still in flight in Chrome, so the lock
/// holds it until it finishes, and the next frame waits for it.
#[derive(Clone)]
pub struct FrameControl {
    connection: cdp::Connection,
    session_id: SessionId,
    in_flight: Arc<Mutex<Option<PendingBeginFrame>>>,
    pauses: Arc<AtomicUsize>,
}

type PendingBeginFrame =
    PendingResponse<headless_experimental::BeginFrameParams>;

/// How long to wait for a frame, as for any other CDP command.
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

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
            in_flight: Arc::new(Mutex::new(None)),
            pauses: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Option<PendingBeginFrame>>> {
        self.in_flight
            .lock()
            .map_err(|_| anyhow!("begin frame lock poisoned"))
    }

    fn begin_frame(
        &self,
        params: headless_experimental::BeginFrameParams,
    ) -> Result<headless_experimental::BeginFrameReturns> {
        self.begin_frame_locked(&mut *self.lock()?, params)
    }

    /// Send a frame while holding the lock, first waiting for any frame still
    /// in flight, as Chrome would reject this one.
    fn begin_frame_locked(
        &self,
        in_flight: &mut Option<PendingBeginFrame>,
        params: headless_experimental::BeginFrameParams,
    ) -> Result<headless_experimental::BeginFrameReturns> {
        if let Some(previous) = in_flight {
            // An error response or a closed connection also ends the frame.
            if let Ok(None) =
                previous.wait_deadline(Instant::now() + FRAME_TIMEOUT)
            {
                bail!("previous begin frame still pending");
            }
            *in_flight = None;
        }
        let pending =
            self.connection.request(params, Some(&self.session_id))?;
        match pending.wait_deadline(Instant::now() + FRAME_TIMEOUT)? {
            Some(result) => Ok(result),
            None => {
                *in_flight = Some(pending);
                bail!("timed out waiting for begin frame");
            }
        }
    }

    /// Start the background loop, running until `terminated` is set.
    pub fn start_background_loop(&self, terminated: Arc<AtomicBool>) {
        let this = self.clone();
        thread::spawn(move || {
            while !terminated.load(Ordering::SeqCst) {
                // Keep going after an error, as a frame can time out while
                // the page is busy and still complete later.
                if let Err(error) = this.request_frame()
                    && !terminated.load(Ordering::SeqCst)
                {
                    log::debug!("background frame failed: {error:#}");
                }
                thread::sleep(FRAME_LOOP_INTERVAL);
            }
        });
    }

    fn request_frame(&self) -> Result<()> {
        let mut in_flight = self.lock()?;
        // Checked while holding the lock, so that once `paused` has taken
        // the lock no new background loop frame can start until it's resumed.
        if self.pauses.load(Ordering::SeqCst) > 0 {
            return Ok(());
        }
        self.begin_frame_locked(
            &mut in_flight,
            headless_experimental::BeginFrameParams::default(),
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
        drop(self.in_flight.lock());
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
    latest_frame: Arc<Mutex<Option<Arc<ScreencastFrame>>>>,
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
                    // Chrome only sends a frame when something on screen
                    // changed, so the newest frame is still accurate until
                    // the next one arrives.
                    let timestamp = event
                        .metadata
                        .timestamp
                        .map(|timestamp| {
                            UNIX_EPOCH
                                + Duration::from_secs_f64(*timestamp.inner())
                        })
                        .unwrap_or_else(SystemTime::now);
                    *latest_frame
                        .lock()
                        .map_err(|_| anyhow!("latest frame lock poisoned"))? =
                        Some(Arc::new(ScreencastFrame {
                            timestamp,
                            data: event.data,
                        }));
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
