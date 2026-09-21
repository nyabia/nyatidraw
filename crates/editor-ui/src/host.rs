use crate::{
    layout_store::PanelHeights,
    preview::{LayerThumbnailSnapshot, NavigatorSnapshot},
    workspace_appearance::WorkspaceAppearance,
};
use dioxus::prelude::*;
use nyatidraw_api::{EditorCommand, EventEnvelope, Revision, UiProjection};
use std::{ops::Deref, rc::Rc};

pub type PalettePins = [Option<[u8; 4]>; 10];

pub trait UiBackend {
    fn protocol_snapshot(&self) -> (UiProjection, Option<EventEnvelope>);
    /// # Errors
    /// Returns the host's admission or execution failure.
    fn push_editor_command(&self, based_on: Revision, command: EditorCommand)
    -> Result<(), String>;
    fn supports_command(&self, _command: &EditorCommand) -> bool {
        true
    }
    /// # Errors
    /// Rejects unsupported commands or host admission failures.
    fn push_ui_editor_command(&self, command: EditorCommand) -> Result<(), String> {
        if !self.supports_command(&command) {
            return Err("현재 환경에서는 아직 지원하지 않는 기능입니다.".into());
        }
        self.push_editor_command(self.protocol_snapshot().0.revision, command)
    }
    fn project_epoch(&self) -> u64 {
        0
    }
    fn navigator_snapshot(&self) -> NavigatorSnapshot {
        NavigatorSnapshot::default()
    }
    fn layer_thumbnail_snapshot(&self) -> LayerThumbnailSnapshot {
        LayerThumbnailSnapshot::default()
    }
    fn panel_heights(&self) -> PanelHeights {
        PanelHeights::default()
    }
    fn submit_panel_heights(&self, _heights: &PanelHeights) {}
    fn workspace_appearance(&self) -> WorkspaceAppearance {
        WorkspaceAppearance::default()
    }
    fn set_workspace_appearance(&self, _appearance: WorkspaceAppearance) {}
    fn palette_pins(&self) -> PalettePins {
        [None; 10]
    }
    fn save_palette_pins(&self, _pins: PalettePins) {}
    fn palette_notice(&self) -> Option<String> {
        None
    }
    fn is_saving_as(&self) -> bool {
        false
    }
    fn layout_notice(&self) -> Option<String> {
        None
    }
    fn now_millis(&self) -> f64;
    fn dock_cancel_probe(&self) -> bool {
        false
    }
    fn preview_preset(&self, projection: &UiProjection) -> nyatidraw_brush::BrushPreset;
}

#[derive(Clone)]
pub struct UiHost(pub Rc<dyn UiBackend>);

impl UiHost {
    /// # Errors
    /// Rejects unsupported commands or host admission failures.
    pub fn push_editor_command(
        &self,
        based_on: Revision,
        command: EditorCommand,
    ) -> Result<(), String> {
        if !self.supports_command(&command) {
            return Err("현재 환경에서는 아직 지원하지 않는 기능입니다.".into());
        }
        self.0.push_editor_command(based_on, command)
    }

    /// # Errors
    /// Rejects unsupported commands or host admission failures.
    pub fn push_ui_editor_command(&self, command: EditorCommand) -> Result<(), String> {
        if !self.supports_command(&command) {
            return Err("현재 환경에서는 아직 지원하지 않는 기능입니다.".into());
        }
        self.0.push_ui_editor_command(command)
    }
}

impl Deref for UiHost {
    type Target = dyn UiBackend;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

#[derive(Clone, Copy)]
pub struct UiSlots {
    pub canvas: fn() -> Element,
    pub file_buttons: fn(bool) -> Element,
    pub update_control: fn() -> Element,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ExportStatus {
    #[default]
    Idle,
    Waiting {
        generation: u64,
    },
    Queued {
        generation: u64,
    },
    Running {
        generation: u64,
    },
    Current {
        generation: u64,
    },
    Failed {
        generation: u64,
    },
}
