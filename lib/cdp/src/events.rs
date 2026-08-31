use anyhow::Result;
use crossbeam_channel as mpmc;
use serde::de::DeserializeOwned;
use serde_json as json;
use std::marker::PhantomData;

use cdp_types::{CdpJsonEventMessage, MethodId, MethodType};

#[derive(Debug)]
pub struct Events {
    pub(crate) receiver: mpmc::Receiver<CdpJsonEventMessage>,
}

impl Events {
    pub fn all(&self) -> mpmc::Receiver<CdpJsonEventMessage> {
        self.receiver.clone()
    }

    // Creates a cheap typed subscriber which can be used to iterate
    // over particular events.
    pub fn subscribe<T: MethodType + DeserializeOwned>(&self) -> Subscriber<T> {
        Subscriber {
            _phantom: PhantomData::<T>,
            method_id: T::method_id(),
            receiver: self.receiver.clone(),
        }
    }

    // Subscribes to typed events by creating a new channel. This enables
    // crossbeam `select!`, at the cost of spawning a new OS thread.
    pub fn subscribe_cloned<
        T: MethodType + DeserializeOwned + Send + 'static,
    >(
        &self,
    ) -> mpmc::Receiver<T> {
        let (tx, rx) = mpmc::bounded(1024);
        let events = self.receiver.clone();
        let _ = std::thread::spawn(move || -> Result<()> {
            loop {
                let message = events.recv()?;
                if message.method == T::method_id() {
                    tx.send(json::from_value(message.params)?)
                        .expect("failed to forward typed event");
                }
            }
        });
        rx
    }
}

#[derive(Debug)]
pub struct Subscriber<T: DeserializeOwned> {
    _phantom: PhantomData<T>,
    receiver: mpmc::Receiver<CdpJsonEventMessage>,
    method_id: MethodId,
}

impl<T: DeserializeOwned> Subscriber<T> {
    pub fn next(&self) -> Result<T> {
        loop {
            let message = self.receiver.recv()?;
            if message.method == self.method_id {
                return Ok(json::from_value(message.params)?);
            }
        }
    }
}
