use anyhow::{Result, anyhow};
use crossbeam_channel as mpmc;
use serde::de::DeserializeOwned;
use std::{
    collections::HashMap,
    marker::PhantomData,
    sync::{Arc, Mutex},
};

use cdp_types::{CdpJsonEventMessage, MethodId, MethodType};

#[derive(Debug, Clone)]
pub struct Events {
    pub(crate) subscribers: Arc<Mutex<Subscribers>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Subscribers {
    pub(crate) closed: bool,
    pub(crate) close_error: Option<String>,
    pub(crate) all: Vec<mpmc::Sender<Arc<CdpJsonEventMessage>>>,
    pub(crate) single:
        HashMap<MethodId, Vec<mpmc::Sender<Arc<CdpJsonEventMessage>>>>,
}

impl Subscribers {
    #[hotpath::measure]
    pub(crate) fn dispatch(&mut self, event: CdpJsonEventMessage) {
        assert!(!self.closed, "Subscribers are closed, can't dispatch");
        let event = Arc::new(event);

        // These channels are unbounded, so a slow subscriber cannot block the
        // WebSocket worker that must read command responses.
        self.all.retain(|s| s.send(event.clone()).is_ok());
        if let Some(subscriptions) = self.single.get_mut(&event.method) {
            subscriptions.retain(|s| s.send(event.clone()).is_ok());
            if subscriptions.is_empty() {
                self.single.remove(&event.method);
            }
        }
    }

    pub(crate) fn close(&mut self, error: Option<String>) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.close_error = error;
        self.all.clear();
        self.single.clear();
    }
}

impl Events {
    pub fn all(&self) -> mpmc::Receiver<Arc<CdpJsonEventMessage>> {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("failed to acquire lock for subscribers");
        let (tx, rx) = mpmc::unbounded();
        if !subscribers.closed {
            subscribers.all.push(tx);
        }
        rx
    }

    // Creates a cheap typed subscriber which can be used to iterate
    // over particular events.
    pub fn subscribe<T: MethodType + DeserializeOwned>(&self) -> Subscriber<T> {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("failed to acquire lock for subscribers");
        let (tx, rx) = mpmc::unbounded();
        if !subscribers.closed {
            subscribers
                .single
                .entry(T::method_id())
                .or_default()
                .push(tx);
        }
        Subscriber {
            _phantom: PhantomData::<T>,
            rx,
            subscribers: self.subscribers.clone(),
        }
    }

    pub fn close(&self) {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("failed to acquire lock for subscribers close");
        subscribers.close(None);
    }

    pub fn close_error(&self) -> Option<String> {
        self.subscribers
            .lock()
            .expect("failed to acquire lock for subscriber error")
            .close_error
            .clone()
    }
}

#[derive(Debug)]
pub struct Subscriber<T: DeserializeOwned> {
    _phantom: PhantomData<T>,
    rx: mpmc::Receiver<Arc<CdpJsonEventMessage>>,
    subscribers: Arc<Mutex<Subscribers>>,
}

impl<T: MethodType + DeserializeOwned> Subscriber<T> {
    // Return the next even or None if there are no more events.
    #[hotpath::measure]
    pub fn next(&self) -> Result<Option<T>> {
        loop {
            match self.rx.recv() {
                Ok(message) => {
                    if message.method == T::method_id() {
                        return Ok(Some(serde_json::from_str::<T>(
                            message.params.get(),
                        )?));
                    }
                }
                Err(mpmc::RecvError) => {
                    let error = self
                        .subscribers
                        .lock()
                        .expect("failed to acquire lock for subscriber error")
                        .close_error
                        .clone();
                    return error.map_or(Ok(None), |error| Err(anyhow!(error)));
                }
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::time::Duration;

    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct TestEvent;

    impl MethodType for TestEvent {
        fn method_id() -> MethodId {
            Cow::Borrowed("Test.event")
        }
    }

    fn event() -> CdpJsonEventMessage {
        CdpJsonEventMessage {
            method: TestEvent::method_id(),
            session_id: None,
            params: serde_json::value::RawValue::from_string("{}".into())
                .unwrap(),
        }
    }

    #[test]
    fn slow_subscriber_does_not_block_dispatch() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let events = Events {
            subscribers: subscribers.clone(),
        };
        let receiver = events.all();
        let (done_tx, done_rx) = mpmc::bounded(1);

        std::thread::spawn(move || {
            let mut subscribers = subscribers.lock().unwrap();
            for _ in 0..100 {
                subscribers.dispatch(event());
            }
            done_tx.send(()).unwrap();
        });

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("dispatch blocked on a slow subscriber");
        assert_eq!(receiver.len(), 100);
    }

    #[test]
    fn typed_subscriber_reports_worker_error() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let events = Events {
            subscribers: subscribers.clone(),
        };
        let receiver = events.subscribe::<TestEvent>();

        subscribers
            .lock()
            .unwrap()
            .close(Some("worker failed".into()));

        let error = receiver.next().unwrap_err();
        assert_eq!(error.to_string(), "worker failed");
    }

    #[test]
    fn subscribing_after_worker_failure_returns_disconnected_receivers() {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let events = Events { subscribers };
        events
            .subscribers
            .lock()
            .unwrap()
            .close(Some("worker failed".into()));
        assert!(matches!(
            events.all().try_recv(),
            Err(mpmc::TryRecvError::Disconnected)
        ));
        assert_eq!(
            events
                .subscribe::<TestEvent>()
                .next()
                .unwrap_err()
                .to_string(),
            "worker failed"
        );
    }
}
