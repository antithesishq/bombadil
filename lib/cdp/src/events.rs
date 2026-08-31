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
    pub fn next(&self) -> Result<T> {
        loop {
            let message = self.receiver.recv()?;
            if message.method == self.method_id {
                return Ok(json::from_value(message.params)?);
            }
        }
    }
}
