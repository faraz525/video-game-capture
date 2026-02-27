use crate::capture::CapturedFrame;
use crate::sync::ring_buffer::Timestamped;
use std::collections::VecDeque;

/// Type of an encoded chunk from the streaming encoder.
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkType {
    /// ftyp + moov initialization segment (written once at start)
    InitSegment,
    /// moof + mdat media segment (one per fragment)
    MediaSegment,
}

/// A chunk of encoded video data produced by the streaming encoder.
#[derive(Debug, Clone)]
pub struct EncodedChunk {
    /// Timestamp in microseconds (from SyncClock epoch)
    pub timestamp_us: u64,
    /// Encoded bytes (ftyp+moov for init, moof+mdat for media)
    pub data: Vec<u8>,
    /// Whether this is an init or media segment
    pub chunk_type: ChunkType,
}

impl Timestamped for EncodedChunk {
    fn timestamp_us(&self) -> u64 {
        self.timestamp_us
    }
}

/// Ring buffer for encoded video chunks produced by streaming encoding.
///
/// Stores a single init segment (ftyp+moov) and time-bounded media segments
/// (moof+mdat). On drain, prepends init segment to produce a valid
/// fragmented MP4 stream. Also caches the first raw frame for thumbnail
/// generation.
pub struct EncodedRingBuffer {
    init_segment: Option<Vec<u8>>,
    chunks: VecDeque<EncodedChunk>,
    max_duration_us: u64,
    fragment_duration_us: u64,
    first_raw_frame: Option<CapturedFrame>,
}

impl EncodedRingBuffer {
    /// Create a new buffer that retains at most `max_duration_us` of media chunks.
    ///
    /// `fragment_duration_us` is the duration of each encoded fragment (typically
    /// `GOP_MULTIPLIER * 1_000_000` seconds). Used by `time_span_us()` to compute
    /// the correct end time of the last chunk.
    pub fn new(max_duration_us: u64, fragment_duration_us: u64) -> Self {
        Self {
            init_segment: None,
            chunks: VecDeque::new(),
            max_duration_us,
            fragment_duration_us,
            first_raw_frame: None,
        }
    }

    /// Push an encoded chunk into the buffer.
    ///
    /// Init segments are stored separately (only the latest is kept).
    /// Media segments are added to the ring buffer with time-based eviction.
    pub fn push(&mut self, chunk: EncodedChunk) {
        match chunk.chunk_type {
            ChunkType::InitSegment => {
                self.init_segment = Some(chunk.data);
            }
            ChunkType::MediaSegment => {
                let cutoff = chunk.timestamp_us.saturating_sub(self.max_duration_us);
                while let Some(front) = self.chunks.front() {
                    if front.timestamp_us < cutoff {
                        self.chunks.pop_front();
                    } else {
                        break;
                    }
                }
                self.chunks.push_back(chunk);
            }
        }
    }

    /// Cache the first raw frame for thumbnail generation.
    ///
    /// Only stores the frame if none has been cached yet.
    pub fn cache_first_frame(&mut self, frame: CapturedFrame) {
        if self.first_raw_frame.is_none() {
            self.first_raw_frame = Some(frame);
        }
    }

    /// Take the cached first raw frame, consuming it.
    pub fn take_first_frame(&mut self) -> Option<CapturedFrame> {
        self.first_raw_frame.take()
    }

    /// Returns the time span of media chunks as `(first_ts, end_ts)` in microseconds.
    ///
    /// `end_ts` includes the duration of the last fragment, so the returned span
    /// represents the full playback window. Returns `None` if there are no media chunks.
    pub fn time_span_us(&self) -> Option<(u64, u64)> {
        let first = self.chunks.front()?.timestamp_us;
        let last = self.chunks.back()?.timestamp_us;
        Some((first, last.saturating_add(self.fragment_duration_us)))
    }

