use std::{
    fs::File,
    io::Read as _,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use nyatidraw_api::{EditSettings, EditSource, FillSettings, PencilTemplate, SelectionMode};

use super::{BrushSettings, DrawingConfig, DrawingTool, LiveInkBridge, ProjectDb, RememberedBrush};

const MAGIC: &[u8; 8] = b"NYTOOL01";
const RECORD_BYTES: usize = 85;
const TOOLS: [DrawingTool; 12] = [
    DrawingTool::Move,
    DrawingTool::MoveSelection,
    DrawingTool::Pencil,
    DrawingTool::Pen,
    DrawingTool::Brush,
    DrawingTool::Eraser,
    DrawingTool::Wand,
    DrawingTool::Lasso,
    DrawingTool::RectangleSelection,
    DrawingTool::Eyedropper,
    DrawingTool::Fill,
    DrawingTool::Gradient,
];

pub(super) struct Store {
    project_writable: bool,
    initialize_project: bool,
    global_path: Option<PathBuf>,
    failed_notice_sent: bool,
}

#[derive(Clone, Default)]
pub(super) struct Pending(Arc<Mutex<Option<DrawingConfig>>>);

impl Pending {
    pub(super) fn submit(&self, drawing: DrawingConfig) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(drawing)
            .is_none()
    }

    pub(super) fn flush(&self, store: &mut Store, db: &ProjectDb, live_ink: &LiveInkBridge) {
        let drawing = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(drawing) = drawing {
            store.persist(db, drawing, live_ink);
        }
    }
}

#[cfg(not(test))]
fn global_settings_path() -> Option<PathBuf> {
    crate::layout_store::settings_path().map(|path| path.with_extension("tools"))
}

#[cfg(test)]
fn global_settings_path() -> Option<PathBuf> {
    None
}

impl Store {
    pub(super) fn load(db: &ProjectDb, live_ink: &LiveInkBridge) -> (Self, Option<DrawingConfig>) {
        let global_path = global_settings_path();
        let global = global_path.as_ref().map_or(Ok(None), |path| {
            let file = match File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.to_string()),
            };
            let mut bytes = Vec::new();
            file.take((RECORD_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            decode(&bytes)
                .map(Some)
                .ok_or_else(|| "invalid global tool preferences".into())
        });
        let project = db
            .load_editor_tool_state()
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                bytes
                    .map(|bytes| {
                        decode(&bytes).ok_or_else(|| "invalid project tool preferences".into())
                    })
                    .transpose()
            });
        if project.is_err() || global.is_err() {
            live_ink.publish_activation_notice(
                "일부 도구 설정을 읽지 못했습니다. 사용 가능한 설정으로 열었으며 기존 값은 보존했습니다."
                    .into(),
            );
        }
        let store = Self {
            project_writable: project.is_ok(),
            initialize_project: matches!(project, Ok(None)),
            global_path: global.is_ok().then_some(global_path).flatten(),
            failed_notice_sent: false,
        };
        (
            store,
            project.ok().flatten().or_else(|| global.ok().flatten()),
        )
    }

    pub(super) fn initialize(
        &mut self,
        db: &ProjectDb,
        drawing: DrawingConfig,
        live_ink: &LiveInkBridge,
    ) {
        if self.initialize_project {
            self.initialize_project = false;
            if db.persist_editor_tool_state(&encode(drawing)).is_err() {
                self.notify_failure(live_ink);
            }
        }
    }

    pub(super) fn persist(
        &mut self,
        db: &ProjectDb,
        drawing: DrawingConfig,
        live_ink: &LiveInkBridge,
    ) {
        let bytes = encode(drawing);
        let project_failed = self.project_writable && db.persist_editor_tool_state(&bytes).is_err();
        let global_failed = self
            .global_path
            .as_ref()
            .is_some_and(|path| crate::layout_store::replace(path, &bytes).is_err());
        if (project_failed || global_failed) && !self.failed_notice_sent {
            self.notify_failure(live_ink);
        }
    }

    fn notify_failure(&mut self, live_ink: &LiveInkBridge) {
        self.failed_notice_sent = true;
        live_ink.publish_activation_notice(
            "도구 설정을 저장하지 못했습니다. 그림 저장과는 별개이며 현재 작업은 계속할 수 있습니다.".into(),
        );
    }
}

