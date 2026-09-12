use crate::live_ink::LiveInkBridge;
use dioxus::prelude::*;
use nyatidraw_api::{EditorCommand, ToolCommand, UiProjection};

#[path = "palette_preferences.rs"]
mod preferences;
use preferences::{CAPACITY, PalettePreferences, Pins};

#[derive(Clone, Copy)]
struct PaletteEntry {
    color: [u8; 4],
    pinned: bool,
    used: u64,
}

struct Palette {
    slots: [Option<PaletteEntry>; CAPACITY],
    observed: Vec<[u8; 4]>,
    clock: u64,
}

impl Palette {
    fn new(pins: Pins) -> Self {
        Self {
            slots: pins.map(|pin| {
                pin.map(|color| PaletteEntry {
                    color,
                    pinned: true,
                    used: 0,
                })
            }),
            observed: Vec::new(),
            clock: 0,
        }
    }

    fn observe(&mut self, incoming: &[[u8; 4]]) {
        let incoming = &incoming[..incoming.len().min(CAPACITY)];
        if self.observed == incoming {
            return;
        }
        // Native MRU order remains the only source of usage. UI renders and pin
        // changes never manufacture color selections or artwork commands.
        // Only replay the promoted prefix. Replaying the entire MRU would
        // reinsert already-evicted colors when pins occupy some of the slots.
        let promoted = (0..=incoming.len())
            .find(|&count| {
                self.observed
                    .iter()
                    .filter(|color| !incoming[..count].contains(color))
                    .take(incoming.len() - count)
                    .copied()
                    .eq(incoming[count..].iter().copied())
            })
            .unwrap_or(incoming.len());
        for &color in incoming[..promoted].iter().rev() {
            if color[3] == 0 {
                continue;
            }
            self.clock = self.clock.saturating_add(1);
            let slot = self
                .slots
                .iter()
                .position(|entry| entry.is_some_and(|entry| entry.color == color))
                .or_else(|| self.slots.iter().position(Option::is_none))
                .or_else(|| {
                    self.slots
                        .iter()
                        .enumerate()
                        .filter_map(|(index, entry)| {
                            entry
                                .filter(|entry| !entry.pinned)
                                .map(|entry| (index, entry.used))
                        })
                        .min_by_key(|(_, used)| *used)
                        .map(|(index, _)| index)
                });
            if let Some(index) = slot {
                let pinned = self.slots[index].is_some_and(|entry| entry.pinned);
                self.slots[index] = Some(PaletteEntry {
                    color,
                    pinned,
                    used: self.clock,
                });
            }
        }
        self.order_recent_slots();
        self.observed = incoming.to_vec();
    }

    fn order_recent_slots(&mut self) {
        let mut recent: Vec<_> = self
            .slots
            .iter()
            .flatten()
            .copied()
            .filter(|entry| !entry.pinned)
            .collect();
        recent.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.used));
        let mut recent = recent.into_iter();
        for slot in &mut self.slots {
            if !slot.is_some_and(|entry| entry.pinned) {
                *slot = recent.next();
            }
        }
    }

    fn toggle_pin(&mut self, index: usize, color: [u8; 4]) -> Option<Pins> {
        let entry = self.slots.get_mut(index)?.as_mut()?;
        if entry.color != color {
            return None;
        }
        entry.pinned = !entry.pinned;
        self.order_recent_slots();
        Some(
            self.slots
                .map(|entry| entry.filter(|entry| entry.pinned).map(|entry| entry.color)),
        )
    }
}

#[derive(Clone)]
struct PaletteContext {
    state: Signal<Palette>,
    preferences: PalettePreferences,
    notice: Signal<Option<String>>,
}

