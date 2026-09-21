use std::{cell::RefCell, rc::Rc, sync::Arc, time::Instant};

use nyatidraw_api::{EditorCommand, EventEnvelope, Revision, UiProjection};
use nyatidraw_editor_ui::{PalettePins, UiBackend, UiHost};

use crate::{live_ink::LiveInkBridge, palette_preferences::PalettePreferences};

struct DesktopUiBackend {
    live_ink: LiveInkBridge,
    palette: PalettePreferences,
    initial_pins: PalettePins,
    heights: RefCell<crate::layout_store::PanelHeights>,
    clock: Instant,
}

pub(crate) fn create(live_ink: LiveInkBridge, notify: Arc<dyn Fn() + Send + Sync>) -> UiHost {
    let (palette, initial_pins) = PalettePreferences::open(notify);
    UiHost(Rc::new(DesktopUiBackend {
        live_ink,
        palette,
        initial_pins,
        heights: RefCell::new(crate::layout_store::PanelHeights::load()),
        clock: Instant::now(),
    }))
}

impl UiBackend for DesktopUiBackend {
    fn protocol_snapshot(&self) -> (UiProjection, Option<EventEnvelope>) {
        self.live_ink.protocol_snapshot()
    }

    fn push_editor_command(
        &self,
        based_on: Revision,
        command: EditorCommand,
    ) -> Result<(), String> {
        self.live_ink
            .push_editor_command(based_on, command)
            .map(|_| ())
            .map_err(|_| "캔버스가 처리 중입니다. 잠시 후 다시 시도하세요".to_owned())
    }

    fn push_ui_editor_command(&self, command: EditorCommand) -> Result<(), String> {
        self.live_ink
            .push_ui_editor_command(command)
            .map(|_| ())
            .map_err(|_| "캔버스가 처리 중입니다. 잠시 후 다시 시도하세요".to_owned())
    }

    fn project_epoch(&self) -> u64 {
        self.live_ink.project_epoch()
    }

    fn navigator_snapshot(&self) -> nyatidraw_editor_ui::preview::NavigatorSnapshot {
        self.live_ink.navigator_snapshot()
    }

    fn layer_thumbnail_snapshot(&self) -> nyatidraw_editor_ui::preview::LayerThumbnailSnapshot {
        self.live_ink.layer_thumbnail_snapshot()
    }

    fn panel_heights(&self) -> nyatidraw_editor_ui::layout_store::PanelHeights {
        let mut result = nyatidraw_editor_ui::layout_store::PanelHeights::default();
        let native = self.heights.borrow();
        for panel in crate::layout_store::PANELS {
            if let Some(height) = native.get(panel) {
                let _ = result.set(panel, height);
            }
        }
        result
    }

    fn submit_panel_heights(&self, heights: &nyatidraw_editor_ui::layout_store::PanelHeights) {
        let mut native = self.heights.borrow_mut();
        for panel in crate::layout_store::PANELS {
            if let Some(height) = heights.get(panel)
                && native.get(panel) != Some(height)
                && let Err(error) = native.set(panel, height)
            {
                self.live_ink
                    .publish_activation_notice(format!("패널 높이를 저장하지 못했습니다: {error}"));
                return;
            }
        }
        self.live_ink.submit_panel_heights(&native);
    }

    fn workspace_appearance(
        &self,
    ) -> nyatidraw_editor_ui::workspace_appearance::WorkspaceAppearance {
        let native = self.live_ink.workspace_appearance();
        nyatidraw_editor_ui::workspace_appearance::WorkspaceAppearance {
            checkerboard: native.checkerboard,
            solid_rgb: native.solid_rgb,
        }
    }

    fn set_workspace_appearance(
        &self,
        appearance: nyatidraw_editor_ui::workspace_appearance::WorkspaceAppearance,
    ) {
        let mut native = self.live_ink.workspace_appearance();
        native.checkerboard = appearance.checkerboard;
        native.solid_rgb = appearance.solid_rgb;
        self.live_ink.set_workspace_appearance(native);
    }

    fn palette_pins(&self) -> PalettePins {
        self.initial_pins
    }

    fn save_palette_pins(&self, pins: PalettePins) {
        self.palette.save(pins);
    }

    fn palette_notice(&self) -> Option<String> {
        self.palette.notice()
    }

    fn is_saving_as(&self) -> bool {
        self.live_ink.is_saving_as()
    }

    fn layout_notice(&self) -> Option<String> {
        self.live_ink.layout_notice()
    }

    fn now_millis(&self) -> f64 {
        self.clock.elapsed().as_secs_f64() * 1_000.0
    }

    fn preview_preset(&self, projection: &UiProjection) -> nyatidraw_brush::BrushPreset {
        crate::native_canvas::preset_for_ui_preview(projection)
    }

    fn dock_cancel_probe(&self) -> bool {
        if std::env::var("NAYATI_DOCK_CANCEL_PROBE").as_deref() != Ok("sequence") {
            return false;
        }
        let Some(project) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
            return false;
        };
        project
            .file_name()
            .is_some_and(|name| name == "edit-source-scratch.ntdr")
            && project
                .parent()
                .is_some_and(|parent| parent.join(".nyatidraw-scratch-dock-probe").is_file())
    }
}

pub(crate) fn export_status(
    status: crate::live_ink::ExportStatus,
) -> nyatidraw_editor_ui::ExportStatus {
    use crate::live_ink::ExportStatus as Native;
    use nyatidraw_editor_ui::ExportStatus as Ui;
    match status {
        Native::Idle => Ui::Idle,
        Native::Waiting { generation } => Ui::Waiting { generation },
        Native::Queued { generation } => Ui::Queued { generation },
        Native::Running { generation } => Ui::Running { generation },
        Native::Current { generation } => Ui::Current { generation },
        Native::Failed { generation } => Ui::Failed { generation },
    }
}
