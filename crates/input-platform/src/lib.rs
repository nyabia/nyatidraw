//! Platform adapters for `nyatidraw-input`.
//!
//! The Windows adapter owns all Win32 interaction. Apps feed its emitted
//! [`StylusSample`] values directly to their bounded input queue; this crate
//! deliberately does not own an unbounded queue or update Dioxus state.

// The only unsafe operations in this crate are confined to `windows.rs`.

use std::collections::HashMap;

use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};

/// Buttons reported by the Windows pen adapter.
///
/// These values are adapter-local conventions stored in the platform-neutral
/// `PenButtons` bitfield. They deliberately do not expose Win32 flag values.
pub const PEN_BUTTON_BARREL: PenButtons = PenButtons(1 << 0);
pub const PEN_BUTTON_PRIMARY: PenButtons = PenButtons(1 << 1);
pub const PEN_BUTTON_SECONDARY: PenButtons = PenButtons(1 << 2);
pub const PEN_BUTTON_TERTIARY: PenButtons = PenButtons(1 << 3);
pub const PEN_BUTTON_FOURTH: PenButtons = PenButtons(1 << 4);

/// Affine mapping from a native canvas client pixel to document space.
///
/// The desktop host must replace this mapping whenever its canvas DPI, size, or
/// viewport changes, and increment `revision` at the same time. A sample is
/// transformed using the snapshot current when the OS event is handled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportSnapshot {
    pub revision: u64,
    pub xx: f64,
    pub xy: f64,
    pub yx: f64,
    pub yy: f64,
    pub tx: f64,
    pub ty: f64,
}

impl ViewportSnapshot {
    #[must_use]
    pub const fn identity(revision: u64) -> Self {
        Self {
            revision,
            xx: 1.0,
            xy: 0.0,
            yx: 0.0,
            yy: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    #[must_use]
    pub fn document_point(self, client_px: Point) -> Point {
        Point {
            x: self
                .xx
                .mul_add(client_px.x, self.xy.mul_add(client_px.y, self.tx)),
            y: self
                .yx
                .mul_add(client_px.x, self.yy.mul_add(client_px.y, self.ty)),
        }
    }
}

/// The message kind after the Windows message loop has identified a pen event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsPointerMessage {
    Down,
    Update,
    Up,
    Cancel,
    CaptureLost,
}

impl WindowsPointerMessage {
    #[must_use]
    pub const fn phase(self) -> PointerPhase {
        match self {
            Self::Down => PointerPhase::Begin,
            Self::Update => PointerPhase::Move,
            Self::Up => PointerPhase::End,
            Self::Cancel | Self::CaptureLost => PointerPhase::Cancel,
        }
    }
}

/// Win32 pen information converted into fixed-width, deterministic Rust data.
///
/// `timestamp_ms` is the original `POINTER_INFO::dwTime` value. It is extended
/// across its 32-bit rollover before being stored as `StylusSample::timestamp_ns`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowsPenEvent {
    pub message: WindowsPointerMessage,
    pub pointer_id: u32,
    pub timestamp_ms: u32,
    pub device_id: u64,
    pub position_client_px: Point,
    /// `None` means the device did not report pressure. The adapter uses 0.5.
    pub pressure: Option<u16>,
    pub tilt: Option<[i16; 2]>,
    pub rotation_degrees: Option<u16>,
    pub buttons: PenButtons,
    pub eraser: bool,
}

/// Classifies the event passed to a desktop bounded queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordedEventKind {
    /// A transition; this must not be dropped by the receiving queue.
    Transition,
    /// A move; this may be coalesced by the receiving queue.
    Move,
}

impl RecordedEventKind {
    #[must_use]
    pub const fn for_phase(phase: PointerPhase) -> Self {
        match phase {
            PointerPhase::Move => Self::Move,
            PointerPhase::Begin | PointerPhase::End | PointerPhase::Cancel => Self::Transition,
        }
    }
}

