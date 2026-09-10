//! Optional position-only, causal stroke correction.
//!
//! Feed each accepted brush sample exactly once, before both live brush evaluation
//! and recording the evaluated samples. Replay consumes those recorded positions
//! without applying this filter a second time. This is not input-loss recovery.

use crate::{Point, PointerPhase, StylusSample};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmoothingError {
    NonFinitePosition,
}

#[derive(Clone, Copy, Debug)]
struct Anchor {
    position: Point,
    sequence: u64,
    timestamp_ns: u64,
    device_id: u64,
    viewport_revision: u64,
}

/// Bounded-memory one-pole position filter, with no lookahead or sample expansion.
///
/// Capture strength at stroke Begin: 0 disables correction, 1..=100 maps linearly
/// to a 0.2..=20 ms time constant. This intentionally adds position lag, not a
/// scheduling delay. Pressure, phase, time and all other metadata remain unchanged.
/// Begin is exact. End goes to the exact physical endpoint in one final segment;
/// it does not leave a shortened stroke or manufacture a delayed catch-up tail.
/// That straight final segment can show a corner at high strength; this minimal
/// filter does not reconstruct a curved endpoint or perform predictive smoothing.
/// Cancel passes through and resets immediately. Create/reset outside a brush
/// gesture so navigation, selection and color picking do not inherit brush state.
#[derive(Clone, Debug, Default)]
pub struct StrokeSmoother {
    strength: u8,
    anchor: Option<Anchor>,
}

impl StrokeSmoother {
    #[must_use]
    pub fn new(strength: u8) -> Self {
        Self {
            strength: strength.min(100),
            anchor: None,
        }
    }

    pub fn reset(&mut self) {
        self.anchor = None;
    }

    /// Processes one sample; successful processing always returns exactly one
    /// sample with the original phase and metadata. Off is a bit-preserving
    /// identity, including malformed input (upstream validation still applies).
    ///
    /// Equal/backward timestamps or sequence, gaps over 250 ms, device changes and viewport
    /// changes restart at the raw coordinate, never extrapolate or mix spaces.
    /// An orphan Move/End passes unchanged: this filter is not an admission gate.
    ///
    /// # Errors
    ///
    /// When enabled, a non-finite position resets the filter and returns
    /// `NonFinitePosition`. The caller must reject/cancel that stroke explicitly,
    /// not silently discard an End and leave a live gesture open. Cancel always
    /// passes through unchanged because its coordinates are not painted.
    pub fn process(&mut self, mut sample: StylusSample) -> Result<StylusSample, SmoothingError> {
        if self.strength == 0 || sample.phase == PointerPhase::Cancel {
            self.reset();
            return Ok(sample);
        }
        if !sample.position_document.x.is_finite() || !sample.position_document.y.is_finite() {
            self.reset();
            return Err(SmoothingError::NonFinitePosition);
        }
        match sample.phase {
            PointerPhase::Begin => self.remember(sample),
            PointerPhase::End | PointerPhase::Cancel => self.reset(),
            PointerPhase::Move => {
                if let Some(previous) = self.anchor {
                    let elapsed = sample.timestamp_ns.checked_sub(previous.timestamp_ns);
                    if previous.device_id == sample.device_id
                        && previous.viewport_revision == sample.viewport_revision
                        && sample.sequence > previous.sequence
                        && let Some(elapsed @ 1..=250_000_000) = elapsed
                        && let Ok(elapsed) = u32::try_from(elapsed)
                    {
                        // elapsed is bounded to 250 ms, hence losslessly fits u32.
                        let elapsed = f64::from(elapsed);
                        let tau_ns = f64::from(self.strength) * 200_000.0;
                        let weight = -(-elapsed / tau_ns).exp_m1();
                        let raw = sample.position_document;
                        let filtered = Point {
                            x: previous.position.x * (1.0 - weight) + raw.x * weight,
                            y: previous.position.y * (1.0 - weight) + raw.y * weight,
                        };
                        // Convex interpolation avoids raw-minus-previous overflow.
                        // Guard the remaining rounding extremes without poisoning
                        // future input with a non-finite filter state.
                        if filtered.x.is_finite() && filtered.y.is_finite() {
                            sample.position_document = filtered;
                        }
                    }
                    self.remember(sample);
                }
            }
        }
        Ok(sample)
    }

