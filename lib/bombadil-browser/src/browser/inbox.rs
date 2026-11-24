use crossbeam_channel as mpmc;
use std::sync::{Arc, OnceLock};
use std::thread::{self, ThreadId};
use stdx::ring_buffer::RingBuffer;

const OWN_CAPACITY: usize = 4;

/// An event queue whose consumer can send to itself without blocking. Own
/// events are taken before external ones.
pub struct InboxReceiver<Event> {
    external_rx: mpmc::Receiver<Event>,
    own: RingBuffer<Event, OWN_CAPACITY>,
    consumer: Arc<OnceLock<ThreadId>>,
}

pub struct InboxSender<Event> {
    external_tx: mpmc::Sender<Event>,
    consumer: Arc<OnceLock<ThreadId>>,
}

pub fn inbox<Event>(
    capacity: usize,
) -> (InboxSender<Event>, InboxReceiver<Event>) {
    let (external_tx, external_rx) = mpmc::bounded(capacity);
    let consumer = Arc::new(OnceLock::new());
    (
        InboxSender {
            external_tx,
            consumer: consumer.clone(),
        },
        InboxReceiver {
            external_rx,
            own: RingBuffer::new(),
            consumer,
        },
    )
}

impl<Event> InboxReceiver<Event> {
    pub fn next(&mut self) -> Option<Event> {
        self.consumer.get_or_init(|| thread::current().id());
        self.own.pop().or_else(|| self.external_rx.recv().ok())
    }

    pub fn send_to_self(&mut self, event: Event) {
        assert!(
            self.own.len() < OWN_CAPACITY,
            "too many events sent to self"
        );
        self.own.push(event);
    }
}

impl<Event> InboxSender<Event> {
    /// Panics on the consumer thread, where a full queue would deadlock.
    pub fn send(&self, event: Event) -> Result<(), mpmc::SendError<Event>> {
        assert!(
            self.consumer.get() != Some(&thread::current().id()),
            "the inbox consumer must use InboxReceiver::send_to_self"
        );
        self.external_tx.send(event)
    }
}

impl<Event> Clone for InboxSender<Event> {
    fn clone(&self) -> Self {
        Self {
            external_tx: self.external_tx.clone(),
            consumer: self.consumer.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::{TestCase, generators::integers};
    use std::collections::VecDeque;

    #[hegel::test]
    fn inbox_takes_own_events_first(tc: TestCase) {
        let capacity = tc.draw(integers::<usize>().min_value(1).max_value(4));
        let (sender, mut events_rx) = inbox::<u8>(capacity);
        let mut own_expected = VecDeque::new();
        let mut external_expected = VecDeque::new();

        for _ in 0..tc.draw(integers::<usize>().max_value(64)) {
            match tc.draw(integers::<u8>().max_value(2)) {
                0 if external_expected.len() < capacity => {
                    let event = tc.draw(integers());
                    sender.external_tx.send(event).unwrap();
                    external_expected.push_back(event);
                }
                1 if own_expected.len() < OWN_CAPACITY => {
                    let event = tc.draw(integers());
                    events_rx.send_to_self(event);
                    own_expected.push_back(event);
                }
                _ if !(own_expected.is_empty()
                    && external_expected.is_empty()) =>
                {
                    let expected = own_expected
                        .pop_front()
                        .or_else(|| external_expected.pop_front());
                    assert_eq!(events_rx.next(), expected);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn inbox_sender_delivers_from_other_threads() {
        let (sender, mut events_rx) = inbox::<u8>(1);
        events_rx.send_to_self(1);
        assert_eq!(events_rx.next(), Some(1));
        thread::spawn(move || sender.send(2).unwrap());
        assert_eq!(events_rx.next(), Some(2));
        assert_eq!(events_rx.next(), None);
    }

    #[test]
    #[should_panic(expected = "must use InboxReceiver::send_to_self")]
    fn inbox_sender_panics_on_consumer_thread() {
        let (sender, mut events_rx) = inbox::<u8>(1);
        events_rx.send_to_self(1);
        events_rx.next();
        let _ = sender.send(2);
    }

    #[test]
    #[should_panic(expected = "too many events sent to self")]
    fn inbox_panics_on_too_many_own_events() {
        let (_sender, mut events_rx) = inbox::<u8>(1);
        for event in 0..=OWN_CAPACITY as u8 {
            events_rx.send_to_self(event);
        }
    }
}