/// An input item ready for an app's bounded native-input queue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedStylusEvent {
    pub kind: RecordedEventKind,
    pub sample: StylusSample,
}

/// State kept on the native event-loop thread.
///
/// `WindowsPenRecorder` is intentionally not a queue. The desktop app must
/// enqueue [`RecordedStylusEvent::sample`] immediately, preserving every
/// `Transition` and only coalescing `Move` samples.
#[derive(Debug)]
pub struct WindowsPenRecorder {
    next_sequence: u64,
    viewport: ViewportSnapshot,
    timestamp: TimestampExtender,
    active: HashMap<u32, ActivePen>,
}

impl WindowsPenRecorder {
    #[must_use]
    pub fn new(viewport: ViewportSnapshot) -> Self {
        Self {
            next_sequence: 0,
            viewport,
            timestamp: TimestampExtender::default(),
            active: HashMap::new(),
        }
    }

    pub fn set_viewport(&mut self, viewport: ViewportSnapshot) {
        self.viewport = viewport;
    }

    #[must_use]
    pub const fn viewport(&self) -> ViewportSnapshot {
        self.viewport
    }

    /// Converts a fully decoded Windows pen event without invoking Win32.
    ///
    /// This is the deterministic boundary used by synthetic fixtures and by
    /// the Windows message adapter below.
    #[must_use]
    pub fn record(&mut self, event: WindowsPenEvent) -> RecordedStylusEvent {
        let phase = event.message.phase();
        // A stroke owns the viewport in effect at Begin. This prevents a
        // resize/zoom between messages from introducing a teleporting Move or
        // End into the recorded document coordinates.
        let mapping = if phase == PointerPhase::Begin {
            self.viewport
        } else {
            self.active
                .get(&event.pointer_id)
                .map_or(self.viewport, |active| active.viewport)
        };
        let sample = StylusSample {
            sequence: self.take_sequence(),
            timestamp_ns: self.timestamp.extend(event.timestamp_ms),
            device_id: event.device_id,
            phase,
            position_document: mapping.document_point(event.position_client_px),
            pressure: normalized_pressure(event.pressure),
            tilt: event.tilt.map(|[x, y]| [f32::from(x), f32::from(y)]),
            twist_radians: event.rotation_degrees.map(degrees_to_radians),
            tangential_pressure: None,
            buttons: event.buttons,
            eraser: event.eraser,
            viewport_revision: mapping.revision,
        }
        .with_normalized_axes();

        match phase {
            PointerPhase::Begin | PointerPhase::Move => {
                self.active.insert(
                    event.pointer_id,
                    ActivePen {
                        sample,
                        viewport: mapping,
                    },
                );
            }
            PointerPhase::End | PointerPhase::Cancel => {
                self.active.remove(&event.pointer_id);
            }
        }

        RecordedStylusEvent {
            kind: RecordedEventKind::for_phase(phase),
            sample,
        }
    }

    /// Records an OS event only when it can continue a known stroke.
    ///
    /// Windows can deliver hover `WM_POINTERUPDATE` before a pen is in contact.
    /// Such a move has no preceding `Begin` and must not manufacture an active
    /// stroke in the recorder. Transition events are deliberately retained for
    /// their existing fail-closed handling at the app boundary.
    pub fn record_admitted(&mut self, event: WindowsPenEvent) -> Option<RecordedStylusEvent> {
        (event.message != WindowsPointerMessage::Update
            || self.active.contains_key(&event.pointer_id))
        .then(|| self.record(event))
    }

    /// Emits a synthetic cancel when Win32 reports capture loss after pointer
    /// metadata has become unavailable. It preserves the last document point,
    /// device id, buttons, and the viewport revision frozen at Begin.
    #[must_use]
    pub fn cancel_active(
        &mut self,
        pointer_id: u32,
        timestamp_ms: u32,
    ) -> Option<RecordedStylusEvent> {
        let active = self.active.remove(&pointer_id)?;
        let sample = StylusSample {
            sequence: self.take_sequence(),
            timestamp_ns: self.timestamp.extend(timestamp_ms),
            phase: PointerPhase::Cancel,
            ..active.sample
        };

        Some(RecordedStylusEvent {
            kind: RecordedEventKind::Transition,
            sample,
        })
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        sequence
    }
}

