use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use chromiumoxide::cdp::browser_protocol::network;
use chromiumoxide::Page;
use futures::{Stream, StreamExt, stream};
use tokio::sync::broadcast;

/// Maximum number of times a single URL can trigger activity before
/// it is considered background noise and filtered out.
const MAX_HITS_PER_URL: u32 = 3;

pub type ActivityStream = Pin<Box<dyn Stream<Item = ()> + Send>>;

/// A shared handle to network activity on a page.  Subscribes to CDP
/// network events once; each call to [`stream`] returns a fresh,
/// per-URL-deduplicated activity stream for a new quiescence window.
pub struct NetworkActivity {
    sender: broadcast::Sender<String>,
}

impl NetworkActivity {
    /// Subscribe to CDP network events on `page`.  The returned
    /// handle is cheap to clone and lives for the browser session.
    pub async fn subscribe(
        page: &Arc<Page>,
    ) -> anyhow::Result<Self> {
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
            Box::pin(requests)
                as Pin<Box<dyn Stream<Item = String> + Send>>,
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

/// Filter a stream of URLs, emitting `()` for each URL that has not
/// yet exceeded [`MAX_HITS_PER_URL`].
fn limit_per_url(
    urls: impl Stream<Item = String> + Send + 'static,
) -> impl Stream<Item = ()> + Send + 'static {
    let mut counts: HashMap<String, u32> = HashMap::new();
    urls.filter_map(move |url| {
        let count = counts.entry(url).or_insert(0);
        *count += 1;
        if *count <= MAX_HITS_PER_URL {
            std::future::ready(Some(()))
        } else {
            std::future::ready(None)
        }
    })
}
