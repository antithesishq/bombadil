use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::network;
use chromiumoxide::cdp::browser_protocol::page;
use futures::{Stream, StreamExt, stream};
use tokio::sync::broadcast;

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

pub type ActivityStream = Pin<Box<dyn Stream<Item = Duration> + Send>>;

pub struct NetworkActivity {
    sender: broadcast::Sender<(String, Duration)>,
}

impl NetworkActivity {
    pub async fn subscribe(page: &Arc<Page>) -> Result<Self> {
        let (sender, _) = broadcast::channel::<(String, Duration)>(256);

        let requests = page
            .event_listener::<network::EventRequestWillBeSent>()
            .await?
            .map(|event| (event.request.url.clone(), NETWORK_BUMP_REQUEST));

        let responses = page
            .event_listener::<network::EventResponseReceived>()
            .await?
            .map(|event| (event.response.url.clone(), NETWORK_BUMP_RESPONSE));

        let merged = stream::select_all(vec![
            Box::pin(requests)
                as Pin<Box<dyn Stream<Item = (String, Duration)> + Send>>,
            Box::pin(responses),
        ]);

        let tx = sender.clone();
        tokio::spawn(async move {
            tokio::pin!(merged);
            while let Some(pair) = merged.next().await {
                let _ = tx.send(pair);
            }
        });

        Ok(NetworkActivity { sender })
    }

    pub fn stream(&self) -> ActivityStream {
        let receiver = self.sender.subscribe();
        let events = tokio_stream::wrappers::BroadcastStream::new(receiver)
            .filter_map(|result| async { result.ok() });
        Box::pin(limit_per_url(events))
    }
}

pub struct Screencast {
    sender: broadcast::Sender<Arc<[u8]>>,
}

impl Screencast {
    pub async fn start(
        page: &Arc<Page>,
        width: u16,
        height: u16,
    ) -> Result<Self> {
        page.execute(
            page::StartScreencastParams::builder()
                .format(page::StartScreencastFormat::Jpeg)
                .quality(50)
                .max_width(width)
                .max_height(height)
                .build(),
        )
        .await?;

        let (sender, _) = broadcast::channel::<Arc<[u8]>>(16);
        let frames =
            page.event_listener::<page::EventScreencastFrame>().await?;
        let tx = sender.clone();
        let page = page.clone();

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
                match page
                    .execute(page::ScreencastFrameAckParams::new(
                        event.session_id,
                    ))
                    .await
                {
                    Ok(_) => log::debug!("screencast: ack sent"),
                    Err(e) => log::warn!("screencast: ack failed: {}", e),
                }
                let _ = tx.send(Arc::from(bytes));
            }
            log::debug!("screencast: listener ended");
        });

        Ok(Screencast { sender })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<[u8]>> {
        self.sender.subscribe()
    }
}

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
                    if count <= FRAME_BUMP_COUNT_MAX {
                        std::future::ready(Some(FRAME_BUMP))
                    } else {
                        std::future::ready(None)
                    }
                }),
        )
    }
}

fn limit_per_url(
    events: impl Stream<Item = (String, Duration)> + Send + 'static,
) -> impl Stream<Item = Duration> + Send + 'static {
    let mut counts: HashMap<String, u32> = HashMap::new();
    events.filter_map(move |(url, bump)| {
        let count = counts.entry(url).or_insert(0);
        *count += 1;
        if *count <= MAX_HITS_PER_URL {
            std::future::ready(Some(bump))
        } else {
            std::future::ready(None)
        }
    })
}