#[derive(Clone, Copy, Debug)]
struct ActivePen {
    sample: StylusSample,
    viewport: ViewportSnapshot,
}

#[derive(Debug, Default)]
struct TimestampExtender {
    last_raw_ms: Option<u32>,
    epoch_ms: u64,
}

impl TimestampExtender {
    fn extend(&mut self, timestamp_ms: u32) -> u64 {
        if let Some(last_raw_ms) = self.last_raw_ms
            && timestamp_ms < last_raw_ms
            && last_raw_ms.wrapping_sub(timestamp_ms) > (u32::MAX / 2)
        {
            self.epoch_ms = self.epoch_ms.saturating_add(u64::from(u32::MAX) + 1);
        }
        self.last_raw_ms = Some(timestamp_ms);
        self.epoch_ms
            .saturating_add(u64::from(timestamp_ms))
            .saturating_mul(1_000_000)
    }
}

#[must_use]
fn normalized_pressure(pressure: Option<u16>) -> f32 {
    pressure.map_or(0.5, |value| f32::from(value) / 1024.0)
}

#[must_use]
fn degrees_to_radians(degrees: u16) -> f32 {
    f32::from(degrees % 360) * core::f32::consts::PI / 180.0
}

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    WindowsMessageError, WindowsMouseEvent, WindowsMouseHistoryStatus, WindowsMouseMessage,
    WindowsMouseMessageBuffer, WindowsPenMessageBuffer, record_win32_message,
    record_win32_message_into, record_win32_mouse_message_into, record_wm_pointer,
    record_wm_pointer_into,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn event(message: WindowsPointerMessage) -> WindowsPenEvent {
        WindowsPenEvent {
            message,
            pointer_id: 41,
            timestamp_ms: 10,
            device_id: 7,
            position_client_px: Point { x: 2.0, y: 3.0 },
            pressure: Some(512),
            tilt: Some([-23, 41]),
            rotation_degrees: Some(90),
            buttons: PEN_BUTTON_BARREL,
            eraser: true,
        }
    }

    #[test]
    fn conversion_preserves_pen_axes_buttons_and_viewport_revision() {
        let mut recorder = WindowsPenRecorder::new(ViewportSnapshot {
            revision: 19,
            xx: 2.0,
            xy: 0.0,
            yx: 0.0,
            yy: 3.0,
            tx: 5.0,
            ty: -7.0,
        });

        let converted = recorder.record(event(WindowsPointerMessage::Down));

        assert_eq!(converted.kind, RecordedEventKind::Transition);
        assert_eq!(converted.sample.sequence, 0);
        assert_eq!(converted.sample.timestamp_ns, 10_000_000);
        assert_eq!(converted.sample.position_document, Point { x: 9.0, y: 2.0 });
        assert!((converted.sample.pressure - 0.5).abs() < f32::EPSILON);
        assert_eq!(converted.sample.tilt, Some([-23.0, 41.0]));
        assert!(
            (converted.sample.twist_radians.expect("rotation") - core::f32::consts::FRAC_PI_2)
                .abs()
                < f32::EPSILON
        );
        assert_eq!(converted.sample.buttons, PEN_BUTTON_BARREL);
        assert!(converted.sample.eraser);
        assert_eq!(converted.sample.viewport_revision, 19);
    }

    #[test]
    fn move_is_the_only_droppable_event_kind() {
        let mut recorder = WindowsPenRecorder::new(ViewportSnapshot::identity(0));
        assert_eq!(
            recorder.record(event(WindowsPointerMessage::Down)).kind,
            RecordedEventKind::Transition
        );
        assert_eq!(
            recorder.record(event(WindowsPointerMessage::Update)).kind,
            RecordedEventKind::Move
        );
        assert_eq!(
            recorder.record(event(WindowsPointerMessage::Up)).kind,
            RecordedEventKind::Transition
        );
        assert_eq!(
            recorder
                .record(event(WindowsPointerMessage::CaptureLost))
                .kind,
            RecordedEventKind::Transition
        );
    }

    #[test]
    fn move_without_begin_does_not_create_an_active_pen() {
        let mut recorder = WindowsPenRecorder::new(ViewportSnapshot::identity(0));
        assert!(
            recorder
                .record_admitted(event(WindowsPointerMessage::Update))
                .is_none()
        );
        assert!(recorder.cancel_active(41, 11).is_none());

        assert!(
            recorder
                .record_admitted(event(WindowsPointerMessage::Down))
                .is_some()
        );
        assert!(
            recorder
                .record_admitted(event(WindowsPointerMessage::Update))
                .is_some()
        );
    }

    #[test]
    fn capture_loss_without_pointer_metadata_cancels_the_last_active_sample() {
        let mut recorder = WindowsPenRecorder::new(ViewportSnapshot::identity(3));
        let _ = recorder.record(event(WindowsPointerMessage::Down));
        recorder.set_viewport(ViewportSnapshot::identity(4));

        let cancel = recorder.cancel_active(41, 12).expect("active pen");

        assert_eq!(cancel.kind, RecordedEventKind::Transition);
        assert_eq!(cancel.sample.phase, PointerPhase::Cancel);
        assert_eq!(cancel.sample.sequence, 1);
        assert_eq!(cancel.sample.timestamp_ns, 12_000_000);
        assert_eq!(cancel.sample.position_document, Point { x: 2.0, y: 3.0 });
        assert_eq!(cancel.sample.viewport_revision, 3);
        assert!(recorder.cancel_active(41, 13).is_none());

        let mut recorder = WindowsPenRecorder::new(ViewportSnapshot::identity(3));
        let _ = recorder.record(event(WindowsPointerMessage::Down));
        recorder.set_viewport(ViewportSnapshot {
            revision: 4,
            tx: 100.0,
            ..ViewportSnapshot::identity(4)
        });
        let end = recorder.record(event(WindowsPointerMessage::Up));
        assert_eq!(end.sample.viewport_revision, 3);
        assert_eq!(end.sample.position_document, Point { x: 2.0, y: 3.0 });

        let _ = recorder.record(event(WindowsPointerMessage::Down));
        recorder.set_viewport(ViewportSnapshot::identity(5));
        let replacement_begin = recorder.record(event(WindowsPointerMessage::Down));
        assert_eq!(replacement_begin.sample.viewport_revision, 5);
    }

    #[test]
    fn timestamp_extension_is_monotonic_across_u32_rollover() {
        let mut recorder = WindowsPenRecorder::new(ViewportSnapshot::identity(0));
        let mut first = event(WindowsPointerMessage::Down);
        first.timestamp_ms = u32::MAX - 1;
        let mut second = event(WindowsPointerMessage::Update);
        second.timestamp_ms = 2;

        let before = recorder.record(first).sample.timestamp_ns;
        let after = recorder.record(second).sample.timestamp_ns;

        assert_eq!(after - before, 4_000_000);
    }

    #[test]
    fn absent_pressure_uses_neutral_default_and_large_pressure_is_clamped() {
        assert!((normalized_pressure(None) - 0.5).abs() < f32::EPSILON);
        let mut recorder = WindowsPenRecorder::new(ViewportSnapshot::identity(0));
        let mut raw = event(WindowsPointerMessage::Update);
        raw.pressure = Some(4_096);
        assert!((recorder.record(raw).sample.pressure - 1.0).abs() < f32::EPSILON);
    }
}
