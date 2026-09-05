use nyatidraw_api::{
    CommandRejectReason, DockLayoutError, DockLayoutRecoveryStatus, DockNode, DockTree,
    DrawingTool, EditorEvent, HistoryProjection, LayerId, LayerProjection, LayerProjectionKind,
    LayerTreeNodeId, Revision, UiProjection, ViewportProjection, WorkspaceProjection,
};
use nyatidraw_document::{GroupNode, LayerTree, LayerTreeNode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    RevisionExhausted,
    UnknownActiveLayer(LayerId),
}

/// Paint-thread-owned source for immutable, semantic UI projections.
///
/// Publishing walks layer metadata only. It does not own or touch the native
/// input queue, raster pixels, or GPU handles.
#[derive(Clone, Debug)]
pub struct ProjectionState {
    current: UiProjection,
}

impl ProjectionState {
    #[must_use]
    pub fn new(dock: DockTree) -> Self {
        let mut current = UiProjection::empty();
        current.dock = dock;
        Self { current }
    }

    #[must_use]
    pub const fn current(&self) -> &UiProjection {
        &self.current
    }

    /// Replaces the paint-thread cache with a projection already published by
    /// the shared authority, such as a worker-triggered fatal workspace latch.
    pub fn install_authoritative(&mut self, projection: UiProjection) {
        self.current = projection;
    }

    /// Stages a validated dock tree for the next complete semantic
    /// publication. The caller must publish before acknowledging the command.
    pub fn stage_dock(&mut self, dock: DockTree) {
        self.current.dock = dock;
    }

    /// Stages the current drawing controls for the next semantic publication.
    pub fn stage_drawing_controls(
        &mut self,
        tool: DrawingTool,
        size_tenths: u16,
        opacity_u16: u16,
        color: [u8; 4],
    ) {
        self.current.drawing_tool = tool;
        self.current.brush_size_tenths = size_tenths;
        self.current.brush_opacity_u16 = opacity_u16;
        if self.current.brush_color != color {
            self.current.recent_colors.retain(|recent| *recent != color);
            self.current.recent_colors.insert(0, color);
            self.current.recent_colors.truncate(8);
        }
        self.current.brush_color = color;
    }

    /// Stages session-only layer isolation for the next semantic projection.
    pub fn stage_solo(&mut self, node: Option<LayerTreeNodeId>) {
        self.current.solo_node = node;
    }

    /// Stages the writer-owned, bounded history ancestry for the next
    /// semantic publication. It contains no database handle, pixels, GPU
    /// state, or raw input.
    pub fn stage_history(&mut self, history: HistoryProjection) {
        self.current.history = history;
    }

    pub fn stage_edit(&mut self, edit: nyatidraw_api::EditProjection) {
        self.current.edit = edit;
    }

    pub fn stage_edit_settings(&mut self, settings: nyatidraw_api::EditSettings) {
        self.current.edit_settings = settings;
    }

    /// Publishes document metadata at one monotonically increasing revision.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown active raster layer or exhausted
    /// revision counter. The previous projection remains unchanged.
    pub fn publish_document(
        &mut self,
        title: String,
        dirty: bool,
        active_layer: Option<LayerId>,
        layers: &LayerTree,
    ) -> Result<EditorEvent, ProjectionError> {
        self.publish_editor_state(
            title,
            dirty,
            active_layer,
            layers,
            self.current.dock.clone(),
            self.current.viewport,
        )
    }

    /// Publishes the complete metadata state after an authoritative semantic
    /// command has succeeded. The revision advances exactly once.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown active layer or exhausted revision.
    pub fn publish_editor_state(
        &mut self,
        title: String,
        dirty: bool,
        active_layer: Option<LayerId>,
        layers: &LayerTree,
        dock: DockTree,
        viewport: ViewportProjection,
    ) -> Result<EditorEvent, ProjectionError> {
        if let Some(active) = active_layer
            && layers.ancestors(LayerTreeNodeId::Raster(active)).is_none()
        {
            return Err(ProjectionError::UnknownActiveLayer(active));
        }
        let next_revision = self
            .current
            .revision
            .checked_next()
            .ok_or(ProjectionError::RevisionExhausted)?;
        let mut projected_layers = Vec::new();
        project_children(layers.root(), 1, &mut projected_layers);

        self.current.revision = next_revision;
        self.current.document_title = title;
        self.current.dirty = dirty;
        self.current.workspace = WorkspaceProjection::Ready;
        self.current.active_layer = active_layer;
        self.current.layers = projected_layers;
        self.current.dock = dock;
        self.current.viewport = viewport;
        Ok(EditorEvent::ProjectionChanged {
            revision: next_revision,
        })
    }

    /// Installs a decoded layout or the complete safe default, then publishes
    /// exactly one semantic revision describing the outcome.
    ///
    /// # Errors
    ///
    /// Returns `RevisionExhausted` without changing the current projection.
    pub fn recover_layout(
        &mut self,
        decoded: Result<DockNode, DockLayoutError>,
    ) -> Result<EditorEvent, ProjectionError> {
        let recovery = DockTree::recover(decoded);
        let next_revision = self
            .current
            .revision
            .checked_next()
            .ok_or(ProjectionError::RevisionExhausted)?;
        self.current.dock = recovery.tree;
        self.current.revision = next_revision;
        Ok(match recovery.status {
            DockLayoutRecoveryStatus::Loaded => EditorEvent::ProjectionChanged {
                revision: next_revision,
            },
            DockLayoutRecoveryStatus::SafeDefault(reason) => EditorEvent::LayoutRecovered {
                revision: next_revision,
                reason,
            },
        })
    }

    /// Checks the optimistic UI revision carried by a command envelope.
    ///
    /// # Errors
    ///
    /// Returns a typed stale-projection rejection when revisions differ.
    pub fn validate_command_revision(&self, based_on: Revision) -> Result<(), CommandRejectReason> {
        if based_on == self.current.revision {
            Ok(())
        } else {
            Err(CommandRejectReason::StaleProjection {
                based_on,
                current: self.current.revision,
            })
        }
    }
}

impl Default for ProjectionState {
    fn default() -> Self {
        Self::new(DockTree::safe_default())
    }
}

fn project_children(group: &GroupNode, depth: u16, out: &mut Vec<LayerProjection>) {
    // Document children are stored bottom-to-top. The UI projection is
    // top-to-bottom, but a group must still precede its own descendants so
    // collapse/indent semantics remain structurally meaningful.
    for (index, child) in group.children.iter().enumerate().rev() {
        match child {
            LayerTreeNode::Raster(layer) => out.push(LayerProjection {
                id: LayerTreeNodeId::Raster(layer.id),
                parent: group.id,
                index,
                depth,
                kind: LayerProjectionKind::Raster,
                name: layer.name.clone(),
                visible: layer.visible,
                reference: layer.reference,
                opacity_u16: layer.opacity_u16,
            }),
            LayerTreeNode::Group(child_group) => {
                out.push(LayerProjection {
                    id: LayerTreeNodeId::Group(child_group.id),
                    parent: group.id,
                    index,
                    depth,
                    kind: LayerProjectionKind::Group,
                    name: child_group.name.clone(),
                    visible: child_group.visible,
                    reference: false,
                    opacity_u16: child_group.opacity_u16,
                });
                project_children(child_group, depth.saturating_add(1), out);
            }
        }
    }
}
