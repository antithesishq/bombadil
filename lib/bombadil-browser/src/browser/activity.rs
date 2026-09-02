use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use cdp::Binary;
use cdp::types::try_match;
use cdp_protocol::cdp::browser_protocol::network;
use cdp_protocol::cdp::browser_protocol::page;
use cdp_protocol::cdp::browser_protocol::target::SessionId;
use crossbeam_channel as mpmc;

/// Maximum number of times a single URL can trigger activity before
/// it is considered background noise and filtered out.
const MAX_HITS_PER_URL: u32 = 3;

/// How long a new outgoing request extends the quiescence deadline.
const NETWORK_BUMP_REQUEST: Duration = Duration::from_millis(100);

/// How long an incoming response extends the quiescence deadline.
const NETWORK_BUMP_RESPONSE: Duration = Duration::from_millis(10);

/// Maximum number of screencast frames that can bump the quiescence
/// timer in a single window. Prevents perpetual animations (CSS
/// transitions, blinking cursors, etc.) from blocking quiescence
/// indefinitely.
const FRAME_BUMP_COUNT_MAX: u32 = 10;

/// How long a screencast frame extends the quiescence deadline.
const FRAME_BUMP: Duration = Duration::from_millis(32);

pub type ActivityStream = mpmc::Receiver<Duration>;

pub fn all_activity(
    events: &cdp::Events,
    activity_tx: mpmc::Sender<Duration>,
) -> Result<()> {
    network_activity(events, activity_tx.clone())?;
    screencast_activity(events, activity_tx)?;
    Ok(())
}

pub fn network_activity(
    events: &cdp::Events,
    activity_tx: mpmc::Sender<Duration>,
) -> Result<()> {
    let all = events.all();
    thread::spawn(move || -> Result<()> {
        let mut hit_counts: HashMap<String, u32> = HashMap::new();
        while let Ok(event) = all.try_recv() {
            let result = try_match!(event, {
                network::EventRequestWillBeSent: event => Some((event.request.url.clone(), NETWORK_BUMP_REQUEST)),
                network::EventResponseReceived: event => Some((event.response.url.clone(), NETWORK_BUMP_RESPONSE)),
            }, _ => None);
            if let Some((url, bump)) = result {
                let count = hit_counts.entry(url).or_insert(0);
                *count += 1;
                if *count <= MAX_HITS_PER_URL {
                    activity_tx.send(bump)?;
                }
            }
        }
        Ok(())
    });

    Ok(())
}

pub fn screencast_start(
    connection: &cdp::Connection,
    session_id: &SessionId,
    width: u16,
    height: u16,
) -> Result<mpmc::Receiver<Arc<Binary>>> {
    let (tx, rx) = mpmc::bounded::<Arc<Binary>>(32);
    let frames = connection.events.subscribe::<page::EventScreencastFrame>();

    connection.send(
        page::StartScreencastParams::builder()
            .format(page::StartScreencastFormat::Jpeg)
            .quality(50)
            .max_width(width)
            .max_height(height)
            .build(),
        Some(session_id),
    )?;

    let conn = connection.clone();
    let session_id = session_id.clone();
    thread::spawn(move || -> Result<()> {
        log::debug!("screencast: listener started");
        while let Some(event) = frames.next()? {
            log::debug!(
                "screencast: frame received (session_id={})",
                event.session_id
            );
            match conn.post(
                page::ScreencastFrameAckParams::new(event.session_id),
                Some(&session_id),
            ) {
                Ok(()) => log::debug!("screencast: ack posted"),
                Err(e) => log::warn!("screencast: ack failed: {}", e),
            }
            let _ = tx.send(Arc::new(event.data));
        }
        log::debug!("screencast: listener ended");
        Ok(())
    });

    Ok(rx)
}

pub fn screencast_activity(
    events: &cdp::Events,
    activity_tx: mpmc::Sender<Duration>,
) -> Result<()> {
    let rx = events.subscribe::<page::EventScreencastFrame>();
    thread::spawn(move || -> Result<()> {
        let mut count = 0u32;
        while rx.next()?.is_some() {
            count += 1;
            if count <= FRAME_BUMP_COUNT_MAX {
                activity_tx.send(FRAME_BUMP)?;
            }
        }
        Ok(())
    });
    Ok(())
}
