use crossbeam_channel::{Receiver, Sender, bounded};

/// Silly implementation of a semaphore only for test coordination.
pub struct Semaphore {
    tx: Sender<()>,
    rx: Receiver<()>,
}

impl Semaphore {
    pub fn new(permits: usize) -> Self {
        let (tx, rx) = bounded(permits);
        for _ in 0..permits {
            tx.send(()).unwrap();
        }
        Self { tx, rx }
    }

    pub fn acquire(&self) -> SemaphoreGuard<'_> {
        self.rx.recv().unwrap();
        SemaphoreGuard { semaphore: self }
    }
}

pub struct SemaphoreGuard<'a> {
    semaphore: &'a Semaphore,
}

impl Drop for SemaphoreGuard<'_> {
    fn drop(&mut self) {
        let _ = self.semaphore.tx.try_send(());
    }
}
