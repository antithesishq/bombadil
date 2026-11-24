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

/// How many unread events a subscription can hold before we give up.
const EVENTS_PENDING_MAX: usize = 1 << 16;

#[derive(Debug, Clone)]
pub(crate) struct Subscribers {
    closed: bool,
    close_error: Option<String>,
    single: HashMap<MethodId, Vec<mpmc::Sender<Arc<CdpJsonEventMessage>>>>,
    events_pending_max: usize,
}

impl Default for Subscribers {
    fn default() -> Self {
        Self {
            closed: false,
            close_error: None,
            single: HashMap::new(),
            events_pending_max: EVENTS_PENDING_MAX,
        }
    }
}

impl Subscribers {
    pub(crate) fn is_interested_in(&self, method: &str) -> bool {
        !self.closed && self.single.contains_key(method)
    }

    #[hotpath::measure]
    pub(crate) fn dispatch(&mut self, event: CdpJsonEventMessage) {
        assert!(!self.closed, "Subscribers are closed, can't dispatch");
        let event = Arc::new(event);

        // Never block on a slow subscriber: the WebSocket worker must keep
        // reading command responses. A full subscription closes them all.
        let mut overflowed = false;
        if let Some(subscriptions) = self.single.get_mut(&event.method) {
            subscriptions.retain(|s| match s.try_send(event.clone()) {
                Ok(()) => true,
                Err(mpmc::TrySendError::Full(_)) => {
                    overflowed = true;
                    true
                }
                Err(mpmc::TrySendError::Disconnected(_)) => false,
            });
            if subscriptions.is_empty() {
                self.single.remove(&event.method);
            }
        }
        if overflowed {
            let error = format!(
                "more than {} unprocessed CDP events: the page likely emits \
                 events faster than they can be processed",
                self.events_pending_max
            );
            log::error!("{error}");
            self.close(Some(error));
        }
    }

    pub(crate) fn close(&mut self, error: Option<String>) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.close_error = error;
        self.single.clear();
    }
}

impl Events {
    /// Receive only the selected methods, preserving their arrival order.
    pub fn methods(
        &self,
        methods: impl IntoIterator<Item = MethodId>,
    ) -> mpmc::Receiver<Arc<CdpJsonEventMessage>> {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("failed to acquire lock for subscribers");
        let (tx, rx) = mpmc::bounded(subscribers.events_pending_max);
        if !subscribers.closed {
            let mut unique = std::collections::HashSet::new();
            for method in methods {
                if unique.insert(method.clone()) {
                    subscribers
                        .single
                        .entry(method)
                        .or_default()
                        .push(tx.clone());
                }
            }
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
        let (tx, rx) = mpmc::bounded(subscribers.events_pending_max);
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

    use hegel::{
        TestCase,
        generators::{booleans, integers},
    };
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
        let receiver = events.methods([TestEvent::method_id()]);
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

    fn subscribers_with_max(
        events_pending_max: usize,
    ) -> (Arc<Mutex<Subscribers>>, Events) {
        let subscribers = Arc::new(Mutex::new(Subscribers {
            events_pending_max,
            ..Subscribers::default()
        }));
        let events = Events {
            subscribers: subscribers.clone(),
        };
        (subscribers, events)
    }

    #[hegel::test]
    fn full_subscription_closes_all(tc: TestCase) {
        let events_pending_max =
            tc.draw(integers::<usize>().min_value(1).max_value(8));
        let (subscribers, events) = subscribers_with_max(events_pending_max);
        let drained = events.methods([TestEvent::method_id()]);
        let slow = events.methods([TestEvent::method_id()]);
        let mut pending = 0;

        for _ in 0..tc.draw(integers::<usize>().max_value(64)) {
            if tc.draw(booleans()) {
                subscribers.lock().unwrap().dispatch(event());
                drained.try_recv().unwrap();
                if pending == events_pending_max {
                    let error = events.close_error().unwrap();
                    assert!(error.contains("faster than"), "{error}");
                    assert!(matches!(
                        drained.try_recv(),
                        Err(mpmc::TryRecvError::Disconnected)
                    ));
                    return;
                }
                pending += 1;
            } else if pending > 0 {
                slow.try_recv().unwrap();
                pending -= 1;
            }
            assert_eq!(events.close_error(), None);
            assert_eq!(slow.len(), pending);
        }
    }

    #[test]
    fn closed_full_subscription_keeps_buffered_events() {
        let (subscribers, events) = subscribers_with_max(2);
        let receiver = events.methods([TestEvent::method_id()]);
        for _ in 0..3 {
            subscribers.lock().unwrap().dispatch(event());
        }

        assert_eq!(receiver.try_iter().count(), 2);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpmc::TryRecvError::Disconnected)
        ));
        assert!(
            !subscribers
                .lock()
                .unwrap()
                .is_interested_in(&TestEvent::method_id())
        );
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
            events.methods([TestEvent::method_id()]).try_recv(),
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
