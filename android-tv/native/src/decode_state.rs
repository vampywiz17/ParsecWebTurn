//! Compressed-input ownership across the existing bounded queue's resets.
use crate::video_output::{Frame, Queue};
use std::{collections::VecDeque, sync::Mutex};

/// Native buffer ownership tokens only, never pixel data or copied surfaces.
pub struct OutputIndex {
    pub index: usize,
    pub render: bool,
    pub skipped: bool,
}
pub struct OutputIndices {
    indices: VecDeque<OutputIndex>,
    pictures: usize,
}
impl Default for OutputIndices {
    fn default() -> Self {
        Self {
            indices: VecDeque::with_capacity(128),
            pictures: 0,
        }
    }
}
impl OutputIndices {
    pub fn push(&mut self, index: usize, render: bool) -> bool {
        if self.indices.len() == 128 {
            return false;
        }
        self.pictures += usize::from(render);
        self.indices.push_back(OutputIndex {
            index,
            render,
            skipped: false,
        });
        true
    }
}

/// The NDK callback and the codec command handler share a looper. The caller's
/// existing worker must make synchronous release/render calls, after releasing
/// the index mutex, so the NDK callback can return and service those commands.
pub fn drain_outputs(
    queue: &Mutex<OutputIndices>,
    mut release: impl FnMut(OutputIndex) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    loop {
        let next = {
            let mut indices = queue.lock().unwrap_or_else(|e| e.into_inner());
            indices.indices.pop_front().map(|mut output| {
                if output.render {
                    indices.pictures -= 1;
                    // Only an already decoded, newer picture can supersede
                    // this one. Always retain the final picture in a burst.
                    output.skipped = indices.pictures > 0;
                    output.render = !output.skipped;
                }
                output
            })
        };
        let Some(next) = next else {
            return Ok(());
        };
        release(next)?;
    }
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
    fn output_callbacks_remain_unblocked_while_worker_releases_a_surface() {
        use std::sync::{mpsc, Arc};
        let indices = Arc::new(Mutex::new(OutputIndices::default()));
        assert!(indices.lock().unwrap().push(1, true));
        let callback = indices.clone();
        let (start, go) = mpsc::sync_channel(1);
        let (done, returned) = mpsc::sync_channel(1);
        let producer = std::thread::spawn(move || {
            go.recv().unwrap();
            let accepted = callback.lock().unwrap().push(2, true);
            let _ = done.send(accepted);
        });
        let mut released = Vec::new();
        let result = drain_outputs(&indices, |output| {
            if output.index == 1 {
                start.send(()).unwrap();
                anyhow::ensure!(
                    returned
                        .recv_timeout(std::time::Duration::from_secs(2))
                        .unwrap_or(false),
                    "Callback was blocked by synchronous release"
                );
            }
            released.push(output.index);
            assert!(output.render);
            Ok(())
        });
        producer.join().unwrap();
        result.unwrap();
        assert_eq!(released, vec![1, 2]);
    }

    #[test]
    fn output_token_bounds_preserve_order_and_each_ownership_index_exactly_once() {
        let mut indices = OutputIndices::default();
        for index in 0..128 {
            assert!(indices.push(index, index % 2 == 0));
        }
        assert!(!indices.push(128, true));
        let queue = Mutex::new(indices);
        let mut released = Vec::new();
        drain_outputs(&queue, |output| {
            released.push((output.index, output.render));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            released,
            (0..128).map(|i| (i, i == 126)).collect::<Vec<_>>()
        );
        drain_outputs(&queue, |_| panic!("Already released an output index")).unwrap();
    }

    #[test]
    fn output_catch_up_keeps_the_last_picture_even_with_trailing_codec_metadata() {
        let mut indices = OutputIndices::default();
        for (index, picture) in [(0, false), (1, true), (2, true), (3, false)] {
            assert!(indices.push(index, picture));
        }
        let mut released = Vec::new();
        drain_outputs(&Mutex::new(indices), |o| {
            released.push((o.index, o.render, o.skipped));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            released,
            [
                (0, false, false),
                (1, false, true),
                (2, true, false),
                (3, false, false)
            ]
        );
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
