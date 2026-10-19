//! Pure-Rust procedural audio synthesizer (`synth:` URIs and programmatic
//! waveforms, sweeps, chords, and noise generators).
//!
//! Lets agents author scored scenes with UI tones, risers, sub-bass drops,
//! chords, or ambient noise beds without needing external audio files on disk.

use crate::{AudioClip, AudioError};
use std::f32::consts::PI;

/// Oscillator or noise waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Square,
    Saw,
    Triangle,
    WhiteNoise,
    PinkNoise,
}

impl Waveform {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "sine" | "sin" => Some(Self::Sine),
            "square" | "sq" => Some(Self::Square),
            "saw" | "sawtooth" => Some(Self::Saw),
            "triangle" | "tri" => Some(Self::Triangle),
            "whitenoise" | "white-noise" | "white" | "noise" => Some(Self::WhiteNoise),
            "pinknoise" | "pink-noise" | "pink" => Some(Self::PinkNoise),
            _ => None,
        }
    }
}

/// Parameters for synthesizing a procedural [`AudioClip`].
#[derive(Debug, Clone)]
pub struct SynthSpec {
    pub waveform: Waveform,
    /// Fundamental frequencies in Hz (multiple frequencies synthesize a chord).
    pub frequencies_hz: Vec<f32>,
    /// Optional end frequency for linear/exponential sweeps (`riser` / `drop`).
    pub sweep_to_hz: Option<f32>,
    pub duration_seconds: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub gain: f32,
    /// Attack time in seconds.
    pub attack: f64,
    /// Release time in seconds.
    pub release: f64,
    /// Deterministic PRNG seed for noise generators.
    pub seed: u64,
}

impl Default for SynthSpec {
    fn default() -> Self {
        Self {
            waveform: Waveform::Sine,
            frequencies_hz: vec![440.0],
            sweep_to_hz: None,
            duration_seconds: 1.0,
            sample_rate: 48_000,
            channels: 1,
            gain: 0.4,
            attack: 0.01,
            release: 0.05,
            seed: 0x5EED_CAFE,
        }
    }
}

/// Synthesize a deterministic [`AudioClip`] from `spec`.
pub fn synthesize(spec: &SynthSpec) -> Result<AudioClip, AudioError> {
    if spec.sample_rate == 0 || spec.channels == 0 {
        return Err(AudioError::InvalidClip(
            "synth sample_rate and channels must be non-zero".into(),
        ));
    }
    if !spec.duration_seconds.is_finite() || spec.duration_seconds <= 0.0 {
        return Err(AudioError::InvalidClip(
            "synth duration_seconds must be positive".into(),
        ));
    }

    let frames = (spec.duration_seconds * f64::from(spec.sample_rate))
        .round()
        .max(1.0) as usize;
    let ch = spec.channels as usize;
    let sr = spec.sample_rate as f32;
    let mut out = Vec::with_capacity(frames * ch);

    let freqs = if spec.frequencies_hz.is_empty() {
        vec![440.0f32]
    } else {
        spec.frequencies_hz.clone()
    };
    let voice_scale = 1.0 / (freqs.len() as f32).sqrt();
    let mut phases = vec![0.0f32; freqs.len()];

    // Deterministic Xorshift64* PRNG + Paul Kellet pink-noise filter state
    let mut rng_state = if spec.seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { spec.seed };
    let mut next_white = || -> f32 {
        rng_state ^= rng_state >> 12;
        rng_state ^= rng_state << 25;
        rng_state ^= rng_state >> 27;
        let u = rng_state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40;
        (u as f32 / ((1u32 << 24) as f32)) * 2.0 - 1.0
    };
    let (mut b0, mut b1, mut b2, mut b3, mut b4, mut b5, mut b6) =
        (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);

    let attack_frames = ((spec.attack.max(0.0) * f64::from(spec.sample_rate)).round() as usize)
        .min(frames);
    let release_frames = ((spec.release.max(0.0) * f64::from(spec.sample_rate)).round() as usize)
        .min(frames);

    for i in 0..frames {
        let progress = (i as f32) / (frames.max(1) as f32);
        let env_in = if attack_frames == 0 {
            1.0
        } else {
            ((i as f32) / (attack_frames as f32)).min(1.0)
        };
        let from_end = frames - 1 - i;
        let env_out = if release_frames == 0 {
            1.0
        } else {
            ((from_end as f32) / (release_frames as f32)).min(1.0)
        };
        let env = env_in.min(env_out) * spec.gain;

        let raw = match spec.waveform {
            Waveform::WhiteNoise => next_white(),
            Waveform::PinkNoise => {
                let white = next_white();
                b0 = 0.99886 * b0 + white * 0.0555179;
                b1 = 0.99332 * b1 + white * 0.0750759;
                b2 = 0.96900 * b2 + white * 0.1538520;
                b3 = 0.86650 * b3 + white * 0.3104856;
                b4 = 0.55000 * b4 + white * 0.5329522;
                b5 = -0.7616 * b5 - white * 0.0168980;
                let pink = b0 + b1 + b2 + b3 + b4 + b5 + b6 + white * 0.5362;
                b6 = white * 0.115926;
                pink * 0.18
            }
            osc => {
                let mut sum = 0.0f32;
                for (vi, &f0) in freqs.iter().enumerate() {
                    let freq = match spec.sweep_to_hz {
                        Some(f1) => f0 + (f1 - f0) * progress,
                        None => f0,
                    };
                    let p = phases[vi];
                    let sample = match osc {
                        Waveform::Sine => (2.0 * PI * p).sin(),
                        Waveform::Square => {
                            if p < 0.5 {
                                1.0
                            } else {
                                -1.0
                            }
                        }
                        Waveform::Saw => 2.0 * p - 1.0,
                        Waveform::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
                        _ => 0.0,
                    };
                    sum += sample;
                    phases[vi] = (p + freq / sr).rem_euclid(1.0);
                }
                sum * voice_scale
            }
        };

        let val = (raw * env).clamp(-1.0, 1.0);
        for _ in 0..ch {
            out.push(val);
        }
    }

    let clip = AudioClip::new(spec.sample_rate, spec.channels, out);
    clip.validate()?;
    Ok(clip)
}

