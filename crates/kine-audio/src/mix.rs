use crate::dsp::{
    apply_biquad, apply_compressor, compute_ducking_envelope, BiquadFilterSpec, CompressorSpec,
};
use crate::{resample, AudioClip, AudioError};
use std::f32::consts::FRAC_PI_4;

/// Fade envelope shape for track fade-in and fade-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeCurve {
    /// Linear ramp ($t$).
    Linear,
    /// Constant-power sine/cosine ramp ($\sin(\frac{\pi}{2} t)$).
    EqualPower,
    /// Smooth cubic Hermite S-curve ($3t^2 - 2t^3$).
    SCurve,
    /// Exponential perceptual curve ($t^2$).
    Exponential,
}

impl Default for FadeCurve {
    fn default() -> Self {
        Self::Linear
    }
}

impl FadeCurve {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "linear" | "lin" => Some(Self::Linear),
            "equalpower" | "equal-power" | "cosine" | "power" => Some(Self::EqualPower),
            "scurve" | "s-curve" | "smooth" | "hermite" => Some(Self::SCurve),
            "exponential" | "exp" => Some(Self::Exponential),
            _ => None,
        }
    }

    #[inline]
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
            Self::SCurve => t * t * (3.0 - 2.0 * t),
            Self::Exponential => t * t,
        }
    }
}

/// One track on the mix timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrackSpec {
    pub clip: AudioClip,
    /// Linear gain (1.0 = unity).
    pub gain: f32,
    /// Start offset on the timeline, in seconds (negative = trim from the start).
    pub start: f64,
    /// Trim from the beginning of the source clip, in seconds.
    pub trim_start: f64,
    /// Optional end timestamp in the source clip, in seconds.
    pub trim_end: Option<f64>,
    /// Whether the source clip loops continuously until the end of the mix.
    pub loop_track: bool,
    /// Stereo pan in `[-1.0, 1.0]` (`-1.0` = hard left, `0.0` = center, `+1.0` = hard right).
    pub pan: f32,
    /// Fade in/out lengths in seconds.
    pub fade_in: f64,
    pub fade_out: f64,
    /// Shape of the fade-in / fade-out ramps.
    pub fade_curve: FadeCurve,
    /// Optional biquad EQ / filter stages applied to this track before mixing.
    pub filters: Vec<BiquadFilterSpec>,
    /// Optional per-track dynamic range compressor.
    pub compressor: Option<CompressorSpec>,
    /// Sidechain ducking attenuation in dB (e.g. `-10.0`). When non-zero, this
    /// track is treated as background and automatically ducks under foreground
    /// (non-ducked) tracks whenever they exceed `duck_threshold_db`.
    pub duck_db: f32,
    /// Threshold in dBFS at which foreground tracks trigger ducking on this track.
    pub duck_threshold_db: f32,
    pub muted: bool,
}

impl Default for AudioTrackSpec {
    fn default() -> Self {
        Self {
            clip: AudioClip::new(48_000, 1, Vec::new()),
            gain: 1.0,
            start: 0.0,
            trim_start: 0.0,
            trim_end: None,
            loop_track: false,
            pan: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            fade_curve: FadeCurve::Linear,
            filters: Vec::new(),
            compressor: None,
            duck_db: 0.0,
            duck_threshold_db: -28.0,
            muted: false,
        }
    }
}

impl AudioTrackSpec {
    /// A unity-gain, un-offset track with no fades.
    pub fn new(clip: AudioClip) -> Self {
        Self {
            clip,
            ..Self::default()
        }
    }
}

/// Point where the soft clipper leaves its linear region. Below it the curve is
/// exactly the identity, so normal mixes are bit-for-bit a plain sum.
const KNEE: f32 = 0.9;

/// Soft clip that maps the whole real line into the **open** interval `(-1, 1)`.
fn soft_clip(x: f32) -> f32 {
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    let over = (magnitude - KNEE) / (1.0 - KNEE);
    let saturated = KNEE + (1.0 - KNEE) * over.tanh();
    if x < 0.0 {
        -saturated
    } else {
        saturated
    }
}

