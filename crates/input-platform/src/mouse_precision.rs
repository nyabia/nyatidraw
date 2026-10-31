use nyatidraw_input::Point;

/// A bounded approximation used only for the fractional part of cursor motion.
/// The OS message position remains the authority for whole-pixel displacement.
#[derive(Clone, Copy, Debug)]
pub struct MouseMotionPolicy {
    pub speed: u32,
    pub acceleration: u32,
    pub thresholds: [u32; 2],
}

impl MouseMotionPolicy {
    fn gain(self, distance: f64, elapsed_ms: u32) -> f64 {
        let rate = distance / f64::from(elapsed_ms.clamp(1, 32));
        let ramp = |threshold: u32| rate / (rate + f64::from(threshold.max(1)));
        let acceleration = match self.acceleration {
            0 => 1.0,
            1 => 1.0 + ramp(self.thresholds[0]),
            _ => 1.0 + ramp(self.thresholds[0]) + ramp(self.thresholds[1]),
        };
        f64::from(self.speed.clamp(1, 20)) / 10.0 * acceleration
    }
}

/// Fractional integration constrained to half a physical pixel of each OS axis.
/// This alignment bound does not reconstruct Windows' original rounding error
/// or reproduce its enhanced-pointer-precision curve.
#[derive(Clone, Copy, Debug)]
pub struct CursorAlignedMouseMotion {
    position: Point,
    timestamp_ms: u32,
    policy: MouseMotionPolicy,
}

impl CursorAlignedMouseMotion {
    #[must_use]
    pub fn new(position: Point, timestamp_ms: u32, policy: MouseMotionPolicy) -> Option<Self> {
        finite(position).then_some(Self {
            position,
            timestamp_ms,
            policy,
        })
    }

    /// Integrates one relative device packet using its queued OS cursor anchor.
    /// Invalid or older packets leave the accumulator untouched.
    pub fn advance(&mut self, delta: Point, anchor: Point, timestamp_ms: u32) -> Option<Point> {
        let elapsed_ms = timestamp_ms.wrapping_sub(self.timestamp_ms);
        if !finite(delta) || !finite(anchor) || elapsed_ms > u32::MAX / 2 {
            return None;
        }
        let gain = self.policy.gain(delta.x.hypot(delta.y), elapsed_ms);
        let proposed = Point {
            x: delta.x.mul_add(gain, self.position.x),
            y: delta.y.mul_add(gain, self.position.y),
        };
        if !finite(proposed) {
            return None;
        }
        self.position = align(proposed, anchor);
        self.timestamp_ms = timestamp_ms;
        Some(self.position)
    }

    /// Keeps a final legacy-button position aligned without snapping its
    /// surviving fractional component back to an integer.
    pub fn finish(&mut self, anchor: Point) -> Option<Point> {
        if !finite(anchor) {
            return None;
        }
        self.position = align(self.position, anchor);
        Some(self.position)
    }
}

fn finite(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

fn align(point: Point, anchor: Point) -> Point {
    Point {
        x: point.x.clamp(anchor.x - 0.5, anchor.x + 0.5),
        y: point.y.clamp(anchor.y - 0.5, anchor.y + 0.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_motion_never_drifts_from_cursor_or_resets_at_integer_anchors() {
        let policy = MouseMotionPolicy {
            speed: 2,
            acceleration: 0,
            thresholds: [6, 10],
        };
        let mut motion =
            CursorAlignedMouseMotion::new(Point { x: -20.0, y: 7.0 }, 10, policy).unwrap();
        for (time, anchor, expected) in [(11, -20.0, -19.8), (12, -20.0, -19.6), (13, -19.0, -19.4)]
        {
            let point = motion
                .advance(Point { x: 1.0, y: 0.0 }, Point { x: anchor, y: 7.0 }, time)
                .unwrap();
            assert!((point.x - expected).abs() < 1e-10);
        }
        let tail = motion.finish(Point { x: -19.0, y: 7.0 }).unwrap();
        assert!((tail.x + 19.4).abs() < 1e-10);
        for acceleration in 0..=2 {
            for speed in 1..=20 {
                let mut motion = CursorAlignedMouseMotion::new(
                    Point { x: 0.0, y: 0.0 },
                    0,
                    MouseMotionPolicy {
                        speed,
                        acceleration,
                        thresholds: [6, 10],
                    },
                )
                .unwrap();
                for time in 1..=128 {
                    let anchor = Point {
                        x: f64::from(time),
                        y: -f64::from(time),
                    };
                    let point = motion
                        .advance(Point { x: 37.0, y: -17.0 }, anchor, time)
                        .unwrap();
                    assert!((point.x - anchor.x).abs() <= 0.5);
                    assert!((point.y - anchor.y).abs() <= 0.5);
                }
            }
        }
    }

    #[test]
    fn stale_and_nonfinite_packets_cannot_poison_saved_stroke_coordinates() {
        let policy = MouseMotionPolicy {
            speed: 10,
            acceleration: 1,
            thresholds: [6, 10],
        };
        let origin = Point { x: 0.0, y: 0.0 };
        let mut motion = CursorAlignedMouseMotion::new(origin, u32::MAX, policy).unwrap();
        assert!(motion.advance(origin, origin, 0).is_some());
        for (delta, anchor, time) in [
            (origin, origin, u32::MAX),
            (
                Point {
                    x: f64::NAN,
                    y: 0.0,
                },
                origin,
                1,
            ),
            (
                origin,
                Point {
                    x: f64::INFINITY,
                    y: 0.0,
                },
                1,
            ),
        ] {
            assert!(motion.advance(delta, anchor, time).is_none());
            assert_eq!(motion.finish(origin), Some(origin));
        }
    }
}
