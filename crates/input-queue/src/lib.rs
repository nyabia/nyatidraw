#![forbid(unsafe_code)]
//! A bounded handoff between the native input and paint loops.

use nyatidraw_input::{PointerPhase, StylusSample};
use std::collections::VecDeque;

/// A queued sample with pressure extrema accumulated from any evicted neighbors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueuedSample {
    pub sample: StylusSample,
    pub pressure_min: f32,
    pub pressure_max: f32,
}

impl QueuedSample {
    fn new(sample: StylusSample) -> Self {
        Self {
            pressure_min: sample.pressure,
            pressure_max: sample.pressure,
            sample,
        }
    }

    fn absorb_pressure(&mut self, other: Self) {
        self.pressure_min = self.pressure_min.min(other.pressure_min);
        self.pressure_max = self.pressure_max.max(other.pressure_max);
    }
}

/// Result of submitting an event to a full queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushError {
    TransitionQueueFull,
}

/// Lifetime accounting for a bounded input queue.
///
/// `coalesced_moves` counts evicted move entries, including an eviction made to
/// admit a non-droppable transition. The counters intentionally retain no
/// coordinates, device IDs, or stroke contents.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InputQueueStats {
    pub enqueued: u64,
    pub coalesced_moves: u64,
    pub rejected_transitions: u64,
    pub max_len: usize,
}

/// Fixed-capacity queue for one producer and one consumer.
#[derive(Debug)]
pub struct InputQueue {
    entries: VecDeque<QueuedSample>,
    capacity: usize,
    stats: InputQueueStats,
}

