//! Compressed-input ownership across the existing bounded queue's resets.
use crate::video_output::{Frame, Queue};

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
