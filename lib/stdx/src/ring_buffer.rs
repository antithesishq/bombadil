use std::mem::MaybeUninit;

pub struct RingBuffer<T, const N: usize> {
    data: [MaybeUninit<T>; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<T, const N: usize> RingBuffer<T, N> {
    pub fn new() -> Self {
        Self {
            data: std::array::from_fn(|_| MaybeUninit::uninit()),
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, value: T) -> Option<T> {
        let evicted = if self.len == N {
            let evicted = unsafe { self.data[self.head].assume_init_read() };
            self.head = (self.head + 1) % N;
            Some(evicted)
        } else {
            self.len += 1;
            None
        };
        self.data[self.tail].write(value);
        self.tail = (self.tail + 1) % N;
        evicted
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let value = unsafe { self.data[self.head].assume_init_read() };
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(value)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn first(&self) -> Option<&T> {
        if self.len > 0 {
            Some(unsafe { self.data[self.head].assume_init_ref() })
        } else {
            None
        }
    }
    pub fn last(&self) -> Option<&T> {
        if self.len > 0 {
            let index = if self.tail == 0 { N - 1 } else { self.tail - 1 };
            Some(unsafe { self.data[index].assume_init_ref() })
        } else {
            None
        }
    }
}

struct DebugValues<'a, T, const N: usize>(&'a RingBuffer<T, N>);

impl<'a, T: std::fmt::Debug, const N: usize> std::fmt::Debug
    for DebugValues<'a, T, N>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl<T: std::fmt::Debug, const N: usize> std::fmt::Debug for RingBuffer<T, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RingBuffer")
            .field("len", &self.len)
            .field("values", &DebugValues(self))
            .finish()
    }
}

impl<T, const N: usize> Default for RingBuffer<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Drop for RingBuffer<T, N> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

pub struct Iter<'a, T, const N: usize> {
    buffer: &'a RingBuffer<T, N>,
    current: usize,
    remaining: usize,
}

impl<T, const N: usize> RingBuffer<T, N> {
    pub fn iter(&self) -> Iter<'_, T, N> {
        Iter {
            buffer: self,
            current: 0,
            remaining: self.len,
        }
    }
}

impl<'a, T, const N: usize> Iterator for Iter<'a, T, N> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.remaining == 0 {
            return None;
        }
        let index = (self.buffer.head + self.current) % N;
        self.current += 1;
        self.remaining -= 1;
        Some(unsafe { self.buffer.data[index].assume_init_ref() })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T, const N: usize> ExactSizeIterator for Iter<'_, T, N> {}

impl<'a, T, const N: usize> IntoIterator for &'a RingBuffer<T, N> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T, N>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use hegel::TestCase;
    use hegel::generators as gs;
    use std::collections::VecDeque;

    use super::RingBuffer;

    const CAP: usize = 8;

    struct RingVsVecDeque {
        ring: RingBuffer<i32, CAP>,
        model: VecDeque<i32>,
    }

    #[hegel::state_machine]
    impl RingVsVecDeque {
        #[rule]
        fn push(&mut self, tc: TestCase) {
            let value = tc.draw(gs::integers::<i32>());

            if self.model.len() < CAP {
                let result = self.ring.push(value);
                assert!(
                    result.is_none(),
                    "ring evicted a value when the model had room for more"
                );
                self.model.push_back(value);
            } else {
                assert_eq!(self.ring.push(value), self.model.pop_front());
                self.model.push_back(value);
            }
        }

        #[rule]
        fn pop(&mut self, _: TestCase) {
            assert_eq!(self.ring.pop(), self.model.pop_front());
        }

        #[invariant]
        fn agrees_with_model(&mut self, _: TestCase) {
            assert_eq!(self.ring.len(), self.model.len());
            assert_eq!(
                self.ring.iter().copied().collect::<Vec<_>>(),
                self.model.iter().copied().collect::<Vec<_>>(),
            );
            assert_eq!(self.ring.first(), self.model.front());
            assert_eq!(self.ring.last(), self.model.back());
        }

        #[invariant(always_run)]
        fn never_exceeds_capacity(&mut self, _: TestCase) {
            assert!(self.model.len() <= CAP);
        }
    }

    #[hegel::test(test_cases = 1000)]
    fn ring_buffer_matches_vecdeque(tc: TestCase) {
        let sut = RingVsVecDeque {
            ring: RingBuffer::new(),
            model: VecDeque::new(),
        };
        hegel::stateful::machine(sut).run(tc);
    }

    #[hegel::test(test_cases = 1000)]
    fn drains_to_empty_in_lockstep(tc: TestCase) {
        let ops = tc.draw(gs::vecs(gs::booleans()).max_size(50));
        let mut ring: RingBuffer<i32, CAP> = RingBuffer::new();
        let mut model: VecDeque<i32> = VecDeque::new();

        for is_push in ops {
            if is_push {
                let v = tc.draw(gs::integers::<i32>());
                if model.len() < CAP {
                    assert!(ring.push(v).is_none());
                    model.push_back(v);
                } else {
                    assert_eq!(ring.push(v), model.pop_front());
                    model.push_back(v);
                }
            } else {
                assert_eq!(ring.pop(), model.pop_front());
            }
        }

        loop {
            let a = ring.pop();
            let b = model.pop_front();
            assert_eq!(a, b);
            if a.is_none() {
                break;
            }
        }
    }
}