/// Call once in the app root after creating its authoritative UI projection.
pub(crate) fn use_palette_preferences(ui_projection: Signal<UiProjection>) {
    let (preferences, pins) = use_hook(|| PalettePreferences::open(dioxus_core::schedule_update()));
    let mut context = use_context_provider(|| PaletteContext {
        state: Signal::new(Palette::new(pins)),
        preferences: preferences.clone(),
        notice: Signal::new(preferences.notice()),
    });
    let notice = preferences.notice();
    if *context.notice.peek() != notice {
        context.notice.set(notice);
    }
    use_effect(move || {
        let incoming = ui_projection.read().recent_colors.clone();
        if context.state.peek().observed != incoming {
            context.state.write().observe(&incoming);
        }
    });
}

#[component]
pub(crate) fn ColorPanel(ui_projection: Signal<UiProjection>) -> Element {
    let current = ui_projection.read().brush_color;
    rsx! {
        div { class: "color-panel compact-color-panel",
            crate::color_picker::ColorPicker { color: current }
            PaletteRow { current, compact: false }
        }
    }
}

#[component]
pub(crate) fn QuickColors(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    let current = ui_projection.read().brush_color;
    let background = ui_projection.read().background_color;
    let front = css_color(current);
    let back = css_color(background);
    rsx! {
        button { class: "color-stack", title: "전경/배경 색상 교환 (X)", aria_label: "전경/배경 색상 교환",
            onclick: move |_| crate::send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SwapColors), error),
            span { class: "color-chip back", style: "background:{back}" }
            span { class: "color-chip front", style: "background:{front}" }
            sub { class: "color-swap-shortcut", "x" }
        }
        PaletteRow { current, compact: true }
    }
}

#[component]
fn PaletteRow(current: [u8; 4], compact: bool) -> Element {
    let context = use_context::<PaletteContext>();
    let slots = context.state.read().slots;
    let notice = context.notice.read().clone();
    rsx! {
        div { class: if compact { "palette-row palette-row-top" } else { "palette-row palette-row-sidebar" },
            aria_label: "최근 색상과 고정 색상",
            for (index, entry) in slots.into_iter().enumerate() {
                if let Some(entry) = entry {
                    PaletteSwatch { key: "{index}", index, color: entry.color, pinned: entry.pinned, selected: entry.color == current }
                } else {
                    span { key: "{index}", class: "palette-empty", aria_hidden: "true" }
                }
            }
            if let Some(message) = notice {
                span { class: "palette-notice", role: "status", title: "{message}", aria_label: "{message}", "!" }
            }
        }
    }
}

#[component]
fn PaletteSwatch(index: usize, color: [u8; 4], pinned: bool, selected: bool) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let context = use_context::<PaletteContext>();
    let mut state = context.state;
    let preferences = context.preferences;
    let error = use_signal(|| Option::<String>::None);
    let background = css_color(color);
    let label = format!("색상 {}, {}, {}", color[0], color[1], color[2]);
    let pin_label = if pinned {
        "색상 고정 해제"
    } else {
        "색상 고정"
    };
    rsx! {
        div { class: if pinned { "palette-slot pinned" } else { "palette-slot" },
            button { class: "palette-swatch", style: "background:{background}", title: "{label}", aria_label: "{label}", aria_pressed: selected,
                onclick: move |_| crate::send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetColor(color)), error),
            }
            button { class: "palette-pin", title: "{pin_label}", aria_label: "{pin_label}", aria_pressed: pinned,
                onclick: move |event| {
                    event.stop_propagation();
                    if let Some(pins) = state.write().toggle_pin(index, color) { preferences.save(pins); }
                },
                svg { view_box: "0 0 16 16", "aria-hidden": "true",
                    path { d: "M5 2h6l-1 5 2 2v1H9v4l-1 1-1-1v-4H4V9l2-2z" }
                }
            }
        }
    }
}

fn css_color(color: [u8; 4]) -> String {
    format!(
        "rgba({},{},{},{})",
        color[0],
        color[1],
        color[2],
        f64::from(color[3]) / 255.0
    )
}
