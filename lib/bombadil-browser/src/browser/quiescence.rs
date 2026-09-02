use crossbeam_channel as mpmc;
use std::{
    thread,
    time::{Duration, Instant},
};

/// Start the countdown. Returns channel with a single value
/// when quiescent.
pub fn start(
    activity_rx: mpmc::Receiver<Duration>,
    timeout_idle: Duration,
    timeout_max: Duration,
) -> mpmc::Receiver<()> {
    let (result_tx, result_rx) = mpmc::bounded(1);

    let _ = thread::spawn(move || {
        let deadline_max = Instant::now() + timeout_max;
        let mut deadline_idle = Instant::now() + timeout_idle;
        loop {
            let deadline_next = deadline_idle.min(deadline_max);
            mpmc::select! {
                recv(activity_rx) -> bump => {
                    match bump {
                        Ok(bump) => {
                            log::debug!("quiescence timer bumped by {bump:?}");
                            deadline_idle = (Instant::now() + bump).min(deadline_max);
                        }
                        Err(mpmc::RecvError) => {
                            // Channel is empty and disconnected.
                            log::debug!("quiescence activity channel disconnected");
                            break;
                        }
                    }
                },
                default(deadline_next.duration_since(Instant::now())) => {
                    if let Err(err) = result_tx.send(()) {
                        log::warn!("failed to send quiescence wait result: {err}");
                    } else {
                        log::debug!("quiescence timer fired successfully");
                    }
                    break;
                },
            }
        }
    });

    result_rx
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

    #[test]
    fn fires_after_timeout_idle_with_no_activity() {
        init();
        let (_empty_tx, empty_rx) = mpmc::unbounded();
        let wait =
            start(empty_rx, Duration::from_millis(100), Duration::from_secs(5));
        let t = Instant::now();
        wait.recv().unwrap();
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

        let wait = start(
            activity_rx,
            Duration::from_millis(150),
            Duration::from_secs(5),
        );
        let t = Instant::now();
        wait.recv().unwrap();
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

        let wait = start(
            activity_rx,
            Duration::from_millis(100),
            Duration::from_millis(300),
        );
        let t = Instant::now();
        wait.recv().unwrap();
        let elapsed = t.elapsed();
        assert!(elapsed >= Duration::from_millis(250));
        assert!(elapsed < Duration::from_millis(600));
    }

    #[test]
    fn drop_handle_cancels() {
        init();
        let (_empty_tx, empty_rx) = mpmc::unbounded();
        let wait =
            start(empty_rx, Duration::from_secs(10), Duration::from_secs(10));
        let t = Instant::now();
        wait.recv().unwrap();
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
