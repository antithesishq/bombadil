use std::pin::Pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::time::{Instant, sleep_until};

/// A handle representing an active quiescence timer.
///
/// Held by the state machine to keep the timer alive. When dropped,
/// the corresponding waiter resolves as cancelled (not quiescent).
pub struct QuiescenceTimer {
    _cancel: tokio::sync::oneshot::Sender<()>,
}

struct QuiescenceWaiter {
    cancel: tokio::sync::oneshot::Receiver<()>,
    activity: Pin<Box<dyn Stream<Item = Duration> + Send>>,
    timeout_idle: Duration,
    timeout_max: Duration,
}

/// Create a quiescence timer driven by an activity stream.
///
/// Each item in the stream is a [`Duration`] that bumps the idle
/// deadline by that amount from now, allowing different activity
/// sources (network, screencast frames, etc.) to control their own
/// settling delay.
///
/// Returns a handle and a future. The future resolves with `true`
/// when the system is quiescent (idle timeout or max timeout
/// elapsed), or `false` if the handle was dropped (cancelled).
pub fn start(
    timeout_idle: Duration,
    timeout_max: Duration,
    activity: Pin<Box<dyn Stream<Item = Duration> + Send>>,
) -> (QuiescenceTimer, impl std::future::Future<Output = bool> + Send) {
    let (cancel_sender, cancel_receiver) = tokio::sync::oneshot::channel();
    let waiter = QuiescenceWaiter {
        cancel: cancel_receiver,
        activity,
        timeout_idle,
        timeout_max,
    };
    (
        QuiescenceTimer {
            _cancel: cancel_sender,
        },
        waiter.wait(),
    )
}

impl QuiescenceWaiter {
    async fn wait(mut self) -> bool {
        let deadline = Instant::now() + self.timeout_max;
        let mut deadline_idle = Instant::now() + self.timeout_idle;

        loop {
            let next = deadline_idle.min(deadline);
            tokio::select! {
                _ = sleep_until(next) => {
                    return true;
                }
                _ = &mut self.cancel => {
                    return false;
                }
                event = self.activity.next() => {
                    match event {
                        Some(bump) => {
                            deadline_idle =
                                (Instant::now() + bump)
                                    .min(deadline);
                        }
                        None => {
                            self.activity =
                                Box::pin(futures::stream::pending());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use tokio::time::Instant;

    fn empty_activity() -> Pin<Box<dyn Stream<Item = Duration> + Send>> {
        Box::pin(stream::empty())
    }

    #[tokio::test]
    async fn fires_after_timeout_idle_with_no_activity() {
        let (_timer, wait) = start(
            Duration::from_millis(100),
            Duration::from_secs(5),
            empty_activity(),
        );
        let t = Instant::now();
        assert!(wait.await);
        let elapsed = t.elapsed();
        assert!(elapsed >= Duration::from_millis(80));
        assert!(elapsed < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn stream_activity_extends_idle() {
        let bump = Duration::from_millis(150);
        let activity = Box::pin(stream::unfold(0u32, move |i| async move {
            if i < 5 {
                tokio::time::sleep(Duration::from_millis(80)).await;
                Some((bump, i + 1))
            } else {
                None
            }
        }));

        let (_timer, wait) = start(
            Duration::from_millis(150),
            Duration::from_secs(5),
            activity,
        );
        let t = Instant::now();
        assert!(wait.await);
        let elapsed = t.elapsed();
        assert!(elapsed >= Duration::from_millis(400));
    }

    #[tokio::test]
    async fn timeout_max_caps_wait() {
        let bump = Duration::from_millis(100);
        let activity = Box::pin(stream::unfold((), move |()| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((bump, ()))
        }));

        let (_timer, wait) = start(
            Duration::from_millis(100),
            Duration::from_millis(300),
            activity,
        );
        let t = Instant::now();
        assert!(wait.await);
        let elapsed = t.elapsed();
        assert!(elapsed >= Duration::from_millis(250));
        assert!(elapsed < Duration::from_millis(600));
    }

    #[tokio::test]
    async fn drop_handle_cancels() {
        let (timer, wait) = start(
            Duration::from_secs(10),
            Duration::from_secs(10),
            empty_activity(),
        );
        drop(timer);
        let t = Instant::now();
        assert!(!wait.await);
        assert!(t.elapsed() < Duration::from_millis(100));
    }
}