/// Builds the final track: resamples every spec to `target_rate`, trims/loops,
/// runs per-track biquad EQ and compression, applies constant-power stereo
/// panning and shaped fades, applies sidechain voiceover ducking to background
/// tracks, sums everything, and soft-clips the sum.
pub fn mix(
    tracks: &[AudioTrackSpec],
    total_seconds: f64,
    target_rate: u32,
    channels: u16,
) -> Result<AudioClip, AudioError> {
    if target_rate == 0 {
        return Err(AudioError::InvalidClip("target sample rate is zero".into()));
    }
    if channels == 0 {
        return Err(AudioError::InvalidClip("target channel count is zero".into()));
    }
    if !total_seconds.is_finite() || total_seconds < 0.0 {
        return Err(AudioError::InvalidClip(
            "total_seconds must be finite and non-negative".into(),
        ));
    }

    let total_frames = (total_seconds * target_rate as f64).round() as usize;
    if total_frames == 0 {
        return Err(AudioError::EmptyMix);
    }
    let lane = channels as usize;
    let total_samples = total_frames
        .checked_mul(lane)
        .ok_or_else(|| AudioError::InvalidClip("mix length overflows usize".into()))?;

    let any_ducking = tracks
        .iter()
        .any(|t| !t.muted && t.duck_db.abs() > f32::EPSILON);

    let mut foreground_bus = vec![0.0f32; total_samples];
    let mut ducked_specs: Vec<(&AudioTrackSpec, Vec<f32>)> = Vec::new();

    for spec in tracks {
        if spec.muted || spec.clip.frame_count() == 0 {
            continue;
        }
        if !spec.gain.is_finite()
            || !spec.start.is_finite()
            || !spec.fade_in.is_finite()
            || !spec.fade_out.is_finite()
            || !spec.trim_start.is_finite()
            || !spec.pan.is_finite()
        {
            return Err(AudioError::InvalidClip(
                "track gain, start, trim, pan and fades must be finite".into(),
            ));
        }

        let mut processed = resample(&spec.clip, target_rate)?;
        processed = trim_clip(&processed, spec.trim_start, spec.trim_end)?;
        if processed.frame_count() == 0 {
            continue;
        }

        for &filter in &spec.filters {
            apply_biquad(&mut processed, filter)?;
        }
        if let Some(comp) = spec.compressor {
            apply_compressor(&mut processed, comp)?;
        }

        let converted = convert_channels(&processed, channels)?;
        let source_frames = converted.frame_count();
        if source_frames == 0 {
            continue;
        }

        let start_frame = (spec.start * target_rate as f64).round() as i64;
        let effective_frames = if spec.loop_track {
            (total_frames as i64 - start_frame).max(0) as usize
        } else {
            source_frames
        };
        if effective_frames == 0 {
            continue;
        }

        let fade_in_frames =
            seconds_to_frames(spec.fade_in.max(0.0), target_rate).min(effective_frames);
        let fade_out_frames =
            seconds_to_frames(spec.fade_out.max(0.0), target_rate).min(effective_frames);
        let (pan_l, pan_r) = pan_gains(spec.pan, channels);

        if any_ducking && spec.duck_db.abs() > f32::EPSILON {
            let mut stem = vec![0.0f32; total_samples];
            render_track_into(
                &mut stem,
                &converted,
                source_frames,
                effective_frames,
                start_frame,
                total_frames,
                lane,
                spec.gain,
                fade_in_frames,
                fade_out_frames,
                spec.fade_curve,
                pan_l,
                pan_r,
            );
            ducked_specs.push((spec, stem));
        } else {
            render_track_into(
                &mut foreground_bus,
                &converted,
                source_frames,
                effective_frames,
                start_frame,
                total_frames,
                lane,
                spec.gain,
                fade_in_frames,
                fade_out_frames,
                spec.fade_curve,
                pan_l,
                pan_r,
            );
        }
    }

    for (spec, stem) in ducked_specs {
        let env = compute_ducking_envelope(
            &foreground_bus,
            channels,
            target_rate,
            spec.duck_threshold_db,
            spec.duck_db,
        );
        for frame in 0..total_frames {
            let g = env[frame];
            let base = frame * lane;
            for ch in 0..lane {
                foreground_bus[base + ch] += stem[base + ch] * g;
            }
        }
    }

    for sample in foreground_bus.iter_mut() {
        *sample = soft_clip(*sample);
    }

    let clip = AudioClip::new(target_rate, channels, foreground_bus);
    clip.validate()?;
    Ok(clip)
}

