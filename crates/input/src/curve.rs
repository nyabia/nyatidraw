use crate::{Point, PointerPhase, StrokeSmoother, StylusSample};

const MAX_SEGMENT_SAMPLES: u32 = 64;
const MAX_BATCH_SAMPLES: usize = 2 * MAX_SEGMENT_SAMPLES as usize;
const FLATNESS_PX: f64 = 0.05;
const MAX_LOOKAHEAD_DISTANCE: f64 = 2.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrokePathError {
    NonFinitePosition,
    InvalidContinuity,
    SampleBudgetExceeded,
    InvalidScale,
}

/// Evaluated samples are recorded exactly as emitted; replay never runs this path.
#[derive(Clone, Debug)]
pub struct StrokeSampleBatch {
    samples: [StylusSample; MAX_BATCH_SAMPLES],
    len: usize,
}

impl StrokeSampleBatch {
    fn empty(seed: StylusSample) -> Self {
        Self {
            samples: [seed; MAX_BATCH_SAMPLES],
            len: 0,
        }
    }

    #[must_use]
    pub fn as_slice(&self) -> &[StylusSample] {
        &self.samples[..self.len]
    }
}

/// Position stabilization followed by a bounded, interpolating cubic path.
/// Zero is an exact bypass. Short segments may wait for one following input;
/// longer segments and stationary axis updates reach their endpoint immediately.
/// A failed path must cancel the owner's whole gesture and await a fresh Begin.
#[derive(Clone, Debug)]
pub struct StrokePath {
    enabled: bool,
    smoother: StrokeSmoother,
    previous: Option<StylusSample>,
    anchor: Option<StylusSample>,
    pending: Option<StylusSample>,
    last_input: Option<StylusSample>,
    sequence: u64,
    scale: f64,
}

impl StrokePath {
    #[must_use]
    pub fn new(strength: u8) -> Self {
        Self::with_scale(strength, 1.0)
    }

    /// Captures document-to-logical-screen scale at Begin, without platform types.
    #[must_use]
    pub fn with_scale(strength: u8, scale: f64) -> Self {
        Self {
            enabled: strength != 0,
            smoother: StrokeSmoother::new(strength),
            previous: None,
            anchor: None,
            pending: None,
            last_input: None,
            sequence: 0,
            scale,
        }
    }

    /// The last received input, including a segment not yet emitted.
    #[must_use]
    pub const fn last_input(&self) -> Option<StylusSample> {
        self.last_input
    }

    /// # Errors
    /// Non-finite geometry, broken ordering/device identity, or exhausted bounded
    /// subdivision/sequence space reject the entire gesture, never a partial tail.
    pub fn process(&mut self, sample: StylusSample) -> Result<StrokeSampleBatch, StrokePathError> {
        let result = self.process_inner(sample);
        if result.is_err() {
            self.clear();
        }
        result
    }

