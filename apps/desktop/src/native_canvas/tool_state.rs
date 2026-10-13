use std::{
    fs::File,
    io::Read as _,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use nyatidraw_project::{
    EDITOR_TOOL_STATE_BYTES as RECORD_BYTES, ProjectBrushState, ProjectToolState,
    decode_editor_tool_state, encode_editor_tool_state,
};

use super::{DrawingConfig, LiveInkBridge, ProjectDb, RememberedBrush};
use nyatidraw_api::LayerId;

#[cfg(test)]
use nyatidraw_api::{DrawingTool, PencilTemplate};
#[cfg(test)]
const TOOLS: [DrawingTool; 13] = [
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
    DrawingTool::MoveArtwork,
];

pub(super) struct Store {
    project_writable: bool,
    initialize_project: bool,
    global_path: Option<PathBuf>,
    failed_notice_sent: bool,
    active_layer: Result<Option<LayerId>, ()>,
}

#[derive(Clone, Default)]
pub(super) struct Pending(Arc<Mutex<PendingValues>>);

#[derive(Default)]
struct PendingValues {
    presets: Option<(PathBuf, Vec<u8>)>,
    drawing: Option<DrawingConfig>,
    active_layer: Option<LayerId>,
    last_submitted_layer: Option<LayerId>,
}

impl Pending {
    pub(super) fn submit_presets(&self, path: PathBuf, bytes: Vec<u8>) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .presets
            .replace((path, bytes))
            .is_none()
    }
    pub(super) fn submit(&self, drawing: DrawingConfig) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drawing
            .replace(drawing)
            .is_none()
    }

    pub(super) fn submit_active_layer(&self, layer: LayerId) -> bool {
        let mut pending = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.last_submitted_layer == Some(layer) {
            return false;
        }
        pending.last_submitted_layer = Some(layer);
        pending.active_layer.replace(layer).is_none()
    }

    pub(super) fn flush(&self, store: &mut Store, db: &ProjectDb, live_ink: &LiveInkBridge) {
        let (drawing, layer, presets) = {
            let mut pending = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                pending.drawing.take(),
                pending.active_layer.take(),
                pending.presets.take(),
            )
        };
        if let Some(drawing) = drawing {
            store.persist(db, drawing, live_ink);
        }
        if let Some(layer) = layer {
            store.persist_active_layer(db, layer, live_ink);
        }
        if let Some((path, bytes)) = presets
            && crate::layout_store::replace(&path, &bytes).is_err()
        {
            live_ink.publish_activation_notice(
                "브러시 프리셋을 저장하지 못했습니다. 그림 저장과는 별개입니다.".into(),
            );
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

pub(super) struct PresetLibrary {
    pub(super) entries: Vec<nyatidraw_api::NamedBrushPreset>,
    pub(super) path: Option<PathBuf>,
    pub(super) writable: bool,
}

impl PresetLibrary {
    pub(super) fn load(live_ink: &LiveInkBridge) -> Self {
        Self::load_path(
            global_settings_path().map(|p| p.with_extension("brushes")),
            live_ink,
        )
    }

    fn load_path(path: Option<PathBuf>, live_ink: &LiveInkBridge) -> Self {
        let result = path.as_ref().map_or(Ok(Vec::new()), |path| {
            let file = match File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(error.to_string()),
            };
            let mut bytes = Vec::new();
            file.take((nyatidraw_api::BRUSH_PRESETS_MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            nyatidraw_api::decode_brush_presets(&bytes)
                .ok_or_else(|| "invalid brush presets".into())
        });
        let writable = result.is_ok();
        if !writable {
            live_ink.publish_activation_notice("브러시 프리셋을 읽지 못했습니다. 원본을 보존하며 이번 실행에서는 저장하지 않습니다.".into());
        }
        Self {
            entries: result.unwrap_or_default(),
            path,
            writable,
        }
    }
}

impl Store {
    pub(super) fn load(db: &ProjectDb, live_ink: &LiveInkBridge) -> (Self, Option<DrawingConfig>) {
        Self::load_with_global_path(db, live_ink, global_settings_path())
    }

    fn load_with_global_path(
        db: &ProjectDb,
        live_ink: &LiveInkBridge,
        global_path: Option<PathBuf>,
    ) -> (Self, Option<DrawingConfig>) {
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
        let active_layer = db.load_active_layer();
        if active_layer.is_err() {
            live_ink.publish_activation_notice("마지막 레이어 정보를 읽지 못해 첫 레이어를 선택했습니다. 기존 정보와 그림은 보존했습니다.".into());
        }
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
            active_layer: active_layer.map_err(|_| ()),
        };
        (
            store,
            project.ok().flatten().or_else(|| global.ok().flatten()),
        )
    }

    pub(super) fn active_layer(&self) -> Option<LayerId> {
        self.active_layer.ok().flatten()
    }

    fn persist_active_layer(&mut self, db: &ProjectDb, layer: LayerId, live_ink: &LiveInkBridge) {
        if self.active_layer.is_err() || self.active_layer == Ok(Some(layer)) {
            return;
        }
        match db.persist_active_layer(layer) {
            Ok(()) => self.active_layer = Ok(Some(layer)),
            Err(_) if !self.failed_notice_sent => {
                self.failed_notice_sent = true;
                live_ink.publish_activation_notice("마지막 레이어를 저장하지 못했습니다. 그림 저장과는 별개이며 작업은 계속할 수 있습니다.".into());
            }
            Err(_) => {}
        }
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
    encode_editor_tool_state(&ProjectToolState {
        tool: drawing.tool,
        pencil_template: drawing.pencil_template,
        last_painting_slot: u8::try_from(drawing.last_painting_slot).unwrap(),
        color: drawing.color,
        background_color: drawing.background_color,
        edit_settings: drawing.edit_settings,
        remembered: drawing.remembered.map(|brush| ProjectBrushState {
            size_tenths: brush.size_tenths,
            opacity_u16: brush.opacity_u16,
            settings: brush.settings,
        }),
    })
}

pub(super) fn decode(bytes: &[u8]) -> Option<DrawingConfig> {
    let state = decode_editor_tool_state(bytes)?;
    let last_painting_slot = usize::from(state.last_painting_slot);
    let remembered = state.remembered.map(|brush| RememberedBrush {
        size_tenths: brush.size_tenths,
        opacity_u16: brush.opacity_u16,
        settings: brush.settings,
    });
    let active = remembered[last_painting_slot];
    Some(DrawingConfig {
        edit_settings: state.edit_settings,
        tool: state.tool,
        pencil_template: state.pencil_template,
        remembered,
        last_painting_slot,
        size_tenths: active.size_tenths,
        opacity_u16: active.opacity_u16,
        brush_settings: active.settings,
        color: state.color,
        background_color: state.background_color,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_brush_library_restores_exact_settings_without_changing_project_or_other_tools() {
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-presets-{}-{}.ntdr",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let library_path = path.with_extension("brushes");
        let bridge = LiveInkBridge::with_capacity(16, LayerId(1));
        let db = ProjectDb::open(&path).unwrap();
        let mut drawing = DrawingConfig::from_projection(&nyatidraw_api::UiProjection::empty());
        drawing.select_tool(DrawingTool::Pencil);
        drawing.pencil_template = PencilTemplate::Graphite2B;
        drawing.select_tool(DrawingTool::Pencil);
        drawing.size_tenths = 173;
        drawing.opacity_u16 = 41_234;
        drawing.brush_settings.smoothing = 73;
        drawing.brush_settings.size_minimum_u16 = 4_321;
        let captured = drawing.brush_preset_values();
        let preset = nyatidraw_api::NamedBrushPreset {
            name: nyatidraw_api::BrushPresetName::new("Saved 2B").unwrap(),
            values: captured,
        };
        let pending = Pending::default();
        assert!(pending.submit_presets(
            library_path.clone(),
            nyatidraw_api::encode_brush_presets(&[])
        ));
        assert!(!pending.submit_presets(
            library_path.clone(),
            nyatidraw_api::encode_brush_presets(&[preset])
        ));
        let (mut store, _) = Store::load(&db, &bridge);
        pending.flush(&mut store, &db, &bridge);
        let loaded = PresetLibrary::load_path(Some(library_path.clone()), &bridge);
        assert!(loaded.writable);
        assert_eq!(loaded.entries, vec![preset]);
        drawing.select_tool(DrawingTool::Pen);
        let untouched_pen = drawing.remembered[1];
        let color = drawing.color;
        let edit = drawing.edit_settings;
        drawing.apply_brush_preset(loaded.entries[0].values);
        assert_eq!(drawing.brush_preset_values(), captured);
        assert_eq!(drawing.remembered[1], untouched_pen);
        assert_eq!(drawing.color, color);
        assert_eq!(drawing.edit_settings, edit);
        assert!(db.load_editor_tool_state().unwrap().is_none());
        assert!(db.load_current().unwrap().is_none());
        drop(db);
        let original = b"NYBRSH99future";
        std::fs::write(&library_path, original).unwrap();
        let corrupt = PresetLibrary::load_path(Some(library_path.clone()), &bridge);
        assert!(!corrupt.writable);
        assert_eq!(std::fs::read(&library_path).unwrap(), original);
        std::fs::remove_file(&library_path).unwrap();
        std::fs::remove_file(path).unwrap();
    }

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
    fn project_brush_state_wins_and_saving_refreshes_both_stores_without_artwork_changes() {
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-tool-priority-{}-{}.ntdr",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let global = path.with_extension("tools");
        let drawing = DrawingConfig::from_projection(&nyatidraw_api::UiProjection::empty());
        let other = DrawingConfig {
            size_tenths: 170,
            color: [123, 45, 67, 255],
            ..drawing
        };
        std::fs::write(&global, encode(other)).unwrap();
        let bridge = LiveInkBridge::with_capacity(16, nyatidraw_api::LayerId(1));
        let db = ProjectDb::open(&path).unwrap();
        assert_eq!(
            Store::load_with_global_path(&db, &bridge, Some(global.clone())).1,
            decode(&encode(other))
        );
        db.persist_editor_tool_state(&encode(drawing)).unwrap();
        let (mut store, loaded) = Store::load_with_global_path(&db, &bridge, Some(global.clone()));
        assert_eq!(loaded, Some(drawing));
        assert_eq!(std::fs::read(&global).unwrap(), encode(other));
        let pending = Pending::default();
        pending.submit_active_layer(LayerId(19));
        pending.submit_active_layer(LayerId(23));
        pending.flush(&mut store, &db, &bridge);
        assert_eq!(db.load_active_layer().unwrap(), Some(LayerId(23)));
        assert_eq!(std::fs::read(&global).unwrap(), encode(other));
        store.persist(&db, drawing, &bridge);
        drop(db);
        let db = ProjectDb::open(&path).unwrap();
        assert_eq!(
            db.load_editor_tool_state().unwrap().unwrap(),
            encode(drawing)
        );
        assert_eq!(
            Store::load(&db, &bridge).0.active_layer(),
            Some(LayerId(23))
        );
        assert_eq!(std::fs::read(&global).unwrap(), encode(drawing));
        assert!(db.load_reopened().unwrap().is_none());
        drop(db);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(global).unwrap();
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