impl InputQueue {
    /// Creates a queue with preallocated storage and a fixed capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
            stats: InputQueueStats::default(),
        }
    }
    /// Pushes an event. Under saturation, the least-curved move is evicted.
    ///
    /// # Errors
    ///
    /// Returns [`PushError::TransitionQueueFull`] when the zero-capacity queue
    /// or a queue containing only non-droppable transitions cannot accept the
    /// sample. The caller must drain the queue and retry transition samples.
    pub fn push(&mut self, sample: StylusSample) -> Result<(), PushError> {
        if self.capacity == 0 {
            self.count_rejected_transition(sample.phase);
            return Err(PushError::TransitionQueueFull);
        }
        let mut incoming = QueuedSample::new(sample);
        if self.entries.len() == self.capacity {
            if let Some(index) = self.least_important_move(&incoming) {
                let Some(removed) = self.entries.remove(index) else {
                    self.count_rejected_transition(sample.phase);
                    return Err(PushError::TransitionQueueFull);
                };
                self.stats.coalesced_moves = self.stats.coalesced_moves.saturating_add(1);
                if let Some(neighbor) = self.neighboring_move_mut(index) {
                    neighbor.absorb_pressure(removed);
                } else {
                    incoming.absorb_pressure(removed);
                }
            } else {
                self.count_rejected_transition(sample.phase);
                return Err(PushError::TransitionQueueFull);
            }
        }
        self.entries.push_back(incoming);
        self.stats.enqueued = self.stats.enqueued.saturating_add(1);
        self.stats.max_len = self.stats.max_len.max(self.entries.len());
        Ok(())
    }

    fn count_rejected_transition(&mut self, phase: PointerPhase) {
        if phase != PointerPhase::Move {
            self.stats.rejected_transitions = self.stats.rejected_transitions.saturating_add(1);
        }
    }

    fn neighboring_move_mut(&mut self, removed_index: usize) -> Option<&mut QueuedSample> {
        let next_is_move = self
            .entries
            .get(removed_index)
            .is_some_and(|entry| entry.sample.phase == PointerPhase::Move);
        if next_is_move {
            return self.entries.get_mut(removed_index);
        }

        let previous_index = removed_index.checked_sub(1)?;
        self.entries
            .get_mut(previous_index)
            .filter(|entry| entry.sample.phase == PointerPhase::Move)
    }

    fn least_important_move(&self, incoming: &QueuedSample) -> Option<usize> {
        let mut chosen = None;
        let mut lowest = f64::INFINITY;
        for (i, e) in self.entries.iter().enumerate() {
            if e.sample.phase != PointerPhase::Move {
                continue;
            }

            let previous = i
                .checked_sub(1)
                .and_then(|index| self.entries.get(index))
                .unwrap_or(e);
            let next = self.entries.get(i + 1).unwrap_or(incoming);
            let a = previous.sample.position_document;
            let b = e.sample.position_document;
            let c = next.sample.position_document;
            let chord_x = c.x - a.x;
            let chord_y = c.y - a.y;
            let chord_length = chord_x.hypot(chord_y);
            let area_twice = ((b.x - a.x) * chord_y - (b.y - a.y) * chord_x).abs();
            let score = if chord_length <= f64::EPSILON {
                if (b.x - a.x).hypot(b.y - a.y) <= f64::EPSILON {
                    0.0
                } else {
                    f64::MAX
                }
            } else {
                area_twice / chord_length
            };
            if score < lowest {
                lowest = score;
                chosen = Some(i);
            }
        }
        chosen
    }
    /// Removes the oldest queued event for the paint consumer.
    pub fn pop(&mut self) -> Option<QueuedSample> {
        self.entries.pop_front()
    }
    /// Number of queued entries, including coalesced moves.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns aggregate queue behaviour without retaining input content.
    #[must_use]
    pub const fn stats(&self) -> InputQueueStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_input::{PenButtons, Point};

    fn s(phase: PointerPhase, seq: u64, x: f64, y: f64, pressure: f32) -> StylusSample {
        StylusSample {
            sequence: seq,
            timestamp_ns: seq,
            device_id: 1,
            phase,
            position_document: Point { x, y },
            pressure,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons::default(),
            eraser: false,
            viewport_revision: 0,
        }
    }
    #[test]
    fn saturation_at_240hz_preserves_transitions_corner_endpoint_and_pressure_extrema() {
        let mut q = InputQueue::with_capacity(4);
        for (phase, seq, x, y, pressure) in [
            (PointerPhase::Begin, 0, 0.0, 0.0, 0.5),
            (PointerPhase::Move, 1, 1.0, 0.0, 0.1),
            (PointerPhase::Move, 2, 2.0, 0.0, 0.7),
            (PointerPhase::Move, 3, 2.0, 1.0, 1.0),
            (PointerPhase::Move, 4, 2.0, 2.0, 0.4),
            (PointerPhase::End, 5, 2.0, 2.0, 0.3),
        ] {
            let mut sample = s(phase, seq, x, y, pressure);
            sample.timestamp_ns = seq * 4_166_667;
            assert!(q.push(sample).is_ok());
        }

        let drained: Vec<_> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(drained.first().unwrap().sample.phase, PointerPhase::Begin);
        assert_eq!(drained.last().unwrap().sample.phase, PointerPhase::End);
        assert!(drained.iter().any(|entry| entry.sample.sequence == 2));
        assert!(drained.iter().any(|entry| entry.sample.sequence == 4));

        let pressure_min = drained
            .iter()
            .map(|entry| entry.pressure_min)
            .fold(f32::INFINITY, f32::min);
        let pressure_max = drained
            .iter()
            .map(|entry| entry.pressure_max)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((pressure_min - 0.1).abs() < f32::EPSILON);
        assert!((pressure_max - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            q.stats(),
            InputQueueStats {
                enqueued: 6,
                coalesced_moves: 2,
                rejected_transitions: 0,
                max_len: 4,
            }
        );
    }

    #[test]
    fn transition_only_saturation_reports_backpressure_without_dropping_a_transition() {
        let mut q = InputQueue::with_capacity(2);
        assert!(q.push(s(PointerPhase::Begin, 0, 0.0, 0.0, 0.5)).is_ok());
        assert!(q.push(s(PointerPhase::End, 1, 1.0, 0.0, 0.5)).is_ok());

        assert_eq!(
            q.push(s(PointerPhase::Cancel, 2, 1.0, 0.0, 0.5)),
            Err(PushError::TransitionQueueFull)
        );
        assert_eq!(
            q.pop().map(|entry| entry.sample.phase),
            Some(PointerPhase::Begin)
        );
        assert_eq!(
            q.pop().map(|entry| entry.sample.phase),
            Some(PointerPhase::End)
        );
        assert_eq!(q.stats().rejected_transitions, 1);
    }
}
