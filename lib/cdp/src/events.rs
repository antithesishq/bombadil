use anyhow::Result;
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
    all: Vec<mpmc::Sender<Arc<CdpJsonEventMessage>>>,
    single: HashMap<MethodId, Vec<mpmc::Sender<Arc<CdpJsonEventMessage>>>>,
}

impl Subscribers {
    #[hotpath::measure]
    pub(crate) fn dispatch(&mut self, event: CdpJsonEventMessage) {
        assert!(!self.closed, "Subscribers are closed, can't dispatch");
        let event = Arc::new(event);

        // Evict disconnected channels while dispatching (`send` returns Err in
        // case of disconnected receiver).
        self.all.retain(|s| s.send(event.clone()).is_ok());
        if let Some(subscriptions) = self.single.get_mut(&event.method) {
            subscriptions.retain(|s| s.send(event.clone()).is_ok());
            if subscriptions.is_empty() {
                self.single.remove(&event.method);
            }
        }
    }

    pub(crate) fn close(&mut self) {
        self.closed = true;
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
        assert!(
            !subscribers.closed,
            "Subscribers are closed, can't subscribe with .all()"
        );
        let (tx, rx) = mpmc::bounded(1024);
        subscribers.all.push(tx);
        rx
    }

    // Creates a cheap typed subscriber which can be used to iterate
    // over particular events.
    pub fn subscribe<T: MethodType + DeserializeOwned>(&self) -> Subscriber<T> {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("failed to acquire lock for subscribers");
        assert!(
            !subscribers.closed,
            "Subscribers are closed, can't subscribe with .subscribe()"
        );
        let (tx, rx) = mpmc::bounded(1024);
        subscribers
            .single
            .entry(T::method_id())
            .or_default()
            .push(tx);
        Subscriber {
            _phantom: PhantomData::<T>,
            rx,
        }
    }

    pub fn close(&self) {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("failed to acquire lock for subscribers close");
        subscribers.close();
    }
}

#[derive(Debug)]
pub struct Subscriber<T: DeserializeOwned> {
    _phantom: PhantomData<T>,
    rx: mpmc::Receiver<Arc<CdpJsonEventMessage>>,
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
                Err(mpmc::RecvError) => return Ok(None),
            };
        }
    }
}