    /// Drain the buffer as a valid fragmented MP4 byte stream.
    ///
    /// Returns init_segment + all media chunks concatenated. The buffer
    /// is emptied after this call. Returns empty `Vec` if no init segment
    /// has been received (media-only bytes are invalid fMP4).
    pub fn drain_as_fmp4(&mut self) -> Vec<u8> {
        // Guard: need both init segment and media chunks for valid fMP4
        if self.chunks.is_empty() || self.init_segment.is_none() {
            self.chunks.clear();
            return Vec::new();
        }

        let mut out = Vec::new();

        if let Some(ref init) = self.init_segment {
            out.extend_from_slice(init);
        }

        for chunk in self.chunks.drain(..) {
            out.extend_from_slice(&chunk.data);
        }

        out
    }

    /// Clear all buffered data (init segment, media chunks, and cached frame).
    ///
    /// Used when the streaming encoder dies mid-session to discard stale chunks
    /// that would otherwise be paired with newer input events from a different
    /// time window.
    pub fn clear(&mut self) {
        self.init_segment = None;
        self.chunks.clear();
        self.first_raw_frame = None;
    }

    /// Returns the number of media chunks in the buffer.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Returns true if there are no media chunks.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Returns true if an init segment has been received.
    #[allow(dead_code)]
    pub fn has_init_segment(&self) -> bool {
        self.init_segment.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_init_chunk() -> EncodedChunk {
        EncodedChunk {
            timestamp_us: 0,
            data: vec![0x00, 0x00, 0x00, 0x1C, b'f', b't', b'y', b'p'], // fake ftyp
            chunk_type: ChunkType::InitSegment,
        }
    }

    fn make_media_chunk(ts: u64) -> EncodedChunk {
        EncodedChunk {
            timestamp_us: ts,
            data: vec![0x00, 0x00, 0x00, 0x10, b'm', b'o', b'o', b'f'], // fake moof
            chunk_type: ChunkType::MediaSegment,
        }
    }

    fn make_raw_frame(ts: u64) -> CapturedFrame {
        CapturedFrame {
            timestamp_us: ts,
            width: 4,
            height: 4,
            data: vec![255, 0, 0, 255].repeat(16),
            pixel_format: crate::capture::FramePixelFormat::Rgba,
        }
    }

    // T1: drain empty returns empty vec
    #[test]
    fn drain_empty_returns_empty_vec() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        let result = buf.drain_as_fmp4();
        assert!(result.is_empty());
    }

    // T2: init segment prepended to drain output
    #[test]
    fn init_segment_prepended_to_drain_output() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        let init = make_init_chunk();
        let init_data = init.data.clone();
        buf.push(init);

        let media = make_media_chunk(1_000_000);
        let media_data = media.data.clone();
        buf.push(media);

