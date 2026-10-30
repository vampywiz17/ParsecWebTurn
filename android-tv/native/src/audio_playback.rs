//! Bounded nonblocking drain of the common PCM queue, on the session worker.
use crate::audio_stream::{Handoff, NativeReceiver};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

struct Pending {
    block: Handoff,
    offset: usize,
    since: Instant,
}
pub struct Playback {
    receiver: NativeReceiver,
    pending: Option<Pending>,
    pub stale_frames: u64,
}
impl Playback {
    pub fn new(receiver: NativeReceiver) -> Self {
        Self {
            receiver,
            pending: None,
            stale_frames: 0,
        }
    }
    /// True means retry shortly: a partial write or the fairness bound left
    /// work to do. Never sleep after just one block while more PCM is ready.
    pub fn pump(&mut self, mut write: impl FnMut(&[i16]) -> Result<usize>) -> Result<bool> {
        for _ in 0..32 {
            if self.pending.is_none() {
                self.pending = self.receiver.take_native_pcm().map(|block| Pending {
                    block,
                    offset: 0,
                    since: Instant::now(),
                });
            }
            let Some(pending) = self.pending.as_mut() else {
                return Ok(false);
            };
            ensure!(
                pending.block.layout == crate::audio_layout::Layout::STEREO,
                "Android audio output requires stereo PCM"
            );
            if pending.since.elapsed() >= Duration::from_millis(100) {
                self.stale_frames += ((pending.block.pcm.len() - pending.offset) / 2) as u64;
                self.receiver
                    .recycle_handoff(self.pending.take().unwrap().block);
                self.receiver.discard_pending();
                return Ok(false);
            }
            let samples = &pending.block.pcm[pending.offset..];
            let written = write(samples)?;
            ensure!(
                written <= samples.len() && written.is_multiple_of(2),
                "Audio output returned an invalid stereo frame count"
            );
            pending.offset += written;
            if pending.offset != pending.block.pcm.len() {
                return Ok(true);
            }
            self.receiver
                .recycle_handoff(self.pending.take().unwrap().block);
        }
        Ok(true)
    }
    pub fn snapshot(&self) -> crate::audio_stream::Snapshot {
        self.receiver.snapshot()
    }
}
impl Drop for Playback {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.receiver.recycle_handoff(pending.block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_stream::Pipeline;

    fn staged(blocks: usize) -> (Pipeline, Playback) {
        let pipeline = Pipeline::start();
        let sender = pipeline.sender().unwrap();
        for n in 0..blocks {
            let pcm: Vec<u8> = (0..96).flat_map(|_| (n as i16 + 1).to_le_bytes()).collect();
            assert!(sender.submit_pcm(&pcm));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while pipeline.snapshot().packets_decoded < blocks as u64 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let playback = Playback::new(pipeline.native_receiver());
        (pipeline, playback)
    }
    #[test]
    fn drains_all_ready_blocks_before_idle_and_preserves_partial_writes() {
        let (_pipeline, mut playback) = staged(3);
        let mut received = Vec::new();
        assert!(playback
            .pump(|pcm| {
                received.extend_from_slice(&pcm[..20]);
                Ok(20)
            })
            .unwrap());
        assert!(playback.pump(|_| Ok(0)).unwrap());
        assert!(!playback
            .pump(|pcm| {
                received.extend_from_slice(pcm);
                Ok(pcm.len())
            })
            .unwrap());
        assert_eq!(received, [vec![1; 96], vec![2; 96], vec![3; 96]].concat());
        assert_eq!(playback.snapshot().queued_frames, 0);
    }
    #[test]
    fn fairness_bound_retries_instead_of_parking_with_ready_audio() {
        let (_pipeline, mut playback) = staged(40);
        let mut blocks = 0;
        assert!(playback
            .pump(|pcm| {
                blocks += 1;
                Ok(pcm.len())
            })
            .unwrap());
        assert_eq!(blocks, 32);
        assert!(!playback
            .pump(|pcm| {
                blocks += 1;
                Ok(pcm.len())
            })
            .unwrap());
        assert_eq!(blocks, 40);
    }
    #[test]
    fn stalled_output_retires_stale_partial_audio_before_writing_again() {
        let (_pipeline, mut playback) = staged(2);
        assert!(playback.pump(|_| Ok(20)).unwrap());
        playback.pending.as_mut().unwrap().since = Instant::now() - Duration::from_millis(101);
        assert!(!playback
            .pump(|_| panic!("stale audio must not be played"))
            .unwrap());
        assert_eq!(playback.stale_frames, 38);
        assert_eq!(playback.snapshot().queued_frames, 0);
    }
}
