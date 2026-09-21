#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceAppearance {
    pub checkerboard: bool,
    pub solid_rgb: [u8; 3],
}

impl Default for WorkspaceAppearance {
    fn default() -> Self {
        Self {
            checkerboard: true,
            solid_rgb: [54; 3],
        }
    }
}

impl WorkspaceAppearance {
    #[must_use]
    pub fn background_colors(self) -> [[u8; 3]; 2] {
        if self.checkerboard {
            [[188; 3], [172; 3]]
        } else {
            [self.solid_rgb; 2]
        }
    }

    #[must_use]
    pub fn solid_hex(self) -> String {
        let [red, green, blue] = self.solid_rgb;
        format!("#{red:02x}{green:02x}{blue:02x}")
    }

    pub fn set_solid_hex(&mut self, value: &str) {
        if value.len() == 7
            && let Some(hex) = value.strip_prefix('#')
            && let Ok(rgb) = u32::from_str_radix(hex, 16)
        {
            let [_, red, green, blue] = rgb.to_be_bytes();
            self.solid_rgb = [red, green, blue];
        }
    }
}