#[allow(clippy::too_many_arguments)]
fn render_track_into(
    bus: &mut [f32],
    converted: &AudioClip,
    source_frames: usize,
    effective_frames: usize,
    start_frame: i64,
    total_frames: usize,
    lane: usize,
    base_gain: f32,
    fade_in_frames: usize,
    fade_out_frames: usize,
    fade_curve: FadeCurve,
    pan_l: f32,
    pan_r: f32,
) {
    for frame in 0..effective_frames {
        let destination = start_frame + frame as i64;
        if destination < 0 || destination as usize >= total_frames {
            continue;
        }
        let gain = base_gain
            * fade_factor(
                frame,
                effective_frames,
                fade_in_frames,
                fade_out_frames,
                fade_curve,
            );
        let out_base = destination as usize * lane;
        let in_base = (frame % source_frames) * lane;
        for channel in 0..lane {
            let ch_pan = if lane >= 2 {
                if channel == 0 {
                    pan_l
                } else if channel == 1 {
                    pan_r
                } else {
                    1.0
                }
            } else {
                1.0
            };
            bus[out_base + channel] += converted.samples[in_base + channel] * gain * ch_pan;
        }
    }
}

/// Constant-power stereo pan gains (`pan ∈ [-1.0, 1.0]`).
///
/// At `pan == 0.0`, returns `(1.0, 1.0)` so an un-panned stereo or mono track
/// retains exact unity gain on both channels.
fn pan_gains(pan: f32, channels: u16) -> (f32, f32) {
    if channels < 2 || pan.abs() < 1e-6 {
        return (1.0, 1.0);
    }
    let p = pan.clamp(-1.0, 1.0);
    // Angle in [0, pi/2]; normalized by sqrt(2) so center (pi/4) is 1.0
    let angle = (p + 1.0) * FRAC_PI_4;
    let norm = std::f32::consts::SQRT_2;
    (angle.cos() * norm, angle.sin() * norm)
}

fn trim_clip(
    clip: &AudioClip,
    trim_start: f64,
    trim_end: Option<f64>,
) -> Result<AudioClip, AudioError> {
    if trim_start <= 0.0 && trim_end.is_none() {
        return Ok(clip.clone());
    }
    let total = clip.frame_count();
    let ch = clip.channels as usize;
    let sr = clip.sample_rate as f64;
    let start_f = ((trim_start.max(0.0) * sr).round() as usize).min(total);
    let end_f = match trim_end {
        Some(te) if te.is_finite() => ((te.max(0.0) * sr).round() as usize).clamp(start_f, total),
        _ => total,
    };
    let slice = clip.samples[start_f * ch..end_f * ch].to_vec();
    Ok(AudioClip::new(clip.sample_rate, clip.channels, slice))
}

fn fade_factor(
    frame: usize,
    frames: usize,
    fade_in: usize,
    fade_out: usize,
    curve: FadeCurve,
) -> f32 {
    let fade_in_factor = if fade_in == 0 {
        1.0
    } else {
        curve.apply((frame as f32 / fade_in as f32).min(1.0))
    };
    let from_end = frames - 1 - frame;
    let fade_out_factor = if fade_out == 0 {
        1.0
    } else {
        curve.apply((from_end as f32 / fade_out as f32).min(1.0))
    };
    fade_in_factor.min(fade_out_factor)
}

fn seconds_to_frames(seconds: f64, rate: u32) -> usize {
    let frames = seconds * rate as f64;
    if !frames.is_finite() || frames <= 0.0 {
        0
    } else {
        frames.round() as usize
    }
}

