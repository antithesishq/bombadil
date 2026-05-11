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

/// Centralized screencast subscription. Starts `Page.startScreencast`
/// once, listens for frames, acks them, decodes the image bytes, and
/// rebroadcasts via a tokio broadcast channel. Both activity tracking
/// and screenshot capture subscribe to this single source.
pub struct Screencast {
    sender: broadcast::Sender<Arc<Vec<u8>>>,
}

impl Screencast {
    /// Start the screencast and begin listening for frames.
    pub async fn start(
        page: &Arc<Page>,
        width: u16,
        height: u16,
    ) -> anyhow::Result<Self> {
        page.execute(
            page::StartScreencastParams::builder()
                .format(page::StartScreencastFormat::Jpeg)
                .quality(50)
                .max_width(width)
                .max_height(height)
                .build(),
        )
        .await?;

        let (sender, _) = broadcast::channel::<Arc<Vec<u8>>>(16);

        let frames =
            page.event_listener::<page::EventScreencastFrame>().await?;

        let tx = sender.clone();
        let page_for_ack = page.clone();
        tokio::spawn(async move {
            tokio::pin!(frames);
            log::debug!("screencast: listener started");
            while let Some(event) = frames.next().await {
                log::debug!(
                    "screencast: frame received (session_id={})",
                    event.session_id
                );
                let bytes = match base64::prelude::BASE64_STANDARD
                    .decode(&event.data)
                {
                    Ok(b) => b,
                    Err(e) => {
                        log::warn!("screencast: decode failed: {}", e);
                        continue;
                    }
                };
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
                let _ = tx.send(Arc::new(bytes));
            }
            log::debug!("screencast: listener ended");
        });

        Ok(Screencast { sender })
    }

    /// Subscribe to decoded frame bytes.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Vec<u8>>> {
        self.sender.subscribe()
    }
}

/// Wraps a [`Screencast`] subscription as a quiescence activity
/// stream. Each frame bumps the deadline, up to a maximum number
/// of bumps per window to avoid infinite animations blocking
/// quiescence.
pub struct ScreencastActivity {
    screencast: Arc<Screencast>,
}

impl ScreencastActivity {
    pub fn new(screencast: Arc<Screencast>) -> Self {
        ScreencastActivity { screencast }
    }

    pub fn stream(&self) -> ActivityStream {
        let receiver = self.screencast.subscribe();
        let mut count = 0u32;
        Box::pin(
            tokio_stream::wrappers::BroadcastStream::new(receiver)
                .filter_map(|result| async { result.ok() })
                .filter_map(move |_| {
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
