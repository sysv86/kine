use crate::AudioError;

/// Decoded PCM audio, held in memory as interleaved `f32`.
///
/// Samples are *nominal* `-1.0..=1.0`, but nothing clamps them: a mix can carry
/// values outside that range until it is soft-clipped, and honesty about that is
/// better than silently folding headroom away at every step.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioClip {
    /// Samples per second.
    pub sample_rate: u32,
    /// Interleaved channel count (1 = mono, 2 = stereo, ...).
    pub channels: u16,
    /// `frame_count() * channels` samples, interleaved frame by frame.
    pub samples: Vec<f32>,
}

impl AudioClip {
    /// Builds a clip from interleaved samples without checking consistency.
    /// Call [`AudioClip::validate`] before relying on the length invariants.
    pub fn new(sample_rate: u32, channels: u16, samples: Vec<f32>) -> Self {
        Self {
            sample_rate,
            channels,
            samples,
        }
    }

    /// Number of interleaved frames (one frame = one sample per channel).
    ///
    /// Returns 0 for a degenerate channel count rather than dividing by zero.
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    /// Playback length in seconds; 0.0 when the sample rate is unset.
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frame_count() as f64 / self.sample_rate as f64
        }
    }

    /// Sample at `(frame, channel)`, or `None` when out of range.
    pub fn sample(&self, frame: usize, channel: usize) -> Option<f32> {
        let channels = self.channels as usize;
        if channel >= channels {
            return None;
        }
        let index = frame.checked_mul(channels)?.checked_add(channel)?;
        self.samples.get(index).copied()
    }

    /// Checks the invariants every other function in this crate relies on:
    /// a non-zero rate and channel count, and a sample buffer that is a whole
    /// number of frames.
    pub fn validate(&self) -> Result<(), AudioError> {
        if self.channels == 0 {
            return Err(AudioError::InvalidClip("channel count is zero".into()));
        }
        if self.sample_rate == 0 {
            return Err(AudioError::InvalidClip("sample rate is zero".into()));
        }
        if !self.samples.len().is_multiple_of(self.channels as usize) {
            return Err(AudioError::InvalidClip(format!(
                "{} samples do not fill a whole number of {}-channel frames",
                self.samples.len(),
                self.channels
            )));
        }
        Ok(())
    }
}
