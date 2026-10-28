use std::collections::BTreeSet;

pub use nyatidraw_api::{CelId, FrameId};
use nyatidraw_api::{GroupId, LayerBlendMode};

use crate::{GroupNode, LayerTree, LayerTreeNode};

pub const MAX_ANIMATION_FRAMES: usize = 256;
pub const MAX_FRAME_DURATION_MS: u32 = 60_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationFrame {
    pub id: FrameId,
    pub cel: CelId,
    pub duration_ms: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationCel {
    pub id: CelId,
    pub root_group: GroupId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationTimeline {
    frames: Vec<AnimationFrame>,
    cels: Vec<AnimationCel>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationError {
    FrameLimit,
    InvalidIdentifier,
    InvalidDuration,
    DuplicateFrame,
    DuplicateCel,
    MissingCel,
    UnusedCel,
    InvalidCelGroup,
    UnknownFrame,
}

impl std::fmt::Display for AnimationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for AnimationError {}

impl AnimationTimeline {
    /// # Errors
    /// Rejects invalid identities, timing, references, or overlapping cel subtrees.
    pub fn new(
        frames: Vec<AnimationFrame>,
        cels: Vec<AnimationCel>,
        tree: &LayerTree,
    ) -> Result<Self, AnimationError> {
        let timeline = Self { frames, cels };
        timeline.validate_against(tree)?;
        Ok(timeline)
    }

    #[must_use]
    pub fn frames(&self) -> &[AnimationFrame] {
        &self.frames
    }

    #[must_use]
    pub fn cels(&self) -> &[AnimationCel] {
        &self.cels
    }

    #[must_use]
    pub fn cel_for_frame(&self, frame: FrameId) -> Option<&AnimationCel> {
        let cel = self.frames.iter().find(|entry| entry.id == frame)?.cel;
        self.cels.iter().find(|entry| entry.id == cel)
    }

    /// Returns a disposable frame composition without changing the full tree.
    /// # Errors
    /// Rejects invalid cel references or an unknown frame.
    pub fn resolve_frame_tree(
        &self,
        tree: &LayerTree,
        frame: FrameId,
    ) -> Result<LayerTree, AnimationError> {
        self.validate_against(tree)?;
        let cel = self
            .cel_for_frame(frame)
            .ok_or(AnimationError::UnknownFrame)?;
        let source = tree.root();
        let selected = source
            .children
            .iter()
            .find(|node| matches!(node, LayerTreeNode::Group(group) if group.id == cel.root_group))
            .ok_or(AnimationError::InvalidCelGroup)?;
        let root = GroupNode {
            id: source.id,
            name: source.name.clone(),
            visible: source.visible,
            opacity_u16: source.opacity_u16,
            clip_to_below: source.clip_to_below,
            blend_mode: source.blend_mode,
            children: vec![selected.clone()],
        };
        LayerTree::new(root).map_err(|_| AnimationError::InvalidCelGroup)
    }

    /// Every cel owns one direct child group of the authoritative document root.
    /// # Errors
    /// Rejects a changed tree that would orphan, overlap, or remove cel artwork.
    pub fn validate_against(&self, tree: &LayerTree) -> Result<(), AnimationError> {
        if self.frames.is_empty()
            || self.frames.len() > MAX_ANIMATION_FRAMES
            || self.cels.is_empty()
            || self.cels.len() > self.frames.len()
        {
            return Err(AnimationError::FrameLimit);
        }
        let mut frame_ids = BTreeSet::new();
        let mut used_cels = BTreeSet::new();
        for frame in &self.frames {
            if frame.id.0 == 0 || frame.cel.0 == 0 {
                return Err(AnimationError::InvalidIdentifier);
            }
            if frame.duration_ms == 0 || frame.duration_ms > MAX_FRAME_DURATION_MS {
                return Err(AnimationError::InvalidDuration);
            }
            if !frame_ids.insert(frame.id) {
                return Err(AnimationError::DuplicateFrame);
            }
            used_cels.insert(frame.cel);
        }
        let mut cel_ids = BTreeSet::new();
        let mut groups = BTreeSet::new();
        for cel in &self.cels {
            if cel.id.0 == 0 {
                return Err(AnimationError::InvalidIdentifier);
            }
            if !cel_ids.insert(cel.id) {
                return Err(AnimationError::DuplicateCel);
            }
            if !groups.insert(cel.root_group) {
                return Err(AnimationError::InvalidCelGroup);
            }
        }
        if !used_cels.is_subset(&cel_ids) {
            return Err(AnimationError::MissingCel);
        }
        if used_cels != cel_ids {
            return Err(AnimationError::UnusedCel);
        }
        if tree.root().children.len() != groups.len() || tree.root().children.iter().any(|node| {
            !matches!(node, LayerTreeNode::Group(group) if groups.contains(&group.id) && !group.clip_to_below && group.visible && group.opacity_u16 == u16::MAX && group.blend_mode == LayerBlendMode::Normal)
        }) { return Err(AnimationError::InvalidCelGroup); }
        Ok(())
    }
}