    fn remember(&mut self, sample: StylusSample) {
        self.anchor = Some(Anchor {
            position: sample.position_document,
            sequence: sample.sequence,
            timestamp_ns: sample.timestamp_ns,
            device_id: sample.device_id,
            viewport_revision: sample.viewport_revision,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PenButtons;

    fn sample(sequence: u64, phase: PointerPhase, position: Point) -> StylusSample {
        StylusSample {
            sequence,
            timestamp_ns: sequence * 4_000_000,
            device_id: 17,
            phase,
            position_document: position,
            pressure: 0.17 + f32::from(u8::try_from(sequence % 4).unwrap()) * 0.2,
            tilt: Some([0.3, -0.2]),
            twist_radians: Some(1.3),
            tangential_pressure: Some(-0.4),
            buttons: PenButtons(3),
            eraser: true,
            viewport_revision: 13,
        }
    }

    #[test]
    fn correction_preserves_replay_metadata_phase_endpoints_and_batch_independence() {
        let input = [
            sample(1, PointerPhase::Begin, Point { x: -10.0, y: 8.0 }),
            sample(2, PointerPhase::Move, Point { x: 0.0, y: 14.0 }),
            sample(3, PointerPhase::Move, Point { x: 10.0, y: 6.0 }),
            sample(4, PointerPhase::Move, Point { x: 20.0, y: 14.0 }),
            sample(5, PointerPhase::End, Point { x: 30.0, y: 10.0 }),
        ];
        for strength in [0, 1, 50, 100, 255] {
            let mut single = StrokeSmoother::new(strength);
            let expected: Vec<_> = input.iter().map(|s| single.process(*s).unwrap()).collect();
            for chunk_size in 1..=input.len() {
                let mut batched = StrokeSmoother::new(strength);
                let actual: Vec<_> = input
                    .chunks(chunk_size)
                    .flat_map(|chunk| {
                        chunk
                            .iter()
                            .map(|s| batched.process(*s).unwrap())
                            .collect::<Vec<_>>()
                    })
                    .collect();
                assert_eq!(actual, expected, "chunking must not change saved artwork");
            }
            for (raw, filtered) in input.iter().zip(&expected) {
                assert_eq!(
                    StylusSample {
                        position_document: raw.position_document,
                        ..*filtered
                    },
                    *raw
                );
                if strength == 0 || matches!(raw.phase, PointerPhase::Begin | PointerPhase::End) {
                    assert_eq!(filtered, raw, "off and endpoints remain exact");
                }
            }
            if strength > 0 {
                assert!(expected[1].position_document.x > input[0].position_document.x);
                assert!(expected[1].position_document.x < input[1].position_document.x);
            }
        }
        let mut weak = StrokeSmoother::new(20);
        let mut strong = StrokeSmoother::new(100);
        weak.process(input[0]).unwrap();
        strong.process(input[0]).unwrap();
        assert!(
            weak.process(input[1]).unwrap().position_document.x
                > strong.process(input[1]).unwrap().position_document.x
        );
    }

    #[test]
    fn cancellation_and_clock_changes_cannot_leak_a_previous_stroke() {
        let begin = sample(1, PointerPhase::Begin, Point { x: -100.0, y: 0.0 });
        let next = sample(2, PointerPhase::Move, Point { x: 100.0, y: 0.0 });
        for phase in [PointerPhase::Cancel, PointerPhase::End] {
            let mut filter = StrokeSmoother::new(100);
            filter.process(begin).unwrap();
            filter.process(StylusSample { phase, ..next }).unwrap();
            assert_eq!(
                filter.process(next).unwrap(),
                next,
                "orphan move cannot resume terminated stroke"
            );
            assert_eq!(filter.process(begin).unwrap(), begin);
        }
        for changed in [
            StylusSample {
                sequence: begin.sequence,
                ..next
            },
            StylusSample {
                sequence: 0,
                ..next
            },
            StylusSample {
                timestamp_ns: begin.timestamp_ns,
                ..next
            },
            StylusSample {
                timestamp_ns: 0,
                ..next
            },
            StylusSample {
                timestamp_ns: u64::MAX,
                ..next
            },
            StylusSample {
                device_id: 999,
                ..next
            },
            StylusSample {
                viewport_revision: 999,
                ..next
            },
        ] {
            let mut filter = StrokeSmoother::new(100);
            filter.process(begin).unwrap();
            assert_eq!(
                filter.process(changed).unwrap(),
                changed,
                "invalid timing or changed space reanchors without extrapolation"
            );
        }
    }

    #[test]
    fn malformed_coordinates_cannot_poison_later_strokes_or_swallow_cancel() {
        let begin = sample(1, PointerPhase::Begin, Point { x: -100.0, y: 0.0 });
        let next = sample(2, PointerPhase::Move, Point { x: 100.0, y: 0.0 });
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let malformed = StylusSample {
                position_document: Point { x: invalid, y: 3.0 },
                ..next
            };
            let mut filter = StrokeSmoother::new(100);
            filter.process(begin).unwrap();
            assert_eq!(
                filter.process(malformed),
                Err(SmoothingError::NonFinitePosition)
            );
            assert_eq!(filter.process(next).unwrap(), next);
            let off = StrokeSmoother::new(0).process(malformed).unwrap();
            assert_eq!(
                off.position_document.x.to_bits(),
                invalid.to_bits(),
                "off preserves raw bits for upstream validation"
            );
            let cancel = filter
                .process(StylusSample {
                    phase: PointerPhase::Cancel,
                    ..malformed
                })
                .unwrap();
            assert_eq!(
                cancel.phase,
                PointerPhase::Cancel,
                "bad coordinates cannot swallow cancellation"
            );
        }
        let mut filter = StrokeSmoother::new(100);
        filter
            .process(StylusSample {
                position_document: Point {
                    x: -f64::MAX,
                    y: f64::MAX,
                },
                ..begin
            })
            .unwrap();
        let output = filter
            .process(StylusSample {
                position_document: Point {
                    x: f64::MAX,
                    y: -f64::MAX,
                },
                ..next
            })
            .unwrap();
        assert!(output.position_document.x.is_finite() && output.position_document.y.is_finite());
    }
}
