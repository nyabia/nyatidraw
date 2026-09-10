use std::collections::BTreeSet;

use nyatidraw_api::{
    CompositeInvalidation, ContentRootId, GroupId, LayerBlendMode, LayerId, LayerTreeNodeId,
    TileCoordinate,
};

const MAX_LAYER_TREE_DEPTH: usize = 64;
const MAX_LAYER_NAME_CHARS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
// Visibility, edit protection, alpha protection and source roles are independent.
#[allow(clippy::struct_excessive_bools)]
pub struct LayerNode {
    /// Preserve stored pixel alpha while recoloring; distinct from edit lock.
    pub alpha_locked: bool,
    /// Uses the nearest preceding unclipped sibling as the clipping base.
    pub clip_to_below: bool,
    pub blend_mode: LayerBlendMode,
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    /// Selection/fill source membership; never changes ordinary composition.
    pub reference: bool,
    pub opacity_u16: u16,
    pub content_root: ContentRootId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupNode {
    pub clip_to_below: bool,
    pub blend_mode: LayerBlendMode,
    pub id: GroupId,
    pub name: String,
    pub visible: bool,
    pub opacity_u16: u16,
    pub children: Vec<LayerTreeNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayerTreeNode {
    Raster(LayerNode),
    Group(GroupNode),
}

impl LayerTreeNode {
    #[must_use]
    pub const fn id(&self) -> LayerTreeNodeId {
        match self {
            Self::Raster(layer) => LayerTreeNodeId::Raster(layer.id),
            Self::Group(group) => LayerTreeNodeId::Group(group.id),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerTreeError {
    DuplicateNode(LayerTreeNodeId),
    InvalidRootProperties,
    TooDeep,
    UnknownNode(LayerTreeNodeId),
    DestinationNotGroup(GroupId),
    CannotEditRoot,
    MoveIntoDescendant,
    IndexOutOfBounds { index: usize, len: usize },
    InvalidName,
    LockedNode(LayerTreeNodeId),
}

/// Validated raster/group hierarchy in bottom-to-top compositing order.
///
/// The root is an implicit document composite. Its visibility and opacity are
/// fixed to visible and fully opaque; user-editable groups live below it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerTree {
    root: GroupNode,
}

impl LayerTree {
    /// Validates global node-ID uniqueness and the implicit-root invariant.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate tagged IDs or mutable root properties.
    pub fn new(root: GroupNode) -> Result<Self, LayerTreeError> {
        if !root.visible
            || root.opacity_u16 != u16::MAX
            || root.clip_to_below
            || root.blend_mode != LayerBlendMode::Normal
        {
            return Err(LayerTreeError::InvalidRootProperties);
        }
        let mut ids = BTreeSet::new();
        ids.insert(LayerTreeNodeId::Group(root.id));
        validate_children(&root, 0, &mut ids)?;
        Ok(Self { root })
    }

    #[must_use]
    pub const fn root(&self) -> &GroupNode {
        &self.root
    }

    #[must_use]
    pub const fn root_id(&self) -> GroupId {
        self.root.id
    }

    /// Returns the direct parent and bottom-to-top child index of a node.
    #[must_use]
    pub fn parent_and_index(&self, node: LayerTreeNodeId) -> Option<(GroupId, usize)> {
        if node == LayerTreeNodeId::Group(self.root.id) {
            return None;
        }
        parent_and_index(&self.root, node)
    }

    /// Inserts a raster or group while preserving global ID and depth invariants.
    ///
    /// Validation is failure-atomic: a rejected insert leaves the tree unchanged.
    ///
    /// # Errors
    ///
    /// Rejects an unknown parent, an out-of-range index, a duplicate ID, or a
    /// tree deeper than the supported project schema.
    pub fn insert(
        &mut self,
        parent: GroupId,
        index: usize,
        node: LayerTreeNode,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        let mut candidate = self.clone();
        let destination = find_group_mut(&mut candidate.root, parent)
            .ok_or(LayerTreeError::DestinationNotGroup(parent))?;
        if index > destination.children.len() {
            return Err(LayerTreeError::IndexOutOfBounds {
                index,
                len: destination.children.len(),
            });
        }
        destination.children.insert(index, node);
        let validated = Self::new(candidate.root)?;
        self.root = validated.root;

        let mut invalidation = CompositeInvalidation::empty();
        invalidation.invalidate_all_tiles(parent);
        for ancestor in self
            .ancestors(LayerTreeNodeId::Group(parent))
            .unwrap_or_default()
        {
            invalidation.invalidate_all_tiles(ancestor);
        }
        Ok(invalidation)
    }

    /// Copies raster metadata immediately above its source in the same group.
    /// The caller must duplicate its tile keys in the same durable transaction.
    ///
    /// # Errors
    /// Rejects unknown sources or duplicate destination IDs without mutation.
    pub fn duplicate_raster(
        &mut self,
        source: LayerId,
        destination: LayerId,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        let source_node = LayerTreeNodeId::Raster(source);
        let (parent, index) = self
            .parent_and_index(source_node)
            .ok_or(LayerTreeError::UnknownNode(source_node))?;
        let Some(LayerTreeNode::Raster(layer)) = find_node_mut(&mut self.root, source_node) else {
            return Err(LayerTreeError::UnknownNode(source_node));
        };
        let mut copy = layer.clone();
        copy.id = destination;
        let suffix = " 복사";
        copy.name = layer
            .name
            .chars()
            .take(MAX_LAYER_NAME_CHARS - suffix.chars().count())
            .chain(suffix.chars())
            .collect();
        self.insert(parent, index + 1, LayerTreeNode::Raster(copy))
    }

    /// Changes a user-visible node name without invalidating pixel composites.
    ///
    /// # Errors
    ///
    /// Rejects the implicit root, unknown nodes, blank names, and names longer
    /// than the project schema limit.
    pub fn rename(
        &mut self,
        node: LayerTreeNodeId,
        name: &str,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        if node == LayerTreeNodeId::Group(self.root.id) {
            return Err(LayerTreeError::CannotEditRoot);
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_LAYER_NAME_CHARS {
            return Err(LayerTreeError::InvalidName);
        }
        match find_node_mut(&mut self.root, node) {
            Some(LayerTreeNode::Raster(layer)) => name.clone_into(&mut layer.name),
            Some(LayerTreeNode::Group(group)) => name.clone_into(&mut group.name),
            None => return Err(LayerTreeError::UnknownNode(node)),
        }
        Ok(CompositeInvalidation::empty())
    }

    /// Returns immutable raster metadata, including its authoritative lock.
    #[must_use]
    pub fn raster(&self, id: LayerId) -> Option<&LayerNode> {
        match find_node(&self.root, LayerTreeNodeId::Raster(id))? {
            LayerTreeNode::Raster(layer) => Some(layer),
            LayerTreeNode::Group(_) => None,
        }
    }

    /// Changes a raster lock without altering its pixels or composition.
    ///
    /// # Errors
    /// Rejects unknown raster IDs without mutation.
    pub fn set_locked(&mut self, id: LayerId, locked: bool) -> Result<(), LayerTreeError> {
        let node = LayerTreeNodeId::Raster(id);
        match find_node_mut(&mut self.root, node) {
            Some(LayerTreeNode::Raster(layer)) => {
                layer.locked = locked;
                Ok(())
            }
            _ => Err(LayerTreeError::UnknownNode(node)),
        }
    }

    /// Changes raster alpha protection without changing existing composition.
    /// # Errors
    /// Rejects unknown rasters atomically.
    pub fn set_alpha_locked(
        &mut self,
        id: LayerId,
        alpha_locked: bool,
    ) -> Result<(), LayerTreeError> {
        let node = LayerTreeNodeId::Raster(id);
        match find_node_mut(&mut self.root, node) {
            Some(LayerTreeNode::Raster(layer)) => {
                layer.alpha_locked = alpha_locked;
                Ok(())
            }
            _ => Err(LayerTreeError::UnknownNode(node)),
        }
    }

    /// Changes same-parent clipping membership and invalidates ancestor composites.
    /// No stored base ID is rebound; sibling order determines the current stack.
    /// # Errors
    /// Rejects the implicit root and unknown nodes without mutation.
    pub fn set_clip_to_below(
        &mut self,
        node: LayerTreeNodeId,
        value: bool,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        let ancestors = self.editable_ancestors(node)?;
        let slot = match find_node_mut(&mut self.root, node) {
            Some(LayerTreeNode::Raster(layer)) => &mut layer.clip_to_below,
            Some(LayerTreeNode::Group(group)) => &mut group.clip_to_below,
            None => return Err(LayerTreeError::UnknownNode(node)),
        };
        if *slot == value {
            return Ok(CompositeInvalidation::empty());
        }
        *slot = value;
        Ok(invalidate_all(ancestors))
    }

    /// Changes a raster/isolated group's backdrop blend mode.
    /// # Errors
    /// Rejects the implicit root and unknown nodes without mutation.
    pub fn set_blend_mode(
        &mut self,
        node: LayerTreeNodeId,
        value: LayerBlendMode,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        let ancestors = self.editable_ancestors(node)?;
        let slot = match find_node_mut(&mut self.root, node) {
            Some(LayerTreeNode::Raster(layer)) => &mut layer.blend_mode,
            Some(LayerTreeNode::Group(group)) => &mut group.blend_mode,
            None => return Err(LayerTreeError::UnknownNode(node)),
        };
        if *slot == value {
            return Ok(CompositeInvalidation::empty());
        }
        *slot = value;
        Ok(invalidate_all(ancestors))
    }

    /// Changes a raster's selection/fill source membership without invalidating
    /// ordinary composition. The caller persists this metadata through history.
    ///
    /// # Errors
    /// Rejects an unknown raster ID without changing the tree.
    pub fn set_reference(&mut self, id: LayerId, reference: bool) -> Result<(), LayerTreeError> {
        let node = LayerTreeNodeId::Raster(id);
        match find_node_mut(&mut self.root, node) {
            Some(LayerTreeNode::Raster(layer)) => {
                layer.reference = reference;
                Ok(())
            }
            _ => Err(LayerTreeError::UnknownNode(node)),
        }
    }

    /// Removes one raster or whole subtree. The caller retains the prior tree
    /// and tiles in history before publishing this candidate.
    ///
    /// # Errors
    /// Rejects the implicit root, unknown IDs, or subtrees containing locked
    /// rasters without changing the tree.
    pub fn remove(&mut self, node: LayerTreeNodeId) -> Result<LayerTreeNode, LayerTreeError> {
        if node == LayerTreeNodeId::Group(self.root.id) {
            return Err(LayerTreeError::CannotEditRoot);
        }
        if find_node(&self.root, node).is_some_and(contains_locked_raster) {
            return Err(LayerTreeError::LockedNode(node));
        }
        remove_node(&mut self.root, node).ok_or(LayerTreeError::UnknownNode(node))
    }

    /// Returns group ancestors from the immediate parent through the root.
    #[must_use]
    pub fn ancestors(&self, node: LayerTreeNodeId) -> Option<Vec<GroupId>> {
        if node == LayerTreeNodeId::Group(self.root.id) {
            return Some(Vec::new());
        }
        ancestors_in_group(&self.root, node)
    }

    /// Presentation-only visibility shared by the GPU and temporary picker.
    /// Solo reveals its ancestor path without changing saved visibility; a Solo
    /// group's descendants retain their own visibility, at every nesting depth.
    #[must_use]
    pub fn display_child_visible(
        &self,
        _parent: GroupId,
        child: &LayerTreeNode,
        solo: Option<LayerTreeNodeId>,
    ) -> bool {
        match solo {
            None => match child {
                LayerTreeNode::Raster(layer) => layer.visible,
                LayerTreeNode::Group(group) => group.visible,
            },
            Some(target) => {
                crate::composition_scope_nodes(self.root(), crate::CompositionScope::Solo(target))
                    .contains(&child.id())
            }
        }
    }

    /// Scopes one raster tile edit to the exact coordinate in its ancestor
    /// group composites. Sibling groups and other tile coordinates are absent.
    ///
    /// # Errors
    ///
    /// Returns `UnknownNode` unless `layer` identifies a raster node.
    pub fn invalidate_raster_tile(
        &self,
        layer: LayerId,
        tile: TileCoordinate,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        let node = LayerTreeNodeId::Raster(layer);
        let ancestors = self
            .ancestors(node)
            .ok_or(LayerTreeError::UnknownNode(node))?;
        let mut invalidation = CompositeInvalidation::empty();
        for group in ancestors {
            invalidation.invalidate_tile(group, tile);
        }
        Ok(invalidation)
    }

    /// Changes a node's visibility and invalidates all cached coordinates only
    /// for composites above that node.
    ///
    /// # Errors
    ///
    /// Returns an error for the implicit root or an unknown node.
    pub fn set_visibility(
        &mut self,
        node: LayerTreeNodeId,
        visible: bool,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        let ancestors = self.editable_ancestors(node)?;
        let changed = match find_node_mut(&mut self.root, node) {
            Some(LayerTreeNode::Raster(layer)) if layer.visible != visible => {
                layer.visible = visible;
                true
            }
            Some(LayerTreeNode::Group(group)) if group.visible != visible => {
                group.visible = visible;
                true
            }
            Some(_) => false,
            None => return Err(LayerTreeError::UnknownNode(node)),
        };
        Ok(if changed {
            invalidate_all(ancestors)
        } else {
            CompositeInvalidation::empty()
        })
    }

    /// Changes a node's opacity and invalidates all cached coordinates only for
    /// composites above that node.
    ///
    /// # Errors
    ///
    /// Returns an error for the implicit root or an unknown node.
    pub fn set_opacity(
        &mut self,
        node: LayerTreeNodeId,
        opacity_u16: u16,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        let ancestors = self.editable_ancestors(node)?;
        let changed = match find_node_mut(&mut self.root, node) {
            Some(LayerTreeNode::Raster(layer)) if layer.opacity_u16 != opacity_u16 => {
                layer.opacity_u16 = opacity_u16;
                true
            }
            Some(LayerTreeNode::Group(group)) if group.opacity_u16 != opacity_u16 => {
                group.opacity_u16 = opacity_u16;
                true
            }
            Some(_) => false,
            None => return Err(LayerTreeError::UnknownNode(node)),
        };
        Ok(if changed {
            invalidate_all(ancestors)
        } else {
            CompositeInvalidation::empty()
        })
    }

    /// Moves one raster/group node using an insertion index measured after the
    /// node has been removed from its old parent.
    ///
    /// The operation validates every failure before mutation, so a rejected
    /// move leaves ordering and ownership unchanged.
    ///
    /// # Errors
    ///
    /// Rejects unknown nodes, root moves, cycles, non-group destinations, and
    /// out-of-range post-removal indices or excessive resulting depth.
    pub fn reorder(
        &mut self,
        node: LayerTreeNodeId,
        new_parent: GroupId,
        index: usize,
    ) -> Result<CompositeInvalidation, LayerTreeError> {
        if node == LayerTreeNodeId::Group(self.root.id) {
            return Err(LayerTreeError::CannotEditRoot);
        }
        let old_ancestors = self
            .ancestors(node)
            .ok_or(LayerTreeError::UnknownNode(node))?;
        let (old_parent, old_index) =
            parent_and_index(&self.root, node).ok_or(LayerTreeError::UnknownNode(node))?;
        let destination = find_group(&self.root, new_parent)
            .ok_or(LayerTreeError::DestinationNotGroup(new_parent))?;
        if let LayerTreeNodeId::Group(moving_group) = node
            && (moving_group == new_parent
                || find_group(group_ref(&self.root, moving_group)?, new_parent).is_some())
        {
            return Err(LayerTreeError::MoveIntoDescendant);
        }

        let destination_len_after_removal =
            destination.children.len() - usize::from(old_parent == new_parent);
        if index > destination_len_after_removal {
            return Err(LayerTreeError::IndexOutOfBounds {
                index,
                len: destination_len_after_removal,
            });
        }
        if old_parent == new_parent && old_index == index {
            return Ok(CompositeInvalidation::empty());
        }

        let mut candidate = self.clone();
        let moved =
            remove_node(&mut candidate.root, node).ok_or(LayerTreeError::UnknownNode(node))?;
        find_group_mut(&mut candidate.root, new_parent)
            .ok_or(LayerTreeError::DestinationNotGroup(new_parent))?
            .children
            .insert(index, moved);
        let candidate = Self::new(candidate.root)?;

        let mut invalidation = invalidate_all(old_ancestors);
        let new_ancestors = candidate
            .ancestors(node)
            .ok_or(LayerTreeError::UnknownNode(node))?;
        invalidation.extend(invalidate_all(new_ancestors));
        *self = candidate;
        Ok(invalidation)
    }

    fn editable_ancestors(&self, node: LayerTreeNodeId) -> Result<Vec<GroupId>, LayerTreeError> {
        if node == LayerTreeNodeId::Group(self.root.id) {
            return Err(LayerTreeError::CannotEditRoot);
        }
        self.ancestors(node)
            .ok_or(LayerTreeError::UnknownNode(node))
    }
}

fn validate_children(
    group: &GroupNode,
    depth: usize,
    ids: &mut BTreeSet<LayerTreeNodeId>,
) -> Result<(), LayerTreeError> {
    if depth > MAX_LAYER_TREE_DEPTH {
        return Err(LayerTreeError::TooDeep);
    }
    for child in &group.children {
        if !ids.insert(child.id()) {
            return Err(LayerTreeError::DuplicateNode(child.id()));
        }
        if let LayerTreeNode::Group(child_group) = child {
            validate_children(child_group, depth + 1, ids)?;
        }
    }
    Ok(())
}

fn ancestors_in_group(group: &GroupNode, target: LayerTreeNodeId) -> Option<Vec<GroupId>> {
    for child in &group.children {
        if child.id() == target {
            return Some(vec![group.id]);
        }
        if let LayerTreeNode::Group(child_group) = child
            && let Some(mut ancestors) = ancestors_in_group(child_group, target)
        {
            ancestors.push(group.id);
            return Some(ancestors);
        }
    }
    None
}

fn contains_locked_raster(node: &LayerTreeNode) -> bool {
    match node {
        LayerTreeNode::Raster(layer) => layer.locked,
        LayerTreeNode::Group(group) => group.children.iter().any(contains_locked_raster),
    }
}

fn find_node(group: &GroupNode, target: LayerTreeNodeId) -> Option<&LayerTreeNode> {
    group.children.iter().find_map(|node| {
        if node.id() == target {
            Some(node)
        } else if let LayerTreeNode::Group(group) = node {
            find_node(group, target)
        } else {
            None
        }
    })
}

fn find_node_mut(group: &mut GroupNode, target: LayerTreeNodeId) -> Option<&mut LayerTreeNode> {
    for child in &mut group.children {
        if child.id() == target {
            return Some(child);
        }
        if let LayerTreeNode::Group(child_group) = child
            && let Some(found) = find_node_mut(child_group, target)
        {
            return Some(found);
        }
    }
    None
}

fn find_group(group: &GroupNode, target: GroupId) -> Option<&GroupNode> {
    if group.id == target {
        return Some(group);
    }
    group.children.iter().find_map(|child| match child {
        LayerTreeNode::Raster(_) => None,
        LayerTreeNode::Group(child_group) => find_group(child_group, target),
    })
}

fn find_group_mut(group: &mut GroupNode, target: GroupId) -> Option<&mut GroupNode> {
    if group.id == target {
        return Some(group);
    }
    group.children.iter_mut().find_map(|child| match child {
        LayerTreeNode::Raster(_) => None,
        LayerTreeNode::Group(child_group) => find_group_mut(child_group, target),
    })
}

fn group_ref(group: &GroupNode, target: GroupId) -> Result<&GroupNode, LayerTreeError> {
    find_group(group, target).ok_or(LayerTreeError::UnknownNode(LayerTreeNodeId::Group(target)))
}

fn parent_and_index(group: &GroupNode, target: LayerTreeNodeId) -> Option<(GroupId, usize)> {
    for (index, child) in group.children.iter().enumerate() {
        if child.id() == target {
            return Some((group.id, index));
        }
        if let LayerTreeNode::Group(child_group) = child
            && let Some(found) = parent_and_index(child_group, target)
        {
            return Some(found);
        }
    }
    None
}

fn remove_node(group: &mut GroupNode, target: LayerTreeNodeId) -> Option<LayerTreeNode> {
    if let Some(index) = group.children.iter().position(|child| child.id() == target) {
        return Some(group.children.remove(index));
    }
    for child in &mut group.children {
        if let LayerTreeNode::Group(child_group) = child
            && let Some(removed) = remove_node(child_group, target)
        {
            return Some(removed);
        }
    }
    None
}

fn invalidate_all(groups: impl IntoIterator<Item = GroupId>) -> CompositeInvalidation {
    let mut invalidation = CompositeInvalidation::empty();
    for group in groups {
        invalidation.invalidate_all_tiles(group);
    }
    invalidation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(id: u128) -> LayerTreeNode {
        LayerTreeNode::Raster(LayerNode {
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: LayerId(id),
            name: format!("Layer {id}"),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })
    }

    #[test]
    fn reorder_depth_overflow_preserves_artwork_and_boundary_moves_remain_valid() {
        // Product risk: moving a valid subtree must not create a tree that the
        // project decoder rejects on restart, or detach artwork on rejection.
        let group = |id, children| GroupNode {
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: GroupId(id),
            name: format!("Group {id}"),
            visible: true,
            opacity_u16: u16::MAX,
            children,
        };
        let mut chain = group(64, vec![raster(1)]);
        for id in (1..64).rev() {
            chain = group(id, vec![LayerTreeNode::Group(chain)]);
        }
        let movable = group(200, vec![LayerTreeNode::Group(group(201, vec![raster(2)]))]);
        let mut tree = LayerTree::new(group(
            100,
            vec![LayerTreeNode::Group(chain), LayerTreeNode::Group(movable)],
        ))
        .expect("valid depth-64 fixture");
        let before = tree.clone();
        for destination in [63, 64] {
            assert_eq!(
                tree.reorder(
                    LayerTreeNodeId::Group(GroupId(200)),
                    GroupId(destination),
                    0
                ),
                Err(LayerTreeError::TooDeep)
            );
            assert_eq!(tree, before, "rejected depth must preserve the entire tree");
        }
        tree.reorder(LayerTreeNodeId::Group(GroupId(200)), GroupId(62), 0)
            .expect("boundary depth remains supported");
        assert_eq!(
            tree.ancestors(LayerTreeNodeId::Group(GroupId(201)))
                .expect("retained subtree")
                .len(),
            64
        );
        assert!(LayerTree::new(tree.root().clone()).is_ok());
    }

    #[test]
    fn insert_preserves_project_tree_identity_and_failure_atomicity() {
        let mut tree = LayerTree::new(GroupNode {
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: GroupId(100),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![raster(1)],
        })
        .expect("fixture tree is valid");

        tree.insert(GroupId(100), 1, raster(2))
            .expect("unique layer inserts");
        assert_eq!(
            tree.parent_and_index(LayerTreeNodeId::Raster(LayerId(2))),
            Some((GroupId(100), 1))
        );

        let before_rejected_insert = tree.clone();
        assert_eq!(
            tree.insert(GroupId(100), 2, raster(2)),
            Err(LayerTreeError::DuplicateNode(LayerTreeNodeId::Raster(
                LayerId(2)
            )))
        );
        assert_eq!(tree, before_rejected_insert);

        tree.rename(LayerTreeNodeId::Raster(LayerId(2)), " Character ")
            .expect("valid names are normalized");
        let renamed = tree.clone();
        assert_eq!(
            tree.rename(LayerTreeNodeId::Raster(LayerId(2)), "   "),
            Err(LayerTreeError::InvalidName)
        );
        assert_eq!(tree, renamed);
        // Product risk: rejected deletion must not lose a subtree; accepted
        // removal must retain the exact node for history reconstruction.
        for invalid in [
            LayerTreeNodeId::Group(GroupId(100)),
            LayerTreeNodeId::Raster(LayerId(99)),
        ] {
            assert!(tree.remove(invalid).is_err());
            assert_eq!(tree, renamed);
        }
        let removed = tree
            .remove(LayerTreeNodeId::Raster(LayerId(2)))
            .expect("remove existing node");
        tree.insert(GroupId(100), 1, removed)
            .expect("restore exact node");
        assert_eq!(tree, renamed);
    }

    #[test]
    fn shading_metadata_invalidates_ancestors_without_breaking_root_or_clone_identity() {
        // Product risk: stale parent caches change displayed/exported artwork,
        // and invalid root metadata must not be accepted through any entry point.
        let child = GroupNode {
            id: GroupId(2),
            name: "Group".into(),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            children: vec![raster(3)],
        };
        let mut tree = LayerTree::new(GroupNode {
            id: GroupId(1),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            children: vec![LayerTreeNode::Group(child)],
        })
        .unwrap();
        for (node, expected) in [
            (
                LayerTreeNodeId::Raster(LayerId(3)),
                vec![GroupId(1), GroupId(2)],
            ),
            (LayerTreeNodeId::Group(GroupId(2)), vec![GroupId(1)]),
        ] {
            assert_eq!(
                tree.set_clip_to_below(node, true)
                    .unwrap()
                    .all_tiles()
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(tree.set_clip_to_below(node, true).unwrap().is_empty());
            assert_eq!(
                tree.set_blend_mode(node, LayerBlendMode::Multiply)
                    .unwrap()
                    .all_tiles()
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(
                tree.set_blend_mode(node, LayerBlendMode::Multiply)
                    .unwrap()
                    .is_empty()
            );
        }
        tree.set_alpha_locked(LayerId(3), true).unwrap();
        tree.duplicate_raster(LayerId(3), LayerId(4)).unwrap();
        let copy = tree.raster(LayerId(4)).unwrap();
        assert!(copy.alpha_locked && copy.clip_to_below);
        assert_eq!(copy.blend_mode, LayerBlendMode::Multiply);
        let original = tree.clone();
        for node in [
            LayerTreeNodeId::Group(GroupId(1)),
            LayerTreeNodeId::Raster(LayerId(99)),
        ] {
            assert!(tree.set_clip_to_below(node, true).is_err());
            assert!(tree.set_blend_mode(node, LayerBlendMode::Multiply).is_err());
            assert_eq!(tree, original);
        }
        assert!(tree.set_alpha_locked(LayerId(99), true).is_err());
        assert_eq!(tree, original);
        for mutate_clip in [false, true] {
            let mut root = tree.root().clone();
            if mutate_clip {
                root.clip_to_below = true;
            } else {
                root.blend_mode = LayerBlendMode::Multiply;
            }
            assert_eq!(
                LayerTree::new(root),
                Err(LayerTreeError::InvalidRootProperties)
            );
        }
    }
}