    fn process_inner(
        &mut self,
        sample: StylusSample,
    ) -> Result<StrokeSampleBatch, StrokePathError> {
        let mut output = StrokeSampleBatch::empty(sample);
        if !self.enabled || sample.phase == PointerPhase::Cancel {
            self.clear();
            self.last_input = (!matches!(sample.phase, PointerPhase::End | PointerPhase::Cancel))
                .then_some(sample);
            output.samples[0] = sample;
            output.len = 1;
            return Ok(output);
        }
        if !finite(sample.position_document) {
            return Err(StrokePathError::NonFinitePosition);
        }
        if !self.scale.is_finite() || !(0.01..=100.0).contains(&self.scale) {
            return Err(StrokePathError::InvalidScale);
        }
        if sample.phase == PointerPhase::Begin {
            self.clear();
            self.sequence = sample.sequence;
            let corrected = self
                .smoother
                .process(sample)
                .map_err(|_| StrokePathError::NonFinitePosition)?;
            self.anchor = Some(corrected);
            self.last_input = Some(sample);
            output.samples[0] = corrected;
            output.len = 1;
            return Ok(output);
        }
        let Some(last) = self.last_input else {
            return Err(StrokePathError::InvalidContinuity);
        };
        if sample.sequence <= last.sequence
            || sample.timestamp_ns < last.timestamp_ns
            || sample.device_id != last.device_id
        {
            return Err(StrokePathError::InvalidContinuity);
        }
        let separate = sample.viewport_revision != last.viewport_revision
            || sample.timestamp_ns == last.timestamp_ns
            || sample.timestamp_ns - last.timestamp_ns > 250_000_000
            || sample.buttons != last.buttons
            || sample.eraser != last.eraser;
        let corrected = self
            .smoother
            .process(sample)
            .map_err(|_| StrokePathError::NonFinitePosition)?;
        self.last_input = Some(sample);
        if let Some(pending) = self.pending.take() {
            self.emit_segment(pending, (!separate).then_some(corrected), &mut output)?;
        }
        if separate {
            self.previous = None;
        }
        let anchor = self.anchor.ok_or(StrokePathError::InvalidContinuity)?;
        let distance = vector(anchor.position_document, corrected.position_document)?.length();
        let stationary = sample.position_document == last.position_document;
        let can_wait = sample.phase == PointerPhase::Move
            && !separate
            && !stationary
            && distance * self.scale <= MAX_LOOKAHEAD_DISTANCE;
        if can_wait {
            self.pending = Some(corrected);
        } else {
            self.emit_segment(corrected, None, &mut output)?;
        }
        if sample.phase == PointerPhase::End {
            self.clear();
        }
        Ok(output)
    }

    fn clear(&mut self) {
        self.smoother.reset();
        self.previous = None;
        self.anchor = None;
        self.pending = None;
        self.last_input = None;
    }

