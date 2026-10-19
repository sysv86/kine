use crate::{AudioClip, AudioError};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Resampler, SincInterpolationParameters, WindowFunction};

/// Resampler used by this crate: `rubato`'s **synchronous sinc** (`Async::new_sinc`)
/// with a Blackman-Harris windowed sinc of 256 taps at 128x oversampling. It was
/// picked over the FFT resampler because it accepts any output/input ratio (the
/// video pipeline may ask for anything), has no FFT-size divisibility constraints
/// and its startup transient is deterministic and trimmed by `process_all`.
const SINC_LEN: usize = 256;

/// Internal chunk size. Only affects how much work `/` memory each internal pass
/// uses, not the result: `process_all` trims the startup delay and returns exactly
/// `ceil(ratio * frames)` output frames.
const CHUNK_FRAMES: usize = 1024;

/// Resamples `clip` to `target_rate`, preserving channel count and duration.
///
/// A clip already at `target_rate` is returned unchanged (a clone, not a
/// rebuild), so the common "everything is 48 kHz already" case costs nothing.
pub fn resample(clip: &AudioClip, target_rate: u32) -> Result<AudioClip, AudioError> {
    clip.validate()?;
    if target_rate == 0 {
        return Err(AudioError::InvalidClip("target sample rate is zero".into()));
    }
    if clip.sample_rate == target_rate {
        return Ok(clip.clone());
    }

    let frames = clip.frame_count();
    if frames == 0 {
        return Ok(AudioClip::new(target_rate, clip.channels, Vec::new()));
    }

    let channels = clip.channels as usize;
    let ratio = target_rate as f64 / clip.sample_rate as f64;
    let params = SincInterpolationParameters::new(SINC_LEN, WindowFunction::BlackmanHarris2);
    let mut resampler = Async::<f32>::new_sinc(
        ratio,
        1.0,
        &params,
        CHUNK_FRAMES,
        channels,
        FixedAsync::Input,
    )
    .map_err(resample_error)?;

    let input = InterleavedSlice::new(&clip.samples, channels, frames)
        .map_err(|err| AudioError::Resample(err.to_string()))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(resample_error)?;
    let samples = output.take_data();

    let resampled = AudioClip::new(target_rate, clip.channels, samples);
    resampled.validate()?;
    Ok(resampled)
}

fn resample_error<E: std::fmt::Display>(err: E) -> AudioError {
    AudioError::Resample(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, channels: u16, hz: f32, seconds: f64) -> AudioClip {
        let frames = (rate as f64 * seconds).round() as usize;
        let mut samples = Vec::with_capacity(frames * channels as usize);
        for i in 0..frames {
            let v = (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin();
            for _ in 0..channels {
                samples.push(v);
            }
        }
        AudioClip::new(rate, channels, samples)
    }

    #[test]
    fn same_rate_is_a_passthrough() {
        let clip = sine(48_000, 2, 1_000.0, 0.1);
        let out = resample(&clip, 48_000).unwrap();
        assert_eq!(out, clip);
    }

    #[test]
    fn upsampling_preserves_duration_within_a_sample_or_two() {
        let clip = sine(44_100, 2, 440.0, 1.0);
        let out = resample(&clip, 48_000).unwrap();
        assert_eq!(out.sample_rate, 48_000);
        assert_eq!(out.channels, 2);

        let expected = clip.duration_seconds();
        let got = out.duration_seconds();
        assert!(
            (got - expected).abs() <= 2.0 / 48_000.0,
            "duration drifted: {got} vs {expected}"
        );
    }

    #[test]
    fn downsampling_preserves_duration_within_a_sample_or_two() {
        let clip = sine(48_000, 1, 440.0, 0.5);
        let out = resample(&clip, 44_100).unwrap();
        let expected = clip.duration_seconds();
        let got = out.duration_seconds();
        assert!(
            (got - expected).abs() <= 2.0 / 44_100.0,
            "duration drifted: {got} vs {expected}"
        );
    }

    #[test]
    fn dc_stays_dc() {
        let clip = AudioClip::new(44_100, 1, vec![0.5f32; 44_100]);
        let out = resample(&clip, 48_000).unwrap();
        // Ignore the very edges, where the sinc window is still ramping in.
        for frame in 100..(out.frame_count() - 100) {
            let v = out.sample(frame, 0).unwrap();
            assert!((v - 0.5).abs() < 1e-3, "frame {frame} = {v}");
        }
    }

    #[test]
    fn zero_target_rate_is_rejected() {
        let clip = sine(48_000, 1, 440.0, 0.01);
        assert!(matches!(
            resample(&clip, 0).unwrap_err(),
            AudioError::InvalidClip(_)
        ));
    }
}
