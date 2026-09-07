use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use cdp::types::try_match;
use cdp_protocol::cdp::browser_protocol::{network, page};
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
const FRAME_BUMP: Duration = Duration::from_millis(8);

#[derive(Debug)]
pub struct ActivityStream {
    receiver: mpmc::Receiver<Duration>,
    cancel_tx: Option<mpmc::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ActivityStream {
    pub(crate) fn receiver(&self) -> &mpmc::Receiver<Duration> {
        &self.receiver
    }
}

impl From<mpmc::Receiver<Duration>> for ActivityStream {
    fn from(receiver: mpmc::Receiver<Duration>) -> Self {
        Self {
            receiver,
            cancel_tx: None,
            worker: None,
        }
    }
}

impl Drop for ActivityStream {
    fn drop(&mut self) {
        if let Some(cancel_tx) = self.cancel_tx.take() {
            let _ = cancel_tx.try_send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn all_activity(events: &cdp::Events) -> Result<ActivityStream> {
    let all = events.all();
    let (activity_tx, activity_rx) = mpmc::unbounded();
    let (cancel_tx, cancel_rx) = mpmc::bounded(1);

    let worker = thread::spawn(move || {
        let mut hit_counts: HashMap<String, u32> = HashMap::new();
        let mut frame_count = 0u32;

        loop {
            let event = mpmc::select_biased! {
                recv(cancel_rx) -> _ => break,
                recv(all) -> event => match event {
                    Ok(event) => event,
                    Err(mpmc::RecvError) => break,
                },
            };
            let method = event.method.clone();
            let bump = (|| -> Result<Option<Duration>> {
                Ok(try_match!(event, {
                    network::EventRequestWillBeSent: event => {
                        let count = hit_counts
                            .entry(event.request.url.clone())
                            .or_insert(0);
                        *count += 1;
                        (*count <= MAX_HITS_PER_URL)
                            .then_some(NETWORK_BUMP_REQUEST)
                    },
                    network::EventResponseReceived: event => {
                        let count = hit_counts
                            .entry(event.response.url.clone())
                            .or_insert(0);
                        *count += 1;
                        (*count <= MAX_HITS_PER_URL)
                            .then_some(NETWORK_BUMP_RESPONSE)
                    },
                    page::EventScreencastFrame => {
                        frame_count += 1;
                        (frame_count <= FRAME_BUMP_COUNT_MAX)
                            .then_some(FRAME_BUMP)
                    },
                }, _ => None))
            })();
            let bump = match bump {
                Ok(bump) => bump,
                Err(error) => {
                    log::warn!(
                        "failed parsing activity event {method}: {error}"
                    );
                    continue;
                }
            };

            if let Some(bump) = bump
                && activity_tx.send(bump).is_err()
            {
                break;
            }
        }
    });

    Ok(ActivityStream {
        receiver: activity_rx,
        cancel_tx: Some(cancel_tx),
        worker: Some(worker),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_stream_cancels_and_joins_worker() {
        let (_activity_tx, activity_rx) = mpmc::unbounded();
        let (cancel_tx, cancel_rx) = mpmc::bounded(1);
        let (stopped_tx, stopped_rx) = mpmc::bounded(1);
        let worker = thread::spawn(move || {
            let _ = cancel_rx.recv();
            stopped_tx.send(()).unwrap();
        });
        let stream = ActivityStream {
            receiver: activity_rx,
            cancel_tx: Some(cancel_tx),
            worker: Some(worker),
        };

        drop(stream);

        stopped_rx
            .try_recv()
            .expect("activity worker was not joined");
    }
}
