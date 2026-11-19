use crate::PalettePins as Pins;

const CAPACITY: usize = 10;

#[derive(Clone, Copy)]
pub struct PaletteEntry {
    pub color: [u8; 4],
    pub pinned: bool,
    used: u64,
}

#[derive(Clone)]
pub struct Palette {
    pub slots: [Option<PaletteEntry>; CAPACITY],
    observed: Vec<[u8; 4]>,
    clock: u64,
}

impl Palette {
    #[must_use]
    pub fn matches_recent(&self, incoming: &[[u8; 4]]) -> bool {
        self.observed == incoming[..incoming.len().min(CAPACITY)]
    }

    #[must_use]
    pub fn new(pins: Pins) -> Self {
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

    pub fn observe(&mut self, incoming: &[[u8; 4]]) {
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

    pub fn toggle_pin(&mut self, index: usize, color: [u8; 4]) -> Option<Pins> {
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
