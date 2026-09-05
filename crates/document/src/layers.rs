use std::collections::BTreeSet;

use nyatidraw_api::{
    CompositeInvalidation, ContentRootId, GroupId, LayerId, LayerTreeNodeId, TileCoordinate,
};

const MAX_LAYER_TREE_DEPTH: usize = 64;
const MAX_LAYER_NAME_CHARS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerNode {
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
        if !root.visible || root.opacity_u16 != u16::MAX {
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
    /// Rejects the implicit root or unknown IDs without changing the tree.
    pub fn remove(&mut self, node: LayerTreeNodeId) -> Result<LayerTreeNode, LayerTreeError> {
        if node == LayerTreeNodeId::Group(self.root.id) {
            return Err(LayerTreeError::CannotEditRoot);
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
}
