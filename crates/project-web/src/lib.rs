#![forbid(unsafe_code)]

//! Browser session bridge for the native `.ntdr` container and shared wire codecs.

use std::ops::{Deref, DerefMut};

use nyatidraw_api::{CanvasSpec, DrawingTool, HistoryNodeId, PencilTemplate, SnapshotId};
#[cfg(test)]
use nyatidraw_document::LayerTreeNode;
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_project::{ReopenedProject, decode_editor_tool_state};
use nyatidraw_project_redb::MemoryProjectDb;
use nyatidraw_tiles::TileSnapshot;
use nyatidraw_web_core::{WebDocument, WebPreferences};

mod preferences;
mod recovery;
pub use recovery::{RecoveryCheckpoint, RecoveryRequest, RecoveryWriter};
#[cfg(test)]
mod tests;

pub const MAX_NTDR_BYTES: usize = 128 * 1024 * 1024;

pub struct WebProject {
    document: WebDocument,
    backing: Vec<u8>,
    saved_preferences: WebPreferences,
    selected_tool: Option<DrawingTool>,
    saved_selected_tool: Option<DrawingTool>,
    pencil_template: PencilTemplate,
    saved_pencil_template: PencilTemplate,
    import_notice: Option<&'static str>,
}

impl Deref for WebProject {
    type Target = WebDocument;

    fn deref(&self) -> &Self::Target {
        &self.document
    }
}

impl DerefMut for WebProject {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.document
    }
}

impl WebProject {
    /// # Errors
    /// Rejects a canvas outside browser resource limits.
    pub fn new(canvas: CanvasSpec) -> Result<Self, String> {
        WebDocument::new(canvas)
            .map(Self::from_document)
            .map_err(message)
    }

    #[must_use]
    pub fn from_document(document: WebDocument) -> Self {
        let saved_preferences = document.preferences();
        let pencil_template = if document.tool() == nyatidraw_web_core::WebTool::Pencil2B {
            PencilTemplate::Graphite2B
        } else {
            PencilTemplate::Mechanical2H
        };
        Self {
            document,
            backing: Vec::new(),
            saved_preferences,
            selected_tool: None,
            saved_selected_tool: None,
            pencil_template,
            saved_pencil_template: pencil_template,
            import_notice: None,
        }
    }

    /// Loads native bytes without modifying the input. Legacy NYWEB001 is import-only.
    /// # Errors
    /// Rejects corruption and resource limits before replacement, preserving groups.
    pub fn decode_ntdr(bytes: &[u8]) -> Result<Self, String> {
        if bytes.starts_with(b"NYWEB001") {
            let mut project =
                Self::from_document(WebDocument::decode_portable(bytes).map_err(message)?);
            project.selected_tool = Some(match project.document.tool() {
                nyatidraw_web_core::WebTool::Pencil2H | nyatidraw_web_core::WebTool::Pencil2B => {
                    DrawingTool::Pencil
                }
                nyatidraw_web_core::WebTool::Pen => DrawingTool::Pen,
                nyatidraw_web_core::WebTool::SoftBrush => DrawingTool::Brush,
                nyatidraw_web_core::WebTool::Eraser => DrawingTool::Eraser,
            });
            project.saved_selected_tool = project.selected_tool;
            return Ok(project);
        }
        let memory = MemoryProjectDb::open(bytes, MAX_NTDR_BYTES).map_err(message)?;
        let db = memory.db();
        let reopened = db.load_reopened().map_err(message)?;
        let (canvas, layers, tiles) = if let Some(reopened) = reopened {
            let parts = reopened.into_parts();
            (
                db.load_cursor_canvas_spec(parts.current_cursor)
                    .map_err(message)?,
                db.load_cursor_layer_tree(parts.current_cursor)
                    .map_err(message)?,
                parts.current_tiles,
            )
        } else {
            (
                db.load_canvas_spec().map_err(message)?,
                db.load_layer_tree().map_err(message)?,
                TileSnapshot::empty(),
            )
        };
        let mut document = if let Some(layers) = layers {
            let active = nyatidraw_editor::layer_edit::raster_layer_ids(&layers)
                .first()
                .copied()
                .ok_or("The browser requires at least one raster layer")?;
            WebDocument::from_snapshot(canvas, tiles, layers, active).map_err(message)?
        } else if tiles.is_empty() {
            WebDocument::new(canvas).map_err(message)?
        } else {
            return Err("This project has artwork but no layer metadata; open it in NyatiDraw Desktop first.".into());
        };
        let state = db.load_editor_tool_state().map_err(message)?;
        let decoded = state.as_deref().and_then(decode_editor_tool_state);
        let import_notice = if state.is_some() && decoded.is_none() {
            Some(
                "Unrecognized tool settings were preserved unchanged. Browser defaults are temporary; saving changed settings requires NyatiDraw Desktop.",
            )
        } else {
            None
        };
        if let Some(state) = decoded {
            document
                .restore_preferences(preferences::to_web(state))
                .map_err(message)?;
        }
        let selected_tool = decoded.map(|state| state.tool);
        let pencil_template =
            decoded.map_or(PencilTemplate::Mechanical2H, |state| state.pencil_template);
        let saved_preferences = document.preferences();
        drop(memory);
        Ok(Self {
            document,
            backing: bytes.to_vec(),
            saved_preferences,
            selected_tool,
            saved_selected_tool: selected_tool,
            pencil_template,
            saved_pencil_template: pencil_template,
            import_notice,
        })
    }

    #[must_use]
    pub const fn restored_tool(&self) -> Option<DrawingTool> {
        self.selected_tool
    }

