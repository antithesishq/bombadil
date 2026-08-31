use crossbeam_channel as mpmc;
use std::{
    thread,
    time::{Duration, Instant},
};

/// A subscription that buffers activity signals before the countdown
/// begins. Call [`start`](QuiescenceSubscription::start) to begin
/// the actual timer.
pub struct QuiescenceSubscription {
    cancel_tx: mpmc::Sender<()>,
    cancel_rx: mpmc::Receiver<()>,
    activity_rx: mpmc::Receiver<Duration>,
}

/// A handle representing an active quiescence timer.
///
/// Keeps the timer alive. When dropped, the corresponding waiter resolves as
/// cancelled (not quiescent).
pub struct QuiescenceTimer {
    _cancel_tx: mpmc::Sender<()>,
}

struct QuiescenceWaiter {
    cancel_rx: mpmc::Receiver<()>,
    activity_rx: mpmc::Receiver<Duration>,
    timeout_idle: Duration,
    timeout_max: Duration,
}

/// Begin buffering activity signals without starting the countdown.
///
/// The returned [`QuiescenceSubscription`] holds the activity stream
/// so that signals arriving before [`QuiescenceSubscription::start`]
/// are not lost.
pub fn subscribe(activity: mpmc::Receiver<Duration>) -> QuiescenceSubscription {
    let (cancel_sender, cancel_receiver) = mpmc::bounded(1);
    QuiescenceSubscription {
        cancel_tx: cancel_sender,
        cancel_rx: cancel_receiver,
        activity_rx: activity,
    }
}

impl QuiescenceSubscription {
    /// Start the countdown. Returns a timer handle and a future that
    /// resolves with `true` when quiescent, or `false` if cancelled.
    pub fn start(
        self,
        timeout_idle: Duration,
        timeout_max: Duration,
    ) -> (QuiescenceTimer, mpmc::Receiver<bool>) {
        let waiter = QuiescenceWaiter {
            cancel_rx: self.cancel_rx,
            activity_rx: self.activity_rx,
            timeout_idle,
            timeout_max,
        };
        (
            QuiescenceTimer {
                _cancel_tx: self.cancel_tx,
            },
            waiter.wait(),
        )
    }
}

impl QuiescenceWaiter {
    fn wait(self) -> mpmc::Receiver<bool> {
        let (result_tx, result_rx) = mpmc::bounded(1);

        let _ = thread::spawn(move || {
            let deadline_max = Instant::now() + self.timeout_max;
            let mut deadline_idle = Instant::now() + self.timeout_idle;
            loop {
                let deadline_next = deadline_idle.min(deadline_max);
                mpmc::select! {
                    recv(self.cancel_rx) -> _ => {
                        if let Err(err) = result_tx.send(false) {
                            log::warn!("failed to send quiescence wait result: {err}");
                        }
                        break;
                    },
                    recv(self.activity_rx) -> bump => {
                        match bump {
                            Ok(bump) => {
                                deadline_idle = (Instant::now() + bump).min(deadline_max);
                            }
                            Err(mpmc::RecvError) => {
                                // Channel is empty and disconnected.
                                log::info!("disconnected");
                                break;
                            }
                        }
                    },
                    default(deadline_next.duration_since(Instant::now())) => {
                        if let Err(err) = result_tx.send(true) {
                            log::warn!("failed to send quiescence wait result: {err}");
                        }
                        break;
                    },
                }
            }
        });

        result_rx
    }
}

#[cfg(test)]
mod tests {
    use crate::browser::activity::ActivityStream;

    use super::*;
    use std::time::Instant;

    fn init() {
        let env = env_logger::Env::default().default_filter_or("debug");
        env_logger::Builder::from_env(env)
            .format_timestamp_millis()
            .is_test(true)
            .try_init()
            .ok();
    }

    pub fn start_immediately(
        timeout_idle: Duration,
        timeout_max: Duration,
        activity_rx: mpmc::Receiver<Duration>,
    ) -> (QuiescenceTimer, mpmc::Receiver<bool>) {
        subscribe(activity_rx).start(timeout_idle, timeout_max)
    }

    #[test]
    fn fires_after_timeout_idle_with_no_activity() {
        init();
        let (_empty_tx, empty_rx) = mpmc::unbounded();
        let (_timer, wait) = start_immediately(
            Duration::from_millis(100),
            Duration::from_secs(5),
            empty_rx,
        );
        let t = Instant::now();
        assert!(wait.recv().unwrap());
        let elapsed = t.elapsed();
        assert!(elapsed >= Duration::from_millis(80));
        assert!(elapsed < Duration::from_millis(500));
    }

    #[test]
    fn stream_activity_extends_idle() {
        init();
        let bump = Duration::from_millis(150);
        let (_activity_tx, activity_rx) = unfold(0u32, move |i| {
            if i < 5 {
                thread::sleep(Duration::from_millis(80));
                Some((bump, i + 1))
            } else {
                None
            }
        });

        let (_timer, wait) = start_immediately(
            Duration::from_millis(150),
            Duration::from_secs(5),
            activity_rx,
        );
        let t = Instant::now();
        assert!(wait.recv().unwrap());
        let elapsed = t.elapsed();
        assert!(elapsed >= Duration::from_millis(400));
    }

    #[test]
    fn timeout_max_caps_wait() {
        init();
        let bump = Duration::from_millis(100);
        let (_activity_tx, activity_rx) = unfold((), move |()| {
            thread::sleep(Duration::from_millis(20));
            Some((bump, ()))
        });

        let (_timer, wait) = start_immediately(
            Duration::from_millis(100),
            Duration::from_millis(300),
            activity_rx,
        );
        let t = Instant::now();
        assert!(wait.recv().unwrap());
        let elapsed = t.elapsed();
        assert!(elapsed >= Duration::from_millis(250));
        assert!(elapsed < Duration::from_millis(600));
    }

    #[test]
    fn drop_handle_cancels() {
        init();
        let (_empty_tx, empty_rx) = mpmc::unbounded();
        let (timer, wait) = start_immediately(
            Duration::from_secs(10),
            Duration::from_secs(10),
            empty_rx,
        );
        drop(timer);
        let t = Instant::now();
        assert!(!wait.recv().unwrap());
        assert!(t.elapsed() < Duration::from_millis(100));
    }

    fn unfold<T: Send + 'static>(
        initial: T,
        mut f: impl FnMut(T) -> Option<(Duration, T)> + Send + 'static,
    ) -> (mpmc::Sender<Duration>, ActivityStream) {
        let (tx, rx) = mpmc::unbounded();
        {
            let tx = tx.clone();
            let _ = thread::spawn(move || {
                let mut current = initial;
                while let Some((bump, next)) = f(current) {
                    if let Err(mpmc::SendError(_)) = tx.send(bump) {
                        // Channel is disconnected.
                        break;
                    }
                    current = next;
                }
            });
        }

        (tx, rx)
    }
}
