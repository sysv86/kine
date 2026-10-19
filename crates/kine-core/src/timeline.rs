use std::collections::HashMap;

/// A keyframe: time (seconds) → value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Keyframe {
    pub time: f64,
    pub value: f64,
}

/// Easing function applied to a segment.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// Analytical cubic-bezier(x1, y1, x2, y2) curve.
    CubicBezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    /// Discrete step function (`steps > 0`).
    Steps {
        steps: u32,
    },
    /// Analytical damped harmonic oscillator (under-damped, critically-damped, and over-damped).
    Spring {
        stiffness: f64,
        damping: f64,
    },
}

impl Easing {
    /// Evaluate easing at normalized progress `t ∈ [0, 1]`.
    pub fn eval(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Self::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
            Self::CubicBezier { x1, y1, x2, y2 } => {
                eval_cubic_bezier(t, *x1, *y1, *x2, *y2)
            }
            Self::Steps { steps } => {
                let n = (*steps).max(1) as f64;
                (t * n).floor().min(n - 1.0) / (n - 1.0).max(1.0)
            }
            Self::Spring { stiffness, damping } => {
                let k = stiffness.max(1e-6);
                let c = damping.max(0.0);
                let omega = k.sqrt();
                let zeta = c / (2.0 * omega);

                if zeta < 1.0 - 1e-5 {
                    // Under-damped harmonic oscillator
                    let omega_d = omega * (1.0 - zeta * zeta).sqrt();
                    1.0 - (-zeta * omega * t).exp()
                        * ((omega_d * t).cos() + (zeta * omega / omega_d) * (omega_d * t).sin())
                } else if (zeta - 1.0).abs() <= 1e-5 {
                    // Critically damped oscillator
                    1.0 - (1.0 + omega * t) * (-omega * t).exp()
                } else {
                    // Over-damped oscillator: exact closed-form two-real-root solution
                    let s = (zeta * zeta - 1.0).sqrt();
                    let r1 = -omega * (zeta - s);
                    let r2 = -omega * (zeta + s);
                    let c1 = r2 / (r2 - r1);
                    let c2 = -r1 / (r2 - r1);
                    1.0 - (c1 * (r1 * t).exp() + c2 * (r2 * t).exp())
                }
            }
        }
    }
}

/// Deterministic Newton-Raphson + bisection solver for CSS/motion cubic-bezier(x1, y1, x2, y2).
fn eval_cubic_bezier(t: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    if (x1 - y1).abs() < 1e-9 && (x2 - y2).abs() < 1e-9 {
        return t;
    }

    let sample_x = |u: f64| -> f64 {
        let inv = 1.0 - u;
        3.0 * inv * inv * u * x1 + 3.0 * inv * u * u * x2 + u * u * u
    };
    let sample_dx = |u: f64| -> f64 {
        let inv = 1.0 - u;
        3.0 * inv * inv * x1 + 6.0 * inv * u * (x2 - x1) + 3.0 * u * u * (1.0 - x2)
    };
    let sample_y = |u: f64| -> f64 {
        let inv = 1.0 - u;
        3.0 * inv * inv * u * y1 + 3.0 * inv * u * u * y2 + u * u * u
    };

    let mut u = t;
    for _ in 0..8 {
        let err = sample_x(u) - t;
        if err.abs() < 1e-7 {
            return sample_y(u);
        }
        let d = sample_dx(u);
        if d.abs() < 1e-6 {
            break;
        }
        u = (u - err / d).clamp(0.0, 1.0);
    }

    // Fallback bisection for steep tangents
    let mut lo = 0.0;
    let mut hi = 1.0;
    u = t;
    for _ in 0..16 {
        let x = sample_x(u);
        if (x - t).abs() < 1e-7 {
            break;
        }
        if t > x {
            lo = u;
        } else {
            hi = u;
        }
        u = 0.5 * (lo + hi);
    }
    sample_y(u)
}

/// A keyframe track: sequence of keyframes with easing between them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KeyframeTrack {
    pub keyframes: Vec<Keyframe>,
    pub easing: Easing,
}

impl KeyframeTrack {
    pub fn new(keyframes: Vec<Keyframe>, easing: Easing) -> Self {
        let mut keyframes = keyframes;
        // `total_cmp` orders NaN deterministically instead of panicking (a
        // NaN keyframe time is bad input, not a crash).
        keyframes.sort_by(|a, b| a.time.total_cmp(&b.time));
        Self { keyframes, easing }
    }

