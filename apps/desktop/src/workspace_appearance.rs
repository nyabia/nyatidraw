use std::{fs::File, io::Read};

const MAGIC: &[u8; 8] = b"NYVIEW01";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceAppearance {
    pub(crate) checkerboard: bool,
    pub(crate) solid_rgb: [u8; 3],
    writable: bool,
}

impl Default for WorkspaceAppearance {
    fn default() -> Self {
        Self {
            checkerboard: true,
            solid_rgb: [54; 3],
            writable: true,
        }
    }
}

impl WorkspaceAppearance {
    pub(crate) fn load() -> Self {
        let Some(path) =
            crate::layout_store::settings_path().map(|path| path.with_extension("appearance"))
        else {
            return Self {
                writable: false,
                ..Self::default()
            };
        };
        let loaded = (|| -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            File::open(path)?.take(13).read_to_end(&mut bytes)?;
            Ok(bytes)
        })();
        match loaded {
            Ok(bytes) if bytes.len() == 12 && bytes.starts_with(MAGIC) && bytes[8] <= 1 => Self {
                checkerboard: bytes[8] == 1,
                solid_rgb: [bytes[9], bytes[10], bytes[11]],
                writable: true,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            _ => {
                eprintln!("native-appearance event=load-failed existing-preferences-preserved");
                Self {
                    writable: false,
                    ..Self::default()
                }
            }
        }
    }

    pub(crate) fn encode(self) -> Option<Vec<u8>> {
        if !self.writable {
            return None;
        }
        let mut bytes = MAGIC.to_vec();
        bytes.push(u8::from(self.checkerboard));
        bytes.extend(self.solid_rgb);
        Some(bytes)
    }

    pub(crate) fn background_colors(self) -> [[u8; 3]; 2] {
        if self.checkerboard {
            [[188; 3], [172; 3]]
        } else {
            [self.solid_rgb; 2]
        }
    }
}
