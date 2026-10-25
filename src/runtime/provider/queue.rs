//! Charge encoded payloads once; callers retain their existing admission/eviction policy.
use std::collections::VecDeque;

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub items: usize,
    pub bytes: usize,
}
pub(crate) struct ByteQueue<T> {
    items: VecDeque<(T, usize)>,
    bytes: usize,
}
impl<T> Default for ByteQueue<T> {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            bytes: 0,
        }
    }
}
impl<T> ByteQueue<T> {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter().map(|(value, _)| value)
    }
    pub fn fits(&self, items: usize, bytes: usize, limit: Limits) -> bool {
        self.len()
            .checked_add(items)
            .is_some_and(|n| n <= limit.items)
            && self
                .bytes
                .checked_add(bytes)
                .is_some_and(|n| n <= limit.bytes)
    }
    /// The caller either checked admission or evicts oldest entries immediately.
    pub fn push_back(&mut self, value: T, bytes: usize) {
        self.items.push_back((value, bytes));
        self.bytes += bytes;
    }
    pub fn try_push(&mut self, value: T, bytes: usize, limit: Limits) -> bool {
        if !self.fits(1, bytes, limit) {
            return false;
        }
        self.push_back(value, bytes);
        true
    }
    /// Batch rejection leaves the existing queue untouched.
    pub fn try_extend(
        &mut self,
        values: impl IntoIterator<Item = (T, usize)>,
        limit: Limits,
    ) -> bool {
        let mut batch = Self::default();
        for (value, bytes) in values {
            if !batch.try_push(value, bytes, limit) || !self.fits(batch.len(), batch.bytes, limit) {
                return false;
            }
        }
        self.append(&mut batch);
        true
    }
    pub fn pop_front(&mut self) -> Option<T> {
        self.items.pop_front().map(|(value, bytes)| {
            self.bytes -= bytes;
            value
        })
    }
    pub fn remove(&mut self, index: usize) -> Option<T> {
        self.items.remove(index).map(|(value, bytes)| {
            self.bytes -= bytes;
            value
        })
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.items.retain(|(value, bytes)| {
            if keep(value) {
                true
            } else {
                self.bytes -= bytes;
                false
            }
        });
    }
    pub fn clear(&mut self) {
        self.items.clear();
        self.bytes = 0;
    }
    /// Merge previously admitted queues while retaining order, including on RPC errors.
    pub fn append(&mut self, other: &mut Self) {
        self.items.append(&mut other.items);
        self.bytes += std::mem::take(&mut other.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_charges_follow_rejection_removal_and_transfer() {
        let limit = Limits {
            items: 3,
            bytes: 10,
        };
        let mut queue = ByteQueue::default();
        assert!(queue.try_extend([(1, 3), (2, 4)], limit));
        assert!(!queue.try_extend([(3, 2), (4, 1)], limit));
        assert!(!queue.try_push(3, 4, limit));
        assert_eq!((queue.len(), queue.bytes()), (2, 7));
        assert_eq!(queue.pop_front(), Some(1));
        assert!(queue.try_push(3, 6, limit));
        assert_eq!(queue.bytes(), 10);
        queue.retain(|value| *value != 2);
        assert_eq!(queue.bytes(), 6);
        let mut tail = ByteQueue::default();
        assert!(tail.try_push(4, 4, limit));
        queue.append(&mut tail);
        assert!(tail.is_empty());
        assert_eq!(tail.bytes(), 0);
        assert_eq!(queue.remove(0), Some(3));
        assert_eq!(queue.bytes(), 4);
        assert!(!queue.fits(usize::MAX, 0, limit));
        assert!(!queue.fits(0, usize::MAX, limit));
        queue.clear();
        assert_eq!((queue.len(), queue.bytes()), (0, 0));
    }
}