/// Parse a `synth:...` URI into a [`SynthSpec`] and synthesize it.
///
/// Supported URI forms:
/// * `synth:sine:440` (or `square`, `saw`, `triangle` + Hz + optional `:seconds`)
/// * `synth:chord:261.63,329.63,392.00` (polyphonic chord)
/// * `synth:sweep:110->880` (frequency riser / drop)
/// * `synth:white-noise` / `synth:pink-noise`
pub fn synthesize_uri(uri: &str, default_duration: f64, sample_rate: u32, channels: u16) -> Result<AudioClip, AudioError> {
    let body = uri
        .strip_prefix("synth:")
        .ok_or_else(|| AudioError::Decode(format!("not a synth URI: {uri}")))?;
    let parts: Vec<&str> = body.split(':').collect();
    let kind = parts.first().copied().unwrap_or("sine");

    let mut spec = SynthSpec {
        duration_seconds: default_duration.max(0.1),
        sample_rate: sample_rate.max(8_000),
        channels: channels.max(1),
        ..SynthSpec::default()
    };

    if let Some(dur_str) = parts.get(2) {
        if let Ok(d) = dur_str.parse::<f64>() {
            if d > 0.0 {
                spec.duration_seconds = d;
            }
        }
    }

    match kind.to_ascii_lowercase().as_str() {
        "chord" => {
            spec.waveform = Waveform::Triangle;
            if let Some(list) = parts.get(1) {
                let parsed: Vec<f32> = list
                    .split(',')
                    .filter_map(|s| s.trim().parse::<f32>().ok())
                    .collect();
                if !parsed.is_empty() {
                    spec.frequencies_hz = parsed;
                }
            }
        }
        "sweep" | "riser" => {
            spec.waveform = Waveform::Sine;
            if let Some(range) = parts.get(1) {
                if let Some((a, b)) = range.split_once("->") {
                    if let (Ok(f0), Ok(f1)) = (a.trim().parse::<f32>(), b.trim().parse::<f32>()) {
                        spec.frequencies_hz = vec![f0];
                        spec.sweep_to_hz = Some(f1);
                    }
                }
            }
        }
        other => {
            spec.waveform = Waveform::parse(other).ok_or_else(|| {
                AudioError::Decode(format!("unknown synth waveform {other:?} in {uri}"))
            })?;
            if let Some(freq_str) = parts.get(1) {
                let parsed: Vec<f32> = freq_str
                    .split(',')
                    .filter_map(|s| s.trim().parse::<f32>().ok())
                    .collect();
                if !parsed.is_empty() {
                    spec.frequencies_hz = parsed;
                }
            }
        }
    }

    synthesize(&spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesizes_waveforms_and_synth_uris() {
        let sine = synthesize_uri("synth:sine:440:0.5", 1.0, 48_000, 2).unwrap();
        assert_eq!(sine.sample_rate, 48_000);
        assert_eq!(sine.channels, 2);
        assert_eq!(sine.frame_count(), 24_000);
        assert!(sine.metrics().peak_linear > 0.3);

        let chord = synthesize_uri("synth:chord:261.63,329.63,392.0", 0.25, 48_000, 1).unwrap();
        assert_eq!(chord.frame_count(), 12_000);

        let pink = synthesize_uri("synth:pink-noise", 0.2, 48_000, 1).unwrap();
        let pink2 = synthesize_uri("synth:pink-noise", 0.2, 48_000, 1).unwrap();
        assert_eq!(pink, pink2, "pink noise synthesis must be deterministic");
    }
}
