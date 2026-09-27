#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum CanvasInputMode {
    #[default]
    Auto = 0,
    Pen = 1,
    Finger = 2,
}

impl CanvasInputMode {
    #[must_use]
    pub const fn storage_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Pen => "pen",
            Self::Finger => "finger",
        }
    }

    #[must_use]
    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "pen" => Some(Self::Pen),
            "finger" => Some(Self::Finger),
            _ => None,
        }
    }

    #[must_use]
    pub fn allows_touch_begin(self, pen_contact: bool, elapsed_since_pen_ms: Option<u64>) -> bool {
        !pen_contact
            && match self {
                Self::Auto => elapsed_since_pen_ms.is_none_or(|elapsed| elapsed >= 700),
                Self::Pen => false,
                Self::Finger => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::CanvasInputMode::{Auto, Finger, Pen};

    #[test]
    fn pen_contact_and_cooldown_prevent_accidental_touch_strokes() {
        for (mode, contact, elapsed, allowed) in [
            (Auto, false, None, true),
            (Auto, false, Some(699), false),
            (Auto, false, Some(700), true),
            (Auto, true, Some(5_000), false),
            (Pen, false, None, false),
            (Finger, false, Some(0), true),
            (Finger, true, Some(5_000), false),
        ] {
            assert_eq!(mode.allows_touch_begin(contact, elapsed), allowed);
        }
    }
}
