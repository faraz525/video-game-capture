use std::collections::VecDeque;

/// A time-bounded ring buffer that holds items with microsecond timestamps.
///
/// Automatically evicts items older than `max_duration_us` when new items
/// are pushed. Items are stored in order of insertion.
pub struct RingBuffer<T: Timestamped> {
    items: VecDeque<T>,
    max_duration_us: u64,
}

/// Trait for items stored in the ring buffer. Each item must have a timestamp.
pub trait Timestamped {
    fn timestamp_us(&self) -> u64;
}

impl<T: Timestamped> RingBuffer<T> {
    /// Create a new ring buffer that holds at most `max_duration_us` microseconds of data.
    pub fn new(max_duration_us: u64) -> Self {
        Self {
            items: VecDeque::new(),
            max_duration_us,
        }
    }

    /// Push an item into the buffer. Evicts any items older than max_duration_us
    /// relative to the new item's timestamp.
    pub fn push(&mut self, item: T) {
        let cutoff = item.timestamp_us().saturating_sub(self.max_duration_us);
        while let Some(front) = self.items.front() {
            if front.timestamp_us() < cutoff {
                self.items.pop_front();
            } else {
                break;
            }
        }
        self.items.push_back(item);
    }

    /// Drain all items from the buffer, returning them in chronological order.
    pub fn drain(&mut self) -> Vec<T> {
        self.items.drain(..).collect()
    }

    /// Returns the number of items currently in the buffer.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns a reference to all items in the buffer.
    pub fn items(&self) -> &VecDeque<T> {
        &self.items
    }

    /// Clear all items from the buffer.
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct TestItem {
        ts: u64,
        value: i32,
    }

    impl Timestamped for TestItem {
        fn timestamp_us(&self) -> u64 {
            self.ts
        }
    }

    fn item(ts: u64, value: i32) -> TestItem {
        TestItem { ts, value }
    }

    #[test]
    fn new_buffer_is_empty() {
        let buf: RingBuffer<TestItem> = RingBuffer::new(1_000_000);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn push_and_read() {
        let mut buf = RingBuffer::new(1_000_000);
        buf.push(item(100, 1));
        buf.push(item(200, 2));

        assert_eq!(buf.len(), 2);
        assert_eq!(buf.items()[0].value, 1);
        assert_eq!(buf.items()[1].value, 2);
    }

    #[test]
    fn drain_returns_all_items() {
        let mut buf = RingBuffer::new(1_000_000);
        buf.push(item(100, 1));
        buf.push(item(200, 2));
        buf.push(item(300, 3));

        let drained = buf.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].value, 1);
        assert_eq!(drained[2].value, 3);
        assert!(buf.is_empty());
    }

    #[test]
    fn evicts_old_items() {
        // Buffer holds 1 second (1,000,000 us)
        let mut buf = RingBuffer::new(1_000_000);

        buf.push(item(0, 1));
        buf.push(item(500_000, 2));
        buf.push(item(1_000_000, 3));

        // All items within 1s window, nothing evicted
        assert_eq!(buf.len(), 3);

        // Push item at 1.5s — item at 0us is now >1s old, should be evicted
        buf.push(item(1_500_000, 4));
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.items()[0].value, 2);

        // Push item at 2.5s — items at 500k and 1M should be evicted
        buf.push(item(2_500_000, 5));
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.items()[0].value, 4);
        assert_eq!(buf.items()[1].value, 5);
    }

    #[test]
    fn clear_empties_buffer() {
        let mut buf = RingBuffer::new(1_000_000);
        buf.push(item(100, 1));
        buf.push(item(200, 2));

        buf.clear();
        assert!(buf.is_empty());
    }

    #[test]
    fn handles_zero_duration() {
        let mut buf = RingBuffer::new(0);
        buf.push(item(100, 1));
        // With 0 duration, each push should evict everything before current timestamp
        assert_eq!(buf.len(), 1);

        buf.push(item(200, 2));
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.items()[0].value, 2);
    }

    #[test]
    fn handles_same_timestamp() {
        let mut buf = RingBuffer::new(1_000_000);
        buf.push(item(100, 1));
        buf.push(item(100, 2));
        buf.push(item(100, 3));

        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn large_buffer_scenario() {
        // 30 seconds at "60fps" = 1800 items
        let mut buf = RingBuffer::new(30_000_000); // 30 seconds
        let frame_interval = 16_667; // ~60fps

        for i in 0..3600 {
            // 60 seconds of frames
            buf.push(item(i * frame_interval, i as i32));
        }

        // Should only keep ~30 seconds worth = ~1800 frames
        let len = buf.len();
        assert!(
            len >= 1799 && len <= 1801,
            "expected ~1800 items, got {len}"
        );

        // Oldest item should be from ~30s mark
        let oldest = buf.items().front().unwrap();
        let newest = buf.items().back().unwrap();
        let duration = newest.ts - oldest.ts;
        assert!(
            duration <= 30_000_000,
            "buffer should span <=30s, spans {duration}us"
        );
    }
}
