use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::network;
use chromiumoxide::cdp::browser_protocol::page;
use futures::{Stream, StreamExt, stream};
use tokio::sync::broadcast;

/// Maximum number of times a single URL can trigger activity before
/// it is considered background noise and filtered out.
const MAX_HITS_PER_URL: u32 = 3;

/// Maximum number of screencast frames that can bump the quiescence
/// timer in a single window. Prevents perpetual animations (CSS
/// transitions, blinking cursors, etc.) from blocking quiescence
/// indefinitely.
const MAX_FRAME_BUMPS: u32 = 10;

/// How long a network event extends the quiescence deadline.
const NETWORK_BUMP: Duration = Duration::from_millis(100);

/// How long a screencast frame extends the quiescence deadline.
const FRAME_BUMP: Duration = Duration::from_millis(32);

pub type ActivityStream = Pin<Box<dyn Stream<Item = Duration> + Send>>;

/// A shared handle to network activity on a page.  Subscribes to CDP
/// network events once; each call to [`stream`] returns a fresh,
/// per-URL-deduplicated activity stream for a new quiescence window.
pub struct NetworkActivity {
    sender: broadcast::Sender<String>,
}

impl NetworkActivity {
    /// Subscribe to CDP network events on `page`.  The returned
    /// handle is cheap to clone and lives for the browser session.
    pub async fn subscribe(page: &Arc<Page>) -> anyhow::Result<Self> {
        let (sender, _) = broadcast::channel::<String>(256);

        let requests = page
            .event_listener::<network::EventRequestWillBeSent>()
            .await?
            .map(|event| event.request.url.clone());

        let responses = page
            .event_listener::<network::EventResponseReceived>()
            .await?
            .map(|event| event.response.url.clone());

        let merged = stream::select_all(vec![
            Box::pin(requests) as Pin<Box<dyn Stream<Item = String> + Send>>,
            Box::pin(responses),
        ]);

        let tx = sender.clone();
        tokio::spawn(async move {
            tokio::pin!(merged);
            while let Some(url) = merged.next().await {
                let _ = tx.send(url);
            }
        });

        Ok(NetworkActivity { sender })
    }

    /// Create a new deduplicated activity stream.  Each URL may
    /// trigger at most [`MAX_HITS_PER_URL`] events before being
    /// silenced.  The dedup state is local to this stream.
    pub fn stream(&self) -> ActivityStream {
        let receiver = self.sender.subscribe();
        let urls = tokio_stream::wrappers::BroadcastStream::new(receiver)
            .filter_map(|result| async { result.ok() });
        Box::pin(limit_per_url(urls))
    }
}

/// A shared handle to screencast frame activity. Subscribes to
/// `Page.screencastFrame` events once; each call to [`stream`]
/// returns a fresh activity stream for a new quiescence window.
pub struct ScreencastActivity {
    sender: broadcast::Sender<()>,
    page: Arc<Page>,
}

fn screencast_params() -> page::StartScreencastParams {
    page::StartScreencastParams::builder()
        .format(page::StartScreencastFormat::Jpeg)
        .quality(50)
        .max_width(800)
        .max_height(600)
        .build()
}

impl ScreencastActivity {
    /// Start the screencast and subscribe to frame events on `page`.
    pub async fn subscribe(page: &Arc<Page>) -> anyhow::Result<Self> {
        page.execute(screencast_params()).await?;

        let (sender, _) = broadcast::channel::<()>(256);

        let frames =
            page.event_listener::<page::EventScreencastFrame>().await?;

        let tx = sender.clone();
        let page_for_ack = page.clone();
        let debug_dir = std::path::PathBuf::from("/tmp/bombadil-screencast");
        let _ = std::fs::create_dir_all(&debug_dir);
        let mut frame_counter = 0u64;
        tokio::spawn(async move {
            tokio::pin!(frames);
            log::debug!("screencast: listener started");
            while let Some(event) = frames.next().await {
                log::debug!(
                    "screencast: frame received (session_id={})",
                    event.session_id
                );
                // Save frame to disk for debugging.
                let path = debug_dir.join(format!("{:06}.jpg", frame_counter));
                if let Ok(bytes) =
                    base64::prelude::BASE64_STANDARD.decode(&event.data)
                {
                    let _ = std::fs::write(&path, &bytes);
                }
                frame_counter += 1;
                // Acknowledge so Chrome keeps sending frames.
                match page_for_ack
                    .execute(page::ScreencastFrameAckParams::new(
                        event.session_id,
                    ))
                    .await
                {
                    Ok(_) => log::debug!("screencast: ack sent"),
                    Err(e) => log::warn!("screencast: ack failed: {}", e),
                }
                let _ = tx.send(());
            }
            log::debug!("screencast: listener ended");
        });

        Ok(ScreencastActivity {
            sender,
            page: page.clone(),
        })
    }

    /// Re-issue StartScreencast so Chrome resumes sending frames
    /// (e.g. after a debugger pause/resume cycle).
    pub async fn restart(&self) {
        if let Err(e) = self.page.execute(screencast_params()).await {
            log::warn!("screencast: restart failed: {}", e);
        }
    }

    /// Stop the screencast so the renderer is free for other
    /// operations (screenshots, debugger pause).
    pub async fn stop(&self) {
        if let Err(e) = self
            .page
            .execute(page::StopScreencastParams::default())
            .await
        {
            log::warn!("screencast: stop failed: {}", e);
        }
    }

    pub fn stream(&self) -> ActivityStream {
        let receiver = self.sender.subscribe();
        let mut count = 0u32;
        Box::pin(
            tokio_stream::wrappers::BroadcastStream::new(receiver)
                .filter_map(|result| async { result.ok() })
                .filter_map(move |()| {
                    count += 1;
                    if count <= MAX_FRAME_BUMPS {
                        std::future::ready(Some(FRAME_BUMP))
                    } else {
                        std::future::ready(None)
                    }
                }),
        )
    }
}

/// Filter a stream of URLs, emitting [`NETWORK_BUMP`] for each URL
/// that has not yet exceeded [`MAX_HITS_PER_URL`].
fn limit_per_url(
    urls: impl Stream<Item = String> + Send + 'static,
) -> impl Stream<Item = Duration> + Send + 'static {
    let mut counts: HashMap<String, u32> = HashMap::new();
    urls.filter_map(move |url| {
        let count = counts.entry(url).or_insert(0);
        *count += 1;
        if *count <= MAX_HITS_PER_URL {
            std::future::ready(Some(NETWORK_BUMP))
        } else {
            std::future::ready(None)
        }
    })
}