pub(super) fn encode(mut drawing: DrawingConfig) -> Vec<u8> {
    drawing.remember_current();
    let mut bytes = Vec::with_capacity(RECORD_BYTES);
    bytes.extend_from_slice(MAGIC);
    bytes.push(u8::try_from(TOOLS.iter().position(|tool| *tool == drawing.tool).unwrap()).unwrap());
    bytes.push(u8::from(
        drawing.pencil_template == PencilTemplate::Graphite2B,
    ));
    bytes.push(u8::try_from(drawing.last_painting_slot).unwrap());
    bytes.extend_from_slice(&drawing.color);
    bytes.extend_from_slice(&drawing.background_color);
    bytes.extend_from_slice(&[
        match drawing.edit_settings.source {
            EditSource::ActiveLayer => 0,
            EditSource::ReferenceLayers => 1,
            EditSource::AllVisible => 2,
        },
        drawing.edit_settings.tolerance,
        match drawing.edit_settings.selection_mode {
            SelectionMode::Replace => 0,
            SelectionMode::Add => 1,
            SelectionMode::Subtract => 2,
        },
        drawing.edit_settings.fill.gap_close_px,
        drawing.edit_settings.fill.expand_px,
        u8::from(drawing.edit_settings.fill.antialias),
    ]);
    for brush in drawing.remembered {
        bytes.extend_from_slice(&brush.size_tenths.to_le_bytes());
        bytes.extend_from_slice(&brush.opacity_u16.to_le_bytes());
        bytes.push(
            u8::from(brush.settings.size_pressure)
                | (u8::from(brush.settings.opacity_pressure) << 1),
        );
        bytes.extend_from_slice(&brush.settings.size_minimum_u16.to_le_bytes());
        bytes.extend_from_slice(&brush.settings.opacity_minimum_u16.to_le_bytes());
        bytes.extend_from_slice(&brush.settings.hardness_u16.to_le_bytes());
        bytes.push(brush.settings.smoothing);
    }
    debug_assert_eq!(bytes.len(), RECORD_BYTES);
    bytes
}