fn convert_channels(clip: &AudioClip, dst: u16) -> Result<AudioClip, AudioError> {
    let src = clip.channels;
    if src == dst {
        return Ok(clip.clone());
    }
    if src == 0 || dst == 0 {
        return Err(AudioError::InvalidClip("channel count is zero".into()));
    }

    let frames = clip.frame_count();
    let (src_channels, dst_channels) = (src as usize, dst as usize);
    let mut samples = vec![0.0f32; frames * dst_channels];

    if src_channels == 1 {
        for frame in 0..frames {
            let value = clip.samples[frame];
            for channel in 0..dst_channels {
                samples[frame * dst_channels + channel] = value;
            }
        }
    } else {
        for frame in 0..frames {
            for channel in 0..src_channels {
                samples[frame * dst_channels + channel % dst_channels] +=
                    clip.samples[frame * src_channels + channel];
            }
        }
        for channel in 0..dst_channels {
            let contributors = (0..src_channels)
                .filter(|source| source % dst_channels == channel)
                .count();
            if contributors > 1 {
                for frame in 0..frames {
                    samples[frame * dst_channels + channel] /= contributors as f32;
                }
            }
        }
    }

    let converted = AudioClip::new(clip.sample_rate, dst, samples);
    converted.validate()?;
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write_wav;

    fn constant(rate: u32, channels: u16, seconds: f64, value: f32) -> AudioClip {
        let frames = (rate as f64 * seconds).round() as usize;
        AudioClip::new(rate, channels, vec![value; frames * channels as usize])
    }

    fn spec(clip: AudioClip) -> AudioTrackSpec {
        AudioTrackSpec::new(clip)
    }

    #[test]
    fn two_identical_tracks_sum_to_twice() {
        let rate = 8_000;
        let track = spec(constant(rate, 1, 0.5, 0.4));
        let single = mix(std::slice::from_ref(&track), 0.5, rate, 1).unwrap();
        let doubled = mix(&[track.clone(), track], 0.5, rate, 1).unwrap();

        for frame in 0..single.frame_count() {
            let one = single.sample(frame, 0).unwrap();
            let two = doubled.sample(frame, 0).unwrap();
            assert!((one - 0.4).abs() < 1e-6, "single = {one}");
            assert!((two - 2.0 * one).abs() < 1e-6, "two = {two}, one = {one}");
        }
    }

    #[test]
    fn sums_above_full_scale_are_soft_clipped_not_clamped() {
        let rate = 8_000;
        let track = spec(constant(rate, 1, 0.2, 0.4));
        let mixed = mix(&[track.clone(), track.clone(), track], 0.2, rate, 1).unwrap();
        let peak = mixed.sample(100, 0).unwrap();
        assert!(peak < 1.0, "soft clip must stay below full scale, got {peak}");
        assert!(peak > 0.99, "soft clip should barely compress 1.2, got {peak}");
    }

    #[test]
    fn muted_track_contributes_nothing() {
        let rate = 8_000;
        let mut track = spec(constant(rate, 1, 0.2, 1.0));
        track.muted = true;
        let mixed = mix(&[track], 0.2, rate, 1).unwrap();
        assert!(mixed.samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn start_offset_shifts_the_content() {
        let rate = 8_000;
        let mut track = spec(constant(rate, 1, 0.5, 0.25));
        track.start = 0.5;
        let mixed = mix(&[track], 1.0, rate, 1).unwrap();

        for frame in 0..4_000 {
            assert_eq!(mixed.sample(frame, 0).unwrap(), 0.0, "frame {frame}");
        }
        for frame in 4_000..8_000 {
            let v = mixed.sample(frame, 0).unwrap();
            assert!((v - 0.25).abs() < 1e-6, "frame {frame} = {v}");
        }
    }

    #[test]
    fn shorter_total_truncates_the_tail() {
        let rate = 8_000;
        let track = spec(constant(rate, 1, 1.0, 0.25));
        let mixed = mix(&[track], 0.25, rate, 1).unwrap();
        assert_eq!(mixed.frame_count(), 2_000);
        assert_eq!(mixed.duration_seconds(), 0.25);
    }

    #[test]
    fn negative_start_trims_the_head() {
        let rate = 8_000;
        let clip = AudioClip::new(
            rate,
            1,
            (0..8_000).map(|i| i as f32 / 8_000.0).collect(),
        );
        let mut track = spec(clip);
        track.start = -0.25;
        let mixed = mix(&[track], 0.5, rate, 1).unwrap();
        let first = mixed.sample(0, 0).unwrap();
        assert!((first - 0.25).abs() < 1e-3, "first = {first}");
    }

    #[test]
    fn fades_are_monotonic() {
        let rate = 8_000;
        let mut track = spec(constant(rate, 1, 1.0, 0.5));
        track.fade_in = 0.25;
        track.fade_out = 0.25;
        let mixed = mix(&[track], 1.0, rate, 1).unwrap();

        let fade_in = 2_000usize;
        let fade_out = 2_000usize;
        for frame in 1..fade_in {
            let prev = mixed.sample(frame - 1, 0).unwrap();
            let cur = mixed.sample(frame, 0).unwrap();
            assert!(cur >= prev, "fade-in dipped at {frame}");
        }
        let count = mixed.frame_count();
        for frame in (count - fade_out + 1)..count {
            let prev = mixed.sample(frame - 1, 0).unwrap();
            let cur = mixed.sample(frame, 0).unwrap();
            assert!(cur <= prev, "fade-out rose at {frame}");
        }
        assert_eq!(mixed.sample(0, 0).unwrap(), 0.0);
        assert_eq!(mixed.sample(count - 1, 0).unwrap(), 0.0);
        let middle = mixed.sample(count / 2, 0).unwrap();
        assert!((middle - 0.5).abs() < 1e-6, "middle = {middle}");
    }

    #[test]
    fn mono_is_duplicated_to_stereo() {
        let rate = 8_000;
        let track = spec(constant(rate, 1, 0.1, 0.3));
        let mixed = mix(&[track], 0.1, rate, 2).unwrap();
        assert_eq!(mixed.channels, 2);
        for frame in 0..mixed.frame_count() {
            assert_eq!(mixed.sample(frame, 0), mixed.sample(frame, 1));
            assert!((mixed.sample(frame, 0).unwrap() - 0.3).abs() < 1e-6);
        }
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        let rate = 8_000;
        let frames = 800;
        let mut samples = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            samples.push(1.0);
            samples.push(0.0);
        }
        let track = spec(AudioClip::new(rate, 2, samples));
        let mixed = mix(&[track], 0.1, rate, 1).unwrap();
        assert_eq!(mixed.channels, 1);
        for frame in 0..mixed.frame_count() {
            let v = mixed.sample(frame, 0).unwrap();
            assert!((v - 0.5).abs() < 1e-6, "frame {frame} = {v}");
        }
    }

    #[test]
    fn arbitrary_channel_fold_averages_contributors() {
        let clip = AudioClip::new(8_000, 3, vec![1.0, 2.0, 3.0]);
        let converted = convert_channels(&clip, 2).unwrap();
        assert_eq!(converted.channels, 2);
        assert!((converted.sample(0, 0).unwrap() - 2.0).abs() < 1e-6);
        assert!((converted.sample(0, 1).unwrap() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn zero_length_mix_is_an_error() {
        assert!(matches!(
            mix(&[], 0.0, 8_000, 1).unwrap_err(),
            AudioError::EmptyMix
        ));
    }

    #[test]
    fn mixing_mismatched_rates_agrees_with_an_explicit_resample() {
        let wav_dir = std::env::temp_dir().join("kine-audio-mix-test");
        std::fs::create_dir_all(&wav_dir).unwrap();
        let path = wav_dir.join("interchange.wav");
        write_wav(&path, &constant(44_100, 2, 0.1, 0.25)).unwrap();
        let decoded = crate::read_wav(&path).unwrap();
        assert_eq!(decoded.sample_rate, 44_100);

        let mixed = mix(&[spec(decoded)], 0.1, 48_000, 2).unwrap();
        assert_eq!(mixed.sample_rate, 48_000);
        assert_eq!(mixed.channels, 2);
    }

    #[test]
    fn loop_pan_and_sidechain_ducking_work_together() {
        let rate = 8_000;
        // 0.1s music loop repeated across 0.5s, panned left, ducked by -12 dB
        let mut music = spec(constant(rate, 1, 0.1, 0.3));
        music.loop_track = true;
        music.pan = -1.0;
        music.duck_db = -12.0;

        // Voice comes in at 0.25s
        let mut voice = spec(constant(rate, 1, 0.2, 0.4));
        voice.start = 0.25;

        let mixed = mix(&[music, voice], 0.5, rate, 2).unwrap();
        assert_eq!(mixed.frame_count(), 4_000);
        // Before voice (at frame 500): left channel has music, right channel is 0
        assert!(mixed.sample(500, 0).unwrap() > 0.35);
        assert!(mixed.sample(500, 1).unwrap().abs() < 1e-5);
        // During voice (at frame 2500): right channel has only voice (~0.4), left has voice + ducked music (< 0.55)
        let left_during = mixed.sample(2500, 0).unwrap();
        assert!(left_during < 0.55, "music should be ducked on left: {left_during}");
    }
}