        let result = buf.drain_as_fmp4();
        assert!(result.starts_with(&init_data));
        assert_eq!(result.len(), init_data.len() + media_data.len());
    }

    // T3: eviction preserves init segment, removes old media
    #[test]
    fn eviction_preserves_init_segment_removes_old_media() {
        let mut buf = EncodedRingBuffer::new(1_000_000, 2_000_000); // 1 second

        buf.push(make_init_chunk());
        buf.push(make_media_chunk(0));
        buf.push(make_media_chunk(500_000));

        // Push chunk at 1.5s — chunk at 0us should be evicted
        buf.push(make_media_chunk(1_500_000));
        assert_eq!(buf.len(), 2); // 500k and 1.5M remain

        // Init segment should still be present
        assert!(buf.has_init_segment());

        let result = buf.drain_as_fmp4();
        // Should start with init segment
        assert!(result.starts_with(&[0x00, 0x00, 0x00, 0x1C, b'f', b't', b'y', b'p']));
    }

    // T4: cache/take first frame roundtrip
    #[test]
    fn cache_take_first_frame_roundtrip() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        let frame = make_raw_frame(1000);
        buf.cache_first_frame(frame.clone());

        let taken = buf.take_first_frame();
        assert!(taken.is_some());
        let taken = taken.unwrap();
        assert_eq!(taken.timestamp_us, 1000);
        assert_eq!(taken.width, 4);
        assert_eq!(taken.height, 4);
    }

    // T5: take first frame before cache returns None
    #[test]
    fn take_first_frame_before_cache_returns_none() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        assert!(buf.take_first_frame().is_none());
    }

    // T6 (from streaming tests): EncodedChunk implements Timestamped correctly
    #[test]
    fn encoded_chunk_implements_timestamped() {
        let chunk = EncodedChunk {
            timestamp_us: 42_000,
            data: vec![1, 2, 3],
            chunk_type: ChunkType::MediaSegment,
        };
        assert_eq!(Timestamped::timestamp_us(&chunk), 42_000);
    }

    // Additional: cache_first_frame only stores first call
    #[test]
    fn cache_first_frame_only_stores_first() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        buf.cache_first_frame(make_raw_frame(1000));
        buf.cache_first_frame(make_raw_frame(2000)); // should be ignored

        let taken = buf.take_first_frame().unwrap();
        assert_eq!(taken.timestamp_us, 1000);
    }

    // time_span_us returns None when empty
    #[test]
    fn time_span_empty_returns_none() {
        let buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        assert!(buf.time_span_us().is_none());
    }

    // time_span_us returns first timestamp and end of last fragment
    #[test]
    fn time_span_returns_first_and_end_of_last() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        buf.push(make_init_chunk());
        buf.push(make_media_chunk(1_000_000));
        buf.push(make_media_chunk(5_000_000));
        buf.push(make_media_chunk(10_000_000));

        let (first, end) = buf.time_span_us().unwrap();
        assert_eq!(first, 1_000_000);
        // end = last_chunk_ts (10M) + fragment_duration (2M)
        assert_eq!(end, 12_000_000);
    }

    // time_span_us with single chunk returns start and end of that fragment
    #[test]
    fn time_span_single_chunk() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        buf.push(make_media_chunk(3_000_000));

        let (first, end) = buf.time_span_us().unwrap();
        assert_eq!(first, 3_000_000);
        // end = chunk_ts (3M) + fragment_duration (2M)
        assert_eq!(end, 5_000_000);
    }

    // time_span_us with 6 chunks at 2s intervals gives 12s total (not 10s)
    #[test]
    fn time_span_includes_last_fragment_duration() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        buf.push(make_init_chunk());
        for i in 0..6 {
            buf.push(make_media_chunk(i * 2_000_000));
        }

        let (first, end) = buf.time_span_us().unwrap();
        assert_eq!(first, 0);
        // 6 chunks: 0, 2M, 4M, 6M, 8M, 10M. Last ends at 10M + 2M = 12M
        assert_eq!(end, 12_000_000);
        assert_eq!(end - first, 12_000_000); // 12 seconds total
    }

    // clear resets all state
    #[test]
    fn clear_resets_all_state() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        buf.push(make_init_chunk());
        buf.push(make_media_chunk(1_000_000));
        buf.push(make_media_chunk(2_000_000));
        buf.cache_first_frame(make_raw_frame(1000));

        buf.clear();

        assert!(buf.is_empty());
        assert!(!buf.has_init_segment());
        assert!(buf.take_first_frame().is_none());
        assert!(buf.time_span_us().is_none());
    }

    // drain returns empty when no init segment (media-only is invalid fMP4)
    #[test]
    fn drain_returns_empty_when_no_init_segment() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        buf.push(make_media_chunk(1_000_000));
        buf.push(make_media_chunk(2_000_000));

        let result = buf.drain_as_fmp4();
        assert!(result.is_empty());
        // Chunks should be cleared even though we returned empty
        assert!(buf.is_empty());
    }

    // Additional: drain clears media chunks but preserves init segment ref
    #[test]
    fn drain_clears_media_chunks() {
        let mut buf = EncodedRingBuffer::new(30_000_000, 2_000_000);
        buf.push(make_init_chunk());
        buf.push(make_media_chunk(1_000_000));
        buf.push(make_media_chunk(2_000_000));

        let _ = buf.drain_as_fmp4();
        assert!(buf.is_empty());
        // Init segment is still referenced (not cleared)
        assert!(buf.has_init_segment());
    }
}
