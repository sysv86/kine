//! Audio DSP building blocks: Robert Bristow-Johnson biquad filters (LowPass,
//! HighPass, BandPass, Notch, PeakingEQ, LowShelf, HighShelf), soft-knee dynamic
//! range compression, envelope followers for sidechain voiceover ducking, and
//! loudness/peak metering.

use crate::{AudioClip, AudioError};
use std::f64::consts::PI;

/// One filter stage in a track's DSP chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BiquadFilterSpec {
    LowPass { cutoff_hz: f32, q: f32 },
    HighPass { cutoff_hz: f32, q: f32 },
    BandPass { center_hz: f32, q: f32 },
    Notch { center_hz: f32, q: f32 },
    PeakingEq { center_hz: f32, q: f32, gain_db: f32 },
    LowShelf { cutoff_hz: f32, gain_db: f32 },
    HighShelf { cutoff_hz: f32, gain_db: f32 },
}

/// Normalized biquad transfer-function coefficients (`Direct Form II Transposed`).
#[derive(Debug, Clone, Copy)]
pub struct BiquadCoeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl BiquadCoeffs {
    /// Compute RBJ Audio EQ Cookbook coefficients for `sample_rate`.
    pub fn from_spec(spec: BiquadFilterSpec, sample_rate: u32) -> Self {
        let sr = (sample_rate.max(1)) as f64;
        let clamp_freq = |f: f32| (f as f64).clamp(10.0, sr * 0.495);
        let clamp_q = |q: f32| (q as f64).clamp(0.1, 24.0);

        let (b0, b1, b2, a0, a1, a2) = match spec {
            BiquadFilterSpec::LowPass { cutoff_hz, q } => {
                let w0 = 2.0 * PI * clamp_freq(cutoff_hz) / sr;
                let cos_w0 = w0.cos();
                let alpha = w0.sin() / (2.0 * clamp_q(q));
                let b1 = 1.0 - cos_w0;
                let b0 = b1 * 0.5;
                let b2 = b0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            BiquadFilterSpec::HighPass { cutoff_hz, q } => {
                let w0 = 2.0 * PI * clamp_freq(cutoff_hz) / sr;
                let cos_w0 = w0.cos();
                let alpha = w0.sin() / (2.0 * clamp_q(q));
                let b0 = (1.0 + cos_w0) * 0.5;
                let b1 = -(1.0 + cos_w0);
                let b2 = b0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            BiquadFilterSpec::BandPass { center_hz, q } => {
                let w0 = 2.0 * PI * clamp_freq(center_hz) / sr;
                let cos_w0 = w0.cos();
                let alpha = w0.sin() / (2.0 * clamp_q(q));
                let b0 = alpha;
                let b1 = 0.0;
                let b2 = -alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            BiquadFilterSpec::Notch { center_hz, q } => {
                let w0 = 2.0 * PI * clamp_freq(center_hz) / sr;
                let cos_w0 = w0.cos();
                let alpha = w0.sin() / (2.0 * clamp_q(q));
                let b0 = 1.0;
                let b1 = -2.0 * cos_w0;
                let b2 = 1.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            BiquadFilterSpec::PeakingEq {
                center_hz,
                q,
                gain_db,
            } => {
                let a = 10.0f64.powf((gain_db as f64) / 40.0);
                let w0 = 2.0 * PI * clamp_freq(center_hz) / sr;
                let cos_w0 = w0.cos();
                let alpha = w0.sin() / (2.0 * clamp_q(q));
                let b0 = 1.0 + alpha * a;
                let b1 = -2.0 * cos_w0;
                let b2 = 1.0 - alpha * a;
                let a0 = 1.0 + alpha / a;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha / a;
                (b0, b1, b2, a0, a1, a2)
            }
            BiquadFilterSpec::LowShelf { cutoff_hz, gain_db } => {
                let a = 10.0f64.powf((gain_db as f64) / 40.0);
                let w0 = 2.0 * PI * clamp_freq(cutoff_hz) / sr;
                let cos_w0 = w0.cos();
                let sin_w0 = w0.sin();
                let two_sqrt_a_alpha = 2.0 * a.sqrt() * (sin_w0 / 2.0 * std::f64::consts::SQRT_2);
                let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
                let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
                let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
                let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
                let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
                let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            BiquadFilterSpec::HighShelf { cutoff_hz, gain_db } => {
                let a = 10.0f64.powf((gain_db as f64) / 40.0);
                let w0 = 2.0 * PI * clamp_freq(cutoff_hz) / sr;
                let cos_w0 = w0.cos();
                let sin_w0 = w0.sin();
                let two_sqrt_a_alpha = 2.0 * a.sqrt() * (sin_w0 / 2.0 * std::f64::consts::SQRT_2);
                let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
                let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
                let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
                let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
                let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
                let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
                (b0, b1, b2, a0, a1, a2)
            }
        };

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

/// Apply a biquad filter in-place across all channels of `clip`.
pub fn apply_biquad(clip: &mut AudioClip, spec: BiquadFilterSpec) -> Result<(), AudioError> {
    clip.validate()?;
    let coeffs = BiquadCoeffs::from_spec(spec, clip.sample_rate);
    let channels = clip.channels as usize;
    let frames = clip.frame_count();
    let mut z1 = vec![0.0f64; channels];
    let mut z2 = vec![0.0f64; channels];

    for frame in 0..frames {
        let base = frame * channels;
        for ch in 0..channels {
            let x = f64::from(clip.samples[base + ch]);
            let y = coeffs.b0 * x + z1[ch];
            z1[ch] = coeffs.b1 * x - coeffs.a1 * y + z2[ch];
            z2[ch] = coeffs.b2 * x - coeffs.a2 * y;
            clip.samples[base + ch] = y as f32;
        }
    }
    Ok(())
}

/// Dynamic range compressor configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompressorSpec {
    /// Threshold in dBFS (e.g. `-18.0`).
    pub threshold_db: f32,
    /// Compression ratio (e.g. `4.0` for 4:1).
    pub ratio: f32,
    /// Attack time in milliseconds (e.g. `10.0`).
    pub attack_ms: f32,
    /// Release time in milliseconds (e.g. `120.0`).
    pub release_ms: f32,
    /// Makeup gain in dB (e.g. `3.0`).
    pub makeup_db: f32,
}

impl Default for CompressorSpec {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 3.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            makeup_db: 0.0,
        }
    }
}

/// Apply linked-stereo feed-forward dynamic range compression in place.
pub fn apply_compressor(clip: &mut AudioClip, spec: CompressorSpec) -> Result<(), AudioError> {
    clip.validate()?;
    let sr = clip.sample_rate as f32;
    let channels = clip.channels as usize;
    let frames = clip.frame_count();
    let ratio = spec.ratio.max(1.0);
    let attack_coeff = (-1.0 / ((spec.attack_ms.max(0.5) * 0.001) * sr)).exp();
    let release_coeff = (-1.0 / ((spec.release_ms.max(1.0) * 0.001) * sr)).exp();
    let makeup_lin = 10.0f32.powf(spec.makeup_db / 20.0);

    let mut env_db = -96.0f32;
    for frame in 0..frames {
        let base = frame * channels;
        let mut peak = 0.0f32;
        for ch in 0..channels {
            peak = peak.max(clip.samples[base + ch].abs());
        }
        let input_db = if peak <= 1e-5 {
            -100.0
        } else {
            20.0 * peak.log10()
        };
        let coeff = if input_db > env_db {
            attack_coeff
        } else {
            release_coeff
        };
        env_db = coeff * env_db + (1.0 - coeff) * input_db;

        let gain_reduction_db = if env_db > spec.threshold_db {
            (spec.threshold_db - env_db) * (1.0 - 1.0 / ratio)
        } else {
            0.0
        };
        let gain = 10.0f32.powf(gain_reduction_db / 20.0) * makeup_lin;
        for ch in 0..channels {
            clip.samples[base + ch] *= gain;
        }
    }
    Ok(())
}

/// Compute a smooth sidechain gain envelope (`0.0..=1.0` multiplier per frame)
/// from a foreground stem (such as a voiceover) to duck background tracks.
///
/// Whenever the foreground stem's envelope exceeds `threshold_db`, the returned
/// multiplier smoothly drops toward `10^(duck_db / 20)`.
pub fn compute_ducking_envelope(
    foreground_interleaved: &[f32],
    channels: u16,
    sample_rate: u32,
    threshold_db: f32,
    duck_db: f32,
) -> Vec<f32> {
    let ch = (channels.max(1)) as usize;
    let frames = foreground_interleaved.len() / ch;
    let sr = (sample_rate.max(1)) as f32;

    let min_gain = 10.0f32.powf(-duck_db.abs() / 20.0).clamp(0.0, 1.0);
    let thresh_lin = 10.0f32.powf(threshold_db / 20.0);
    let attack_coeff = (-1.0 / (0.015 * sr)).exp(); // 15 ms fast duck attack
    let release_coeff = (-1.0 / (0.220 * sr)).exp(); // 220 ms smooth release
    let hold_samples = (0.060 * sr) as usize; // 60 ms hold to avoid pumping between syllables

    let mut out = vec![1.0f32; frames];
    let mut current_gain = 1.0f32;
    let mut hold_counter = 0usize;

    for frame in 0..frames {
        let base = frame * ch;
        let mut peak = 0.0f32;
        for c in 0..ch {
            peak = peak.max(foreground_interleaved[base + c].abs());
        }
        let target_gain = if peak >= thresh_lin {
            hold_counter = hold_samples;
            min_gain
        } else if hold_counter > 0 {
            hold_counter -= 1;
            min_gain
        } else {
            1.0
        };

        let coeff = if target_gain < current_gain {
            attack_coeff
        } else {
            release_coeff
        };
        current_gain = coeff * current_gain + (1.0 - coeff) * target_gain;
        out[frame] = current_gain;
    }
    out
}

/// Summary loudness and dynamic metrics of an [`AudioClip`].
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct AudioMetrics {
    pub peak_linear: f32,
    pub peak_db: f32,
    pub rms_linear: f32,
    pub rms_db: f32,
    /// Approximate integrated loudness in LUFS.
    pub lufs_approx: f32,
}

impl AudioClip {
    /// Compute peak, RMS, and approximate LUFS loudness metrics.
    pub fn metrics(&self) -> AudioMetrics {
        if self.samples.is_empty() {
            return AudioMetrics {
                peak_linear: 0.0,
                peak_db: -100.0,
                rms_linear: 0.0,
                rms_db: -100.0,
                lufs_approx: -100.0,
            };
        }
        let mut peak = 0.0f32;
        let mut sum_sq = 0.0f64;
        for &s in &self.samples {
            let a = s.abs();
            if a > peak {
                peak = a;
            }
            sum_sq += f64::from(s) * f64::from(s);
        }
        let rms = (sum_sq / (self.samples.len() as f64)).sqrt() as f32;
        let to_db = |x: f32| if x <= 1e-5 { -100.0 } else { 20.0 * x.log10() };
        let rms_db = to_db(rms);
        AudioMetrics {
            peak_linear: peak,
            peak_db: to_db(peak),
            rms_linear: rms,
            rms_db,
            lufs_approx: (rms_db - 0.691).max(-100.0),
        }
    }

    /// Normalize the clip's peak amplitude to `target_peak_db` (e.g. `-1.0` dBFS).
    pub fn normalize_peak(&mut self, target_peak_db: f32) {
        let m = self.metrics();
        if m.peak_linear > 1e-6 {
            let target_lin = 10.0f32.powf(target_peak_db / 20.0);
            let scale = target_lin / m.peak_linear;
            for s in &mut self.samples {
                *s *= scale;
            }
        }
    }

    /// Extract a per-video-frame RMS envelope (`0.0..=1.0`) for audio-reactive
    /// visual animations or shaders.
    pub fn frame_envelope(&self, fps_num: u32, fps_den: u32, total_frames: usize) -> Vec<f32> {
        if total_frames == 0 || self.frame_count() == 0 || fps_num == 0 || fps_den == 0 {
            return vec![0.0; total_frames];
        }
        let ch = (self.channels.max(1)) as usize;
        let total_audio_frames = self.frame_count();
        let mut env = Vec::with_capacity(total_frames);

        for f in 0..total_frames {
            let t0 = (f as f64) * f64::from(fps_den) / f64::from(fps_num);
            let t1 = ((f + 1) as f64) * f64::from(fps_den) / f64::from(fps_num);
            let a0 = ((t0 * f64::from(self.sample_rate)).floor() as usize).min(total_audio_frames);
            let a1 = ((t1 * f64::from(self.sample_rate)).ceil() as usize)
                .clamp(a0, total_audio_frames);
            if a0 >= a1 {
                env.push(0.0);
                continue;
            }
            let mut sum_sq = 0.0f64;
            let count = (a1 - a0) * ch;
            for idx in (a0 * ch)..(a1 * ch) {
                let s = f64::from(self.samples[idx]);
                sum_sq += s * s;
            }
            let rms = (sum_sq / (count.max(1) as f64)).sqrt() as f32;
            env.push(rms.clamp(0.0, 1.0));
        }
        env
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highpass_attenuates_dc_offset() {
        let mut clip = AudioClip::new(48_000, 1, vec![0.5f32; 4_800]);
        apply_biquad(
            &mut clip,
            BiquadFilterSpec::HighPass {
                cutoff_hz: 200.0,
                q: 0.707,
            },
        )
        .unwrap();
        // After the filter settles, DC must be near zero
        assert!(clip.samples.last().unwrap().abs() < 1e-2);
    }

    #[test]
    fn ducking_envelope_attenuates_during_voice_activity() {
        let mut voice = vec![0.0f32; 4_800];
        // Active voice burst in the middle
        for s in &mut voice[1_200..2_400] {
            *s = 0.5;
        }
        let env = compute_ducking_envelope(&voice, 1, 48_000, -24.0, -12.0);
        assert!((env[0] - 1.0).abs() < 1e-4, "silent head stays at unity");
        // 15 ms one-pole attack: 800 samples (16.7 ms) in, the envelope is
        // roughly halfway to the -12 dB floor, and fully ducked by ~35 ms.
        assert!(env[2_000] < 0.55, "attack must be underway: {}", env[2_000]);
        assert!(env[2_900] < 0.35, "active voice ducks background: {}", env[2_900]);
    }
}