    fn emit_segment(
        &mut self,
        endpoint: StylusSample,
        following: Option<StylusSample>,
        output: &mut StrokeSampleBatch,
    ) -> Result<(), StrokePathError> {
        let start = self.anchor.ok_or(StrokePathError::InvalidContinuity)?;
        let points = controls(
            self.previous.map(|s| s.position_document),
            start.position_document,
            endpoint.position_document,
            following.map(|s| s.position_document),
        )?;
        let divisions = subdivisions(points, self.scale)?;
        if output.len + divisions > MAX_BATCH_SAMPLES {
            return Err(StrokePathError::SampleBudgetExceeded);
        }
        for step in 1..=divisions {
            let sequence = self
                .sequence
                .checked_add(1)
                .ok_or(StrokePathError::SampleBudgetExceeded)?;
            let mut evaluated = if step == divisions {
                endpoint
            } else {
                interpolate(start, endpoint, points, step, divisions)
            };
            if !finite(evaluated.position_document) {
                return Err(StrokePathError::NonFinitePosition);
            }
            evaluated.sequence = sequence;
            self.sequence = sequence;
            output.samples[output.len] = evaluated;
            output.len += 1;
        }
        self.previous = self.anchor;
        self.anchor = Some(endpoint);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Vector {
    x: f64,
    y: f64,
}

impl Vector {
    fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    fn scaled(self, scale: f64) -> Self {
        Self {
            x: self.x * scale,
            y: self.y * scale,
        }
    }
}

fn vector(start: Point, end: Point) -> Result<Vector, StrokePathError> {
    let value = Vector {
        x: end.x - start.x,
        y: end.y - start.y,
    };
    if !value.length().is_finite() {
        return Err(StrokePathError::NonFinitePosition);
    }
    Ok(value)
}

fn finite(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

fn mix(left: Point, right: Point, t: f64) -> Point {
    Point {
        x: left.x * (1.0 - t) + right.x * t,
        y: left.y * (1.0 - t) + right.y * t,
    }
}

fn corner(incoming: Vector, outgoing: Vector) -> bool {
    let left = incoming.length();
    let right = outgoing.length();
    if left <= f64::EPSILON || right <= f64::EPSILON {
        return true;
    }
    let cosine =
        (incoming.x / left) * (outgoing.x / right) + (incoming.y / left) * (outgoing.y / right);
    cosine <= 0.5
}

fn tangent(incoming: Vector, outgoing: Vector, length: f64) -> Vector {
    let left = incoming.scaled(1.0 / incoming.length());
    let right = outgoing.scaled(1.0 / outgoing.length());
    let sum = Vector {
        x: left.x + right.x,
        y: left.y + right.y,
    };
    sum.scaled(length / sum.length())
}

fn controls(
    previous: Option<Point>,
    start: Point,
    end: Point,
    following: Option<Point>,
) -> Result<[Point; 4], StrokePathError> {
    let chord = vector(start, end)?;
    let before = previous.map(|point| vector(point, start)).transpose()?;
    let after = following.map(|point| vector(end, point)).transpose()?;
    let linear = chord.length() <= f64::EPSILON
        || before.is_some_and(|before| corner(before, chord))
        || after.is_some_and(|after| corner(chord, after));
    if linear {
        return Ok([
            start,
            mix(start, end, 1.0 / 3.0),
            mix(start, end, 2.0 / 3.0),
            end,
        ]);
    }
    let handle = chord.length() / 3.0;
    let left = before.map_or(chord.scaled(1.0 / 3.0), |before| {
        tangent(before, chord, handle.min(before.length() / 3.0))
    });
    let right = after.map_or(chord.scaled(1.0 / 3.0), |after| {
        tangent(chord, after, handle.min(after.length() / 3.0))
    });
    let points = [
        start,
        Point {
            x: start.x + left.x,
            y: start.y + left.y,
        },
        Point {
            x: end.x - right.x,
            y: end.y - right.y,
        },
        end,
    ];
    if points.iter().all(|point| finite(*point)) {
        Ok(points)
    } else {
        Err(StrokePathError::NonFinitePosition)
    }
}

fn subdivisions(points: [Point; 4], scale: f64) -> Result<usize, StrokePathError> {
    let differences = [
        vector(points[0], points[1])?,
        vector(points[1], points[2])?,
        vector(points[2], points[3])?,
    ];
    let curvature = differences.windows(2).fold(0.0_f64, |largest, pair| {
        largest.max((pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y))
    });
    let mut divisions = 1_u32;
    // |B''| <= 6 * curvature; linear subdivision error <= |B''| / (8 * divisions^2).
    while curvature * scale * 0.75 / f64::from(divisions * divisions) > FLATNESS_PX {
        if divisions >= MAX_SEGMENT_SAMPLES {
            return Err(StrokePathError::SampleBudgetExceeded);
        }
        divisions *= 2;
    }
    Ok(divisions as usize)
}

fn interpolate(
    start: StylusSample,
    end: StylusSample,
    points: [Point; 4],
    step: usize,
    divisions: usize,
) -> StylusSample {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let (t, axis_t) = (
        step as f64 / divisions as f64,
        step as f32 / divisions as f32,
    );
    let first = mix(points[0], points[1], t);
    let second = mix(points[1], points[2], t);
    let third = mix(points[2], points[3], t);
    let timestamp_delta = end.timestamp_ns - start.timestamp_ns;
    let timestamp = u128::from(start.timestamp_ns)
        + u128::from(timestamp_delta) * step as u128 / divisions as u128;
    StylusSample {
        phase: PointerPhase::Move,
        position_document: mix(mix(first, second, t), mix(second, third, t), t),
        pressure: start.pressure * (1.0 - axis_t) + end.pressure * axis_t,
        timestamp_ns: u64::try_from(timestamp).expect("interpolation is bounded by its endpoints"),
        ..end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PenButtons;

    fn sample(sequence: u64, phase: PointerPhase, x: f64, y: f64) -> StylusSample {
        StylusSample {
            sequence,
            timestamp_ns: sequence * 4_000_000,
            device_id: 7,
            phase,
            position_document: Point { x, y },
            pressure: 0.5,
            tilt: Some([0.2, -0.1]),
            twist_radians: Some(0.7),
            tangential_pressure: Some(-0.3),
            buttons: PenButtons(1),
            eraser: false,
            viewport_revision: 3,
        }
    }

    #[test]
    fn disabled_path_preserves_recorded_input_bits_and_all_transitions() {
        let mut path = StrokePath::with_scale(0, f64::NAN);
        for phase in [
            PointerPhase::Begin,
            PointerPhase::Move,
            PointerPhase::End,
            PointerPhase::Cancel,
        ] {
            for x in [-0.0, 0.7, f64::INFINITY, f64::NAN] {
                let input = sample(17, phase, x, 0.0);
                let output = path.process(input).unwrap();
                assert_eq!(output.as_slice().len(), 1);
                let output = output.as_slice()[0];
                assert_eq!(output.position_document.x.to_bits(), x.to_bits());
                assert_eq!(
                    StylusSample {
                        position_document: Point::default(),
                        ..output
                    },
                    StylusSample {
                        position_document: Point::default(),
                        ..input
                    }
                );
            }
        }
    }

    #[test]
    fn short_strokes_stationary_pressure_and_endpoints_survive_curve_expansion() {
        for scale in [0.25, 0.5, 1.0, 2.0] {
            for strength in [1, 50, 100] {
                let inputs = [
                    sample(10, PointerPhase::Begin, 0.0, 0.0),
                    sample(11, PointerPhase::Move, 1.0 / scale, 0.5 / scale),
                    StylusSample {
                        pressure: 0.9,
                        ..sample(12, PointerPhase::Move, 1.0 / scale, 0.5 / scale)
                    },
                    sample(13, PointerPhase::Move, 2.0 / scale, 0.0),
                    sample(14, PointerPhase::End, 2.3 / scale, -0.3 / scale),
                ];
                let mut path = StrokePath::with_scale(strength, scale);
                let mut evaluated = Vec::new();
                for input in inputs {
                    let batch = path.process(input).unwrap();
                    assert!(batch.as_slice().len() <= MAX_BATCH_SAMPLES);
                    if input.sequence == 12 {
                        assert_eq!(
                            batch.as_slice().last().unwrap().pressure.to_bits(),
                            0.9_f32.to_bits(),
                            "stationary pressure must not wait for another movement"
                        );
                    }
                    evaluated.extend_from_slice(batch.as_slice());
                }
                assert_eq!(evaluated.first(), inputs.first());
                let last = evaluated.last().unwrap();
                assert_eq!(last.position_document, inputs[4].position_document);
                assert_eq!(last.phase, PointerPhase::End);
                assert!(
                    evaluated[1..evaluated.len() - 1]
                        .iter()
                        .all(|s| s.phase == PointerPhase::Move)
                );
                assert!(evaluated.windows(2).all(
                    |p| p[1].sequence > p[0].sequence && p[1].timestamp_ns >= p[0].timestamp_ns
                ));
                for input in inputs {
                    assert!(
                        evaluated
                            .iter()
                            .any(|s| s.timestamp_ns == input.timestamp_ns
                                && s.pressure.to_bits() == input.pressure.to_bits()
                                && s.tilt == input.tilt
                                && s.twist_radians == input.twist_radians
                                && s.tangential_pressure == input.tangential_pressure)
                    );
                }
                assert!(path.last_input().is_none());
            }
        }
    }

    #[test]
    fn slow_small_zigzags_and_sharp_corners_cannot_be_replaced_by_overshooting_curves() {
        let mut path = StrokePath::new(100);
        let mut output = Vec::new();
        for index in 0_u32..8 {
            let input = StylusSample {
                timestamp_ns: u64::from(index) * 100_000_000,
                ..sample(
                    u64::from(index) + 1,
                    if index == 0 {
                        PointerPhase::Begin
                    } else if index == 7 {
                        PointerPhase::End
                    } else {
                        PointerPhase::Move
                    },
                    f64::from(index) * 0.25,
                    if index % 2 == 0 { 0.25 } else { -0.25 },
                )
            };
            output.extend_from_slice(path.process(input).unwrap().as_slice());
        }
        for pair in output.windows(2) {
            assert!(
                pair[0].position_document.y * pair[1].position_document.y < 0.0,
                "slow deliberate subpixel zigzags must keep every alternating turn"
            );
        }
        for following in [Point { x: 1.0, y: 1.0 }, Point { x: 0.0, y: 0.0 }] {
            let points = controls(
                None,
                Point::default(),
                Point { x: 1.0, y: 0.0 },
                Some(following),
            )
            .unwrap();
            assert!(
                points.iter().all(|p| p.y == 0.0),
                "corners and cusps cannot grow a rounded hook"
            );
        }
        let points = controls(
            Some(Point { x: -10.0, y: -2.0 }),
            Point::default(),
            Point { x: 10.0, y: 0.0 },
            Some(Point { x: 20.0, y: -2.0 }),
        )
        .unwrap();
        assert!(
            points[1].y > 0.0 && points[2].y > 0.0,
            "a broad smooth turn must use a curve even above the lookahead distance"
        );
        assert!(subdivisions(points, 1.0).unwrap() > 1);
    }

    #[test]
    fn zoom_and_batching_cannot_change_screen_space_curve_geometry() {
        let mut reference = None;
        for scale in [0.25, 0.5, 1.0, 2.0] {
            let inputs = [
                sample(1, PointerPhase::Begin, 0.0, 0.0),
                sample(2, PointerPhase::Move, 1.0 / scale, 0.2 / scale),
                sample(3, PointerPhase::Move, 2.0 / scale, 0.7 / scale),
                sample(4, PointerPhase::Move, 3.0 / scale, 1.7 / scale),
                sample(5, PointerPhase::End, 4.0 / scale, 3.0 / scale),
            ];
            for chunk_size in 1..=inputs.len() {
                let mut path = StrokePath::with_scale(50, scale);
                let mut output = Vec::new();
                for chunk in inputs.chunks(chunk_size) {
                    for input in chunk {
                        for sample in path.process(*input).unwrap().as_slice() {
                            output.push((
                                sample.position_document.x * scale,
                                sample.position_document.y * scale,
                                sample.pressure,
                                sample.sequence,
                            ));
                        }
                    }
                }
                if let Some(expected) = &reference {
                    assert_eq!(&output, expected);
                } else {
                    reference = Some(output);
                }
            }
        }
    }

    #[test]
    fn pending_tail_cancellation_and_invalid_input_cannot_leak_into_another_stroke() {
        let begin = sample(1, PointerPhase::Begin, 0.0, 0.0);
        let pending = sample(2, PointerPhase::Move, 1.0, 0.0);
        for phase in [PointerPhase::Cancel, PointerPhase::End] {
            let mut path = StrokePath::new(100);
            path.process(begin).unwrap();
            assert!(path.process(pending).unwrap().as_slice().is_empty());
            assert_eq!(path.last_input(), Some(pending));
            let output = path.process(sample(3, phase, 1.0, 0.0)).unwrap();
            if phase == PointerPhase::Cancel {
                assert_eq!(
                    output.as_slice().len(),
                    1,
                    "cancel must discard, not paint, the pending tail"
                );
            } else {
                assert!(
                    output.as_slice().len() >= 2,
                    "shutdown End must flush the pending endpoint"
                );
            }
            assert!(path.last_input().is_none());
            assert!(path.process(pending).is_err());
            assert_eq!(path.process(begin).unwrap().as_slice(), &[begin]);
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut path = StrokePath::new(100);
            path.process(begin).unwrap();
            path.process(pending).unwrap();
            assert_eq!(
                path.process(sample(3, PointerPhase::End, bad, 0.0))
                    .unwrap_err(),
                StrokePathError::NonFinitePosition
            );
            assert!(path.process(pending).is_err());
            assert_eq!(path.process(begin).unwrap().as_slice(), &[begin]);
        }
        let mut path = StrokePath::new(100);
        path.process(sample(u64::MAX / 4_000_000, PointerPhase::Begin, 0.0, 0.0))
            .unwrap();
        path.sequence = u64::MAX;
        let end = StylusSample {
            sequence: u64::MAX,
            timestamp_ns: u64::MAX,
            ..sample(2, PointerPhase::End, 1.0, 1.0)
        };
        assert_eq!(
            path.process(end).unwrap_err(),
            StrokePathError::SampleBudgetExceeded
        );
        let too_curved = [
            Point::default(),
            Point { x: 1.0e6, y: 1.0e6 },
            Point {
                x: 2.0e6,
                y: -1.0e6,
            },
            Point { x: 3.0e6, y: 0.0 },
        ];
        assert_eq!(
            subdivisions(too_curved, 1.0),
            Err(StrokePathError::SampleBudgetExceeded)
        );
    }
}
