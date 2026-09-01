use anyhow::Result;
use crossbeam_channel as mpmc;
use serde::de::DeserializeOwned;
use serde_json as json;
use std::marker::PhantomData;

use cdp_types::{CdpJsonEventMessage, MethodId, MethodType};

#[derive(Debug, Clone)]
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
}

#[derive(Debug)]
pub struct Subscriber<T: DeserializeOwned> {
    _phantom: PhantomData<T>,
    receiver: mpmc::Receiver<CdpJsonEventMessage>,
    method_id: MethodId,
}

impl<T: DeserializeOwned> Subscriber<T> {
    // Return the next even or None if there are no more events.
    pub fn next(&self) -> Result<Option<T>> {
        loop {
            match self.receiver.recv() {
                Ok(message) => {
                    if message.method == self.method_id {
                        return Ok(json::from_value(message.params)?);
                    }
                }
                Err(mpmc::RecvError) => return Ok(None),
            };
        }
    }
}
