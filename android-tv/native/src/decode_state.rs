//! Compressed-input ownership across the existing bounded queue's resets.
use crate::video_output::{Frame, Queue};

/// MediaCodec accepts complete access units, unlike a stream-oriented decoder.
/// Retain only bounded non-VCL prefixes until the next picture, without adding
/// a frame of lookahead or copying the compressed picture into another Vec.
pub fn append_metadata(prefix: &mut Vec<u8>, bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        prefix.len().saturating_add(bytes.len()) <= 256 * 1024,
        "Video access-unit metadata exceeds limit"
    );
    prefix.extend_from_slice(bytes);
    Ok(())
}

pub fn presentation_time(previous: u64, timestamp_100ns: i64) -> u64 {
    ((timestamp_100ns.max(0) / 10) as u64).max(previous.saturating_add(1))
}

pub fn retire_pending(
    queue: &mut Queue,
    generation: &mut u64,
    pending: &mut Option<Frame>,
) -> bool {
    if *generation == queue.reset_generation {
        return false;
    }
    if let Some(frame) = pending.take() {
        queue.recycle(frame.bytes);
    }
    *generation = queue.reset_generation;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_stream::Codec;
    use std::time::Instant;

    #[test]
    fn metadata_prefix_is_ordered_bounded_and_reusable_without_picture_lookahead() {
        let mut prefix = Vec::new();
        append_metadata(&mut prefix, &[0, 0, 1, 9, 0xf0]).unwrap();
        append_metadata(&mut prefix, &[0, 0, 1, 6, 0x80]).unwrap();
        assert_eq!(prefix, [0, 0, 1, 9, 0xf0, 0, 0, 1, 6, 0x80]);
        assert!(append_metadata(&mut prefix, &vec![0; 256 * 1024]).is_err());
        assert_eq!(prefix.len(), 10);
        let capacity = prefix.capacity();
        prefix.clear();
        append_metadata(&mut prefix, &[0, 0, 1, 9]).unwrap();
        assert_eq!(prefix.capacity(), capacity);
    }

    #[test]
    fn burst_delivery_never_reuses_a_picture_timestamp() {
        assert_eq!(presentation_time(0, 0), 1);
        assert_eq!(presentation_time(1, 0), 2);
        assert_eq!(presentation_time(2, 1000), 100);
        assert_eq!(presentation_time(100, 1000), 101);
        assert_eq!(presentation_time(101, -1), 102);
    }

    #[test]
    fn a_pending_input_slot_cannot_keep_a_retired_reference_picture() {
        let mut queue = Queue::default();
        let mut generation = queue.reset_generation;
        assert!(queue.submit_codec(vec![1], true, false, 1, Codec::H264));
        let mut pending = queue.pop();
        assert!(!retire_pending(&mut queue, &mut generation, &mut pending));
        assert!(pending.is_some());
        queue.recover(Instant::now());
        assert!(queue.submit_codec(vec![2], true, false, 2, Codec::H264));
        assert!(retire_pending(&mut queue, &mut generation, &mut pending));
        assert!(pending.is_none());
        assert_eq!(queue.pop().unwrap().bytes, vec![2]);
    }

    #[test]
    fn bounded_codec_setup_overflow_preserves_the_real_failure_budget() {
        let mut queue = Queue::default();
        queue.optional_setup(true);
        for _ in 0..5 {
            // Each batch starts at a random-access picture. The real bounded
            // queue is exercised rather than replacing it with a fake buffer.
            assert!(queue.submit_codec(vec![1], true, false, 0, Codec::H264));
            for _ in 0..40 {
                queue.submit_codec(vec![2], false, false, 0, Codec::H264);
            }
            assert!(queue.report.queue_depth <= 32);
            assert!(queue.report.failure_stage.is_none());
        }
        queue.optional_setup(false);
        assert!(queue.recover(Instant::now()));
        assert!(queue.recover(Instant::now()));
        assert!(queue.recover(Instant::now()));
        assert!(!queue.recover(Instant::now()));
        assert_eq!(queue.report.failure_stage, Some("video-recovery-exhausted"));
    }
}