    #[must_use]
    pub const fn import_notice(&self) -> Option<&'static str> {
        self.import_notice
    }

    pub fn set_project_tool(&mut self, tool: DrawingTool) {
        self.selected_tool = Some(tool);
        if tool == DrawingTool::Pencil {
            self.pencil_template = if self.document.tool() == nyatidraw_web_core::WebTool::Pencil2B
            {
                PencilTemplate::Graphite2B
            } else {
                PencilTemplate::Mechanical2H
            };
        }
    }

    #[must_use]
    pub const fn pencil_template(&self) -> PencilTemplate {
        self.pencil_template
    }

    /// # Errors
    /// Rejects settings that cannot be represented by the shared tool record.
    pub fn encode_tool_preferences(&self) -> Result<Vec<u8>, String> {
        let mut state = preferences::to_native(self.preferences(), self.selected_tool, None)?;
        if state.tool != DrawingTool::Pencil {
            state.pencil_template = self.pencil_template;
        }
        Ok(nyatidraw_project::encode_editor_tool_state(&state))
    }

    #[must_use]
    pub fn valid_tool_preferences(bytes: &[u8]) -> bool {
        decode_editor_tool_state(bytes).is_some()
    }

    /// # Errors
    /// Rejects invalid fallback settings without modifying artwork or history.
    pub fn restore_fallback_tool_preferences(&mut self, bytes: &[u8]) -> Result<(), String> {
        let state = decode_editor_tool_state(bytes).ok_or("Invalid app tool preferences")?;
        if self.selected_tool.is_none() && self.import_notice.is_none() {
            self.document
                .restore_preferences(preferences::to_web(state))
                .map_err(message)?;
            self.selected_tool = Some(state.tool);
            self.pencil_template = state.pencil_template;
        }
        Ok(())
    }

    /// Commits an exact structural edit into a copy of the native database.
    /// The accepted backing is replaced only after every write and close succeeds.
    /// # Errors
    /// Rejects active strokes, invalid preferences, exhausted budgets and storage failures.
    pub fn encode_ntdr(&mut self) -> Result<Vec<u8>, String> {
        if self.document.is_drawing() {
            return Err("Finish or cancel the current stroke before saving".into());
        }
        let memory = MemoryProjectDb::open(&self.backing, MAX_NTDR_BYTES).map_err(message)?;
        let db = memory.db();
        let reopened = db.load_reopened().map_err(message)?;
        let changed = match &reopened {
            Some(reopened) => {
                let parts = reopened.clone().into_parts();
                parts.current_tiles != *self.document.snapshot()
                    || db
                        .load_cursor_layer_tree(parts.current_cursor)
                        .map_err(message)?
                        .as_ref()
                        != Some(self.document.layers())
                    || db
                        .load_cursor_canvas_spec(parts.current_cursor)
                        .map_err(message)?
                        != self.document.canvas()
            }
            None => true,
        };
        if changed {
            let next_id = next_id(reopened.as_ref())?;
            let initialize_metadata = reopened.is_none();
            let session = reopened.map_or_else(
                || HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty()),
                HeadlessStrokeSession::from_reopened,
            );
            if initialize_metadata {
                db.persist_layer_tree(self.document.layers())
                    .map_err(message)?;
                db.persist_canvas_spec(self.document.canvas())
                    .map_err(message)?;
            }
            let batch = session
                .prepare_structural_change(
                    SnapshotId(next_id),
                    HistoryNodeId(next_id),
                    0,
                    self.document.snapshot().clone(),
                )
                .map_err(|error| format!("Cannot prepare project edit: {error:?}"))?;
            db.commit_structural_with_metadata(
                &batch,
                self.document.layers(),
                self.document.canvas(),
            )
            .map_err(message)?;
        }
        let current_preferences = self.document.preferences();
        let original = db.load_editor_tool_state().map_err(message)?;
        if original.is_none()
            || self.saved_preferences != current_preferences
            || self.selected_tool != self.saved_selected_tool
            || self.pencil_template != self.saved_pencil_template
        {
            let decoded = original.as_deref().and_then(decode_editor_tool_state);
            if original.is_some() && decoded.is_none() {
                return Err("This project has unrecognized tool settings. They were preserved; change them in NyatiDraw Desktop before saving new browser settings.".into());
            }
            let mut state =
                preferences::to_native(current_preferences, self.selected_tool, decoded)?;
            if state.tool != DrawingTool::Pencil {
                state.pencil_template = self.pencil_template;
            }
            db.persist_editor_tool_state(&nyatidraw_project::encode_editor_tool_state(&state))
                .map_err(message)?;
        }
        let bytes = memory.into_bytes().map_err(message)?;
        self.backing.clone_from(&bytes);
        self.saved_preferences = current_preferences;
        self.saved_selected_tool = self.selected_tool;
        self.saved_pencil_template = self.pencil_template;
        Ok(bytes)
    }
}

fn next_id(reopened: Option<&ReopenedProject>) -> Result<u128, String> {
    let Some(reopened) = reopened else {
        return Ok(1);
    };
    let parts = reopened.clone().into_parts();
    let max_snapshot = parts
        .cursors
        .values()
        .map(|cursor| cursor.snapshot_id.0)
        .max()
        .unwrap_or(0);
    let max_node = parts
        .history
        .nodes()
        .map(|node| node.id.0)
        .max()
        .unwrap_or(0);
    max_snapshot
        .max(max_node)
        .checked_add(1)
        .ok_or_else(|| "Project history identifiers exhausted".into())
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