    /// Start and end time of this track in seconds.
    pub fn time_range(&self) -> (f64, f64) {
        let first = self
            .keyframes
            .first()
            .map(|k| if k.time.is_finite() { k.time } else { 0.0 })
            .unwrap_or(0.0);
        let last = self
            .keyframes
            .last()
            .map(|k| if k.time.is_finite() { k.time } else { 0.0 })
            .unwrap_or(0.0);
        (first, last)
    }

    /// Evaluate the track at time `t` (seconds).
    pub fn eval(&self, t: f64) -> f64 {
        if self.keyframes.is_empty() {
            return 0.0;
        }
        if t <= self.keyframes[0].time {
            return self.keyframes[0].value;
        }
        if t >= self.keyframes[self.keyframes.len() - 1].time {
            return self.keyframes[self.keyframes.len() - 1].value;
        }

        // Find segment
        for window in self.keyframes.windows(2) {
            let k0 = &window[0];
            let k1 = &window[1];
            if t >= k0.time && t < k1.time {
                let span = k1.time - k0.time;
                if span <= f64::EPSILON {
                    return k1.value;
                }
                let local_t = (t - k0.time) / span;
                let eased = self.easing.eval(local_t);
                return k0.value + (k1.value - k0.value) * eased;
            }
        }

        self.keyframes[self.keyframes.len() - 1].value
    }
}

/// Animation state: a registry of named tracks.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnimationState {
    tracks: HashMap<String, KeyframeTrack>,
}

impl AnimationState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, track: KeyframeTrack) {
        self.tracks.insert(name.into(), track);
    }

    pub fn eval(&self, name: &str, t: f64) -> Option<f64> {
        self.tracks.get(name).map(|tr| tr.eval(t))
    }

    pub fn track_names(&self) -> Vec<&str> {
        self.tracks.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_interpolation() {
        let track = KeyframeTrack::new(
            vec![
                Keyframe { time: 0.0, value: 0.0 },
                Keyframe { time: 1.0, value: 100.0 },
            ],
            Easing::Linear,
        );
        assert_eq!(track.eval(0.0), 0.0);
        assert_eq!(track.eval(0.5), 50.0);
        assert_eq!(track.eval(1.0), 100.0);
    }

    #[test]
    fn test_clamp_before_after() {
        let track = KeyframeTrack::new(
            vec![
                Keyframe { time: 1.0, value: 10.0 },
                Keyframe { time: 2.0, value: 20.0 },
            ],
            Easing::Linear,
        );
        assert_eq!(track.eval(0.0), 10.0);
        assert_eq!(track.eval(3.0), 20.0);
    }

    #[test]
    fn test_spring_converges() {
        let track = KeyframeTrack::new(
            vec![
                Keyframe { time: 0.0, value: 0.0 },
                Keyframe { time: 2.0, value: 100.0 },
            ],
            Easing::Spring {
                stiffness: 100.0,
                damping: 20.0,
            },
        );
        // At t=2.0 (end of spring), should be near 100.
        let v = track.eval(2.0);
        assert!((v - 100.0).abs() < 1.0, "Spring did not converge: {v}");
    }

    #[test]
    fn test_overdamped_spring_and_cubic_bezier() {
        let overdamped = Easing::Spring {
            stiffness: 100.0,
            damping: 40.0, // zeta = 2.0 > 1.0
        };
        assert!(overdamped.eval(0.0).abs() < 1e-9);
        let mid = overdamped.eval(0.5);
        assert!(mid > 0.0 && mid < 1.0, "unexpected overdamped mid: {mid}");

        let bez = Easing::CubicBezier {
            x1: 0.25,
            y1: 0.1,
            x2: 0.25,
            y2: 1.0,
        };
        assert!(bez.eval(0.0).abs() < 1e-9);
        assert!((bez.eval(1.0) - 1.0).abs() < 1e-9);
        assert!(bez.eval(0.5) > 0.5);
    }

    #[test]
    fn test_nan_keyframe_time_does_not_panic() {
        // Bad input must not crash the renderer; NaN sorts deterministically
        // instead of unwrapping a failed `partial_cmp`.
        let track = KeyframeTrack::new(
            vec![
                Keyframe { time: f64::NAN, value: 5.0 },
                Keyframe { time: 1.0, value: 10.0 },
            ],
            Easing::Linear,
        );
        assert!(track.eval(0.5).is_finite());
        assert!(track.eval(2.0).is_finite());
    }
}