pub(super) fn decode(bytes: &[u8]) -> Option<DrawingConfig> {
    if bytes.len() != RECORD_BYTES || !bytes.starts_with(MAGIC) {
        return None;
    }
    let tool = *TOOLS.get(usize::from(bytes[8]))?;
    let pencil_template = match bytes[9] {
        0 => PencilTemplate::Mechanical2H,
        1 => PencilTemplate::Graphite2B,
        _ => return None,
    };
    let last_painting_slot = usize::from(bytes[10]);
    let color: [u8; 4] = bytes[11..15].try_into().ok()?;
    let background_color: [u8; 4] = bytes[15..19].try_into().ok()?;
    if color[3] == 0 || background_color[3] == 0 || last_painting_slot >= 5 {
        return None;
    }
    let expected_slot = DrawingConfig::painting_slot(tool).map(|slot| {
        if tool == DrawingTool::Pencil && pencil_template == PencilTemplate::Graphite2B {
            4
        } else {
            slot
        }
    });
    if expected_slot.is_some_and(|slot| slot != last_painting_slot) {
        return None;
    }
    let edit_settings = EditSettings {
        source: match bytes[19] {
            0 => EditSource::ActiveLayer,
            1 => EditSource::ReferenceLayers,
            2 => EditSource::AllVisible,
            _ => return None,
        },
        tolerance: bytes[20],
        selection_mode: match bytes[21] {
            0 => SelectionMode::Replace,
            1 => SelectionMode::Add,
            2 => SelectionMode::Subtract,
            _ => return None,
        },
        fill: FillSettings {
            gap_close_px: bytes[22],
            expand_px: bytes[23],
            antialias: match bytes[24] {
                0 => false,
                1 => true,
                _ => return None,
            },
        },
    };
    if edit_settings.fill.gap_close_px > 8 || edit_settings.fill.expand_px > 64 {
        return None;
    }
    let mut remembered = [RememberedBrush::for_tool(DrawingTool::Pencil); 5];
    for (slot, encoded) in remembered.iter_mut().zip(bytes[25..].chunks_exact(12)) {
        let number = |index| u16::from_le_bytes([encoded[index], encoded[index + 1]]);
        *slot = RememberedBrush {
            size_tenths: number(0),
            opacity_u16: number(2),
            settings: BrushSettings {
                size_pressure: encoded[4] & 1 != 0,
                opacity_pressure: encoded[4] & 2 != 0,
                size_minimum_u16: number(5),
                opacity_minimum_u16: number(7),
                hardness_u16: number(9),
                smoothing: encoded[11],
            },
        };
        if !(1..=2_000).contains(&slot.size_tenths)
            || slot.opacity_u16 == 0
            || encoded[4] > 3
            || slot.settings.smoothing > 100
        {
            return None;
        }
    }
    let active = remembered[last_painting_slot];
    Some(DrawingConfig {
        edit_settings,
        tool,
        pencil_template,
        remembered,
        last_painting_slot,
        size_tenths: active.size_tenths,
        opacity_u16: active.opacity_u16,
        brush_settings: active.settings,
        color,
        background_color,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_brush_parameters_cannot_reopen_as_a_different_or_invalid_brush() {
        let mut drawing = DrawingConfig::from_projection(&nyatidraw_api::UiProjection::empty());
        drawing.color = [21, 42, 63, 255];
        drawing.background_color = [65, 43, 21, 255];
        drawing.pencil_template = PencilTemplate::Graphite2B;
        for (index, tool) in [
            DrawingTool::Pencil,
            DrawingTool::Pen,
            DrawingTool::Brush,
            DrawingTool::Eraser,
        ]
        .into_iter()
        .enumerate()
        {
            drawing.select_tool(tool);
            drawing.size_tenths = 10 + u16::try_from(index).unwrap();
            drawing.opacity_u16 = 40_000 + u16::try_from(index).unwrap();
            drawing.brush_settings.size_minimum_u16 = 15_000;
            drawing.brush_settings.opacity_minimum_u16 = 7_000;
            drawing.brush_settings.hardness_u16 = 32_000;
            drawing.brush_settings.smoothing = 37;
            drawing.remember_current();
        }
        for tool in TOOLS {
            drawing.select_tool(tool);
            assert_eq!(decode(&encode(drawing)), Some(drawing));
        }
        let valid = encode(drawing);
        for length in 0..valid.len() {
            assert!(decode(&valid[..length]).is_none());
        }
        for (index, value) in [
            (0, 0),
            (8, 255),
            (9, 2),
            (10, 5),
            (14, 0),
            (18, 0),
            (19, 3),
            (21, 3),
            (22, 9),
            (23, 65),
            (24, 2),
            (29, 4),
            (36, 101),
        ] {
            let mut malformed = valid.clone();
            malformed[index] = value;
            assert!(decode(&malformed).is_none(), "invalid field {index}");
        }
        let mut unsupported = valid;
        unsupported.push(0);
        assert!(decode(&unsupported).is_none());
    }

    #[test]
    fn corrupt_optional_preferences_preserve_artwork_access_and_the_original_record() {
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-tool-state-{}-{}.ntdr",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let drawing = DrawingConfig::from_projection(&nyatidraw_api::UiProjection::empty());
        let db = ProjectDb::open(&path).unwrap();
        let bridge = LiveInkBridge::with_capacity(16, nyatidraw_api::LayerId(1));
        let (mut store, loaded) = Store::load(&db, &bridge);
        assert!(loaded.is_none());
        assert!(
            store.global_path.is_none(),
            "tests must never touch user preferences"
        );
        store.initialize(&db, drawing, &bridge);
        assert!(db.load_current().unwrap().is_none());
        assert_eq!(
            decode(&db.load_editor_tool_state().unwrap().unwrap()),
            Some(drawing)
        );
        let changed = DrawingConfig {
            color: [123, 45, 67, 255],
            ..drawing
        };
        store.persist(&db, changed, &bridge);
        drop(db);
        let db = ProjectDb::open(&path).unwrap();
        assert_eq!(Store::load(&db, &bridge).1, Some(changed));
        db.persist_editor_tool_state(b"unknown future settings")
            .unwrap();
        drop(db);
        let db = ProjectDb::open(&path).unwrap();
        let (mut store, loaded) = Store::load(&db, &bridge);
        assert!(loaded.is_none());
        store.initialize(&db, drawing, &bridge);
        store.persist(&db, drawing, &bridge);
        assert_eq!(
            db.load_editor_tool_state().unwrap().unwrap(),
            b"unknown future settings"
        );
        assert!(db.load_reopened().unwrap().is_none());
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
}
