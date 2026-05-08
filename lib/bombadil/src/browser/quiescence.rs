use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

/// A timer that waits for a burst of activity to settle.
///
/// Each call to [`bump`] resets a short idle countdown. The timer
/// resolves when either:
/// - the idle timeout expires (no bump received for `timeout_idle`), or
/// - the max timeout fires regardless of activity.
pub struct QuiescenceTimer {
    sender: mpsc::Sender<()>,
}

pub struct QuiescenceWaiter {
    receiver: mpsc::Receiver<()>,
    timeout_idle: Duration,
    timeout_max: Duration,
}

impl QuiescenceTimer {
    pub fn new(
        timeout_idle: Duration,
        timeout_max: Duration,
    ) -> (Self, QuiescenceWaiter) {
        let (sender, receiver) = mpsc::channel(64);
        (
            QuiescenceTimer { sender },
            QuiescenceWaiter {
                receiver,
                timeout_idle,
                timeout_max,
            },
        )
    }

    /// Signal that activity occurred, resetting the idle countdown.
    pub fn bump(&self) {
        // Best-effort: if the channel is full or closed, ignore.
        let _ = self.sender.try_send(());
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl QuiescenceWaiter {
    /// Wait until the browser is quiescent (idle timeout elapsed) or
    /// the max timeout fires, whichever comes first.
    pub async fn wait(mut self) {
        let deadline = Instant::now() + self.timeout_max;
        let mut deadline_idle = Instant::now() + self.timeout_idle;

        loop {
            let next = deadline_idle.min(deadline);
            tokio::select! {
                _ = sleep_until(next) => {
                    break;
                }
                bump = self.receiver.recv() => {
                    match bump {
                        Some(()) => {
                            deadline_idle =
                                (Instant::now() + self.timeout_idle)
                                    .min(deadline);
                        }
                        // Sender dropped — treat as quiescent.
                        None => break,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant as StdInstant;

    #[tokio::test]
    async fn fires_after_timeout_idle_with_no_bumps() {
        let (_timer, waiter) = QuiescenceTimer::new(
            Duration::from_millis(100),
            Duration::from_secs(5),
        );
        let start = StdInstant::now();
        waiter.wait().await;
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(80));
        assert!(elapsed < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn bumps_extend_timeout_idle() {
        let (timer, waiter) = QuiescenceTimer::new(
            Duration::from_millis(150),
            Duration::from_secs(5),
        );
        let start = StdInstant::now();

        tokio::spawn(async move {
            for _ in 0..5 {
                tokio::time::sleep(Duration::from_millis(80)).await;
                timer.bump();
            }
        });

        waiter.wait().await;
        let elapsed = start.elapsed();
        // 5 bumps at 80ms intervals = ~400ms of activity, then
        // 150ms idle
        assert!(elapsed >= Duration::from_millis(400));
    }

    #[tokio::test]
    async fn timeout_max_caps_wait() {
        let (timer, waiter) = QuiescenceTimer::new(
            Duration::from_millis(100),
            Duration::from_millis(300),
        );
        let start = StdInstant::now();

        // Bump continuously — should still finish at max timeout.
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if timer.is_closed() {
                    break;
                }
                timer.bump();
            }
        });

        waiter.wait().await;
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(250));
        assert!(elapsed < Duration::from_millis(600));
    }

    #[tokio::test]
    async fn sender_drop_resolves_immediately() {
        let (timer, waiter) = QuiescenceTimer::new(
            Duration::from_secs(10),
            Duration::from_secs(10),
        );
        drop(timer);
        let start = StdInstant::now();
        waiter.wait().await;
        assert!(start.elapsed() < Duration::from_millis(100));
    }
}
