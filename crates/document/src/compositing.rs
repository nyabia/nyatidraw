//! Backend-neutral stack binding and dependency-aware source participation.
use std::collections::BTreeSet;

use nyatidraw_api::LayerTreeNodeId;

use crate::{GroupNode, LayerTreeNode};

pub struct LayerStack<'a> {
    pub base: &'a LayerTreeNode,
    pub clips: &'a [LayerTreeNode],
}

/// Resolves original sibling order before visibility filtering. Leading orphan
/// clips produce no stack and never bind across a parent boundary.
pub fn layer_stacks(group: &GroupNode) -> impl Iterator<Item = LayerStack<'_>> {
    let mut index = 0;
    std::iter::from_fn(move || {
        while index < group.children.len() && clipped(&group.children[index]) {
            index += 1;
        }
        let base = group.children.get(index)?;
        index += 1;
        let start = index;
        while index < group.children.len() && clipped(&group.children[index]) {
            index += 1;
        }
        Some(LayerStack {
            base,
            clips: &group.children[start..index],
        })
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositionScope {
    Solo(LayerTreeNodeId),
    Reference,
}

/// Computes source participation including transitive clipping bases. This is
/// presentation-only: original flags and sibling positions are not changed.
/// Solo reveals its target/ancestor/base path, but group contents retain their
/// own visibility. Reference scope never reveals durably hidden artwork.
#[must_use]
pub fn composition_scope_nodes(
    root: &GroupNode,
    scope: CompositionScope,
) -> BTreeSet<LayerTreeNodeId> {
    let mut selected = BTreeSet::new();
    // Only the explicitly soloed node and its ancestor path may reveal a
    // hidden clipping base. Descendants included as group/base context still
    // obey their saved visibility, including hidden bases of visible clips.
    let mut revealed_path = BTreeSet::new();
    match scope {
        CompositionScope::Solo(target) => {
            if target == LayerTreeNodeId::Group(root.id) {
                visible_children(root, &mut selected);
            } else {
                seed_solo(root, target, &mut selected, &mut revealed_path);
            }
        }
        CompositionScope::Reference => {
            seed_references(root, true, &mut selected);
        }
    }
    loop {
        let before = selected.len();
        dependencies(root, &revealed_path, &mut selected);
        if selected.len() == before {
            break;
        }
    }
    selected
}

fn clipped(node: &LayerTreeNode) -> bool {
    match node {
        LayerTreeNode::Raster(layer) => layer.clip_to_below,
        LayerTreeNode::Group(group) => group.clip_to_below,
    }
}

fn visible(node: &LayerTreeNode) -> bool {
    match node {
        LayerTreeNode::Raster(layer) => layer.visible,
        LayerTreeNode::Group(group) => group.visible,
    }
}

fn visible_children(group: &GroupNode, selected: &mut BTreeSet<LayerTreeNodeId>) {
    for child in &group.children {
        if visible(child) {
            include_subtree(child, selected);
        }
    }
}

fn include_subtree(node: &LayerTreeNode, selected: &mut BTreeSet<LayerTreeNodeId>) {
    selected.insert(node.id());
    if let LayerTreeNode::Group(group) = node {
        visible_children(group, selected);
    }
}

fn seed_solo(
    group: &GroupNode,
    target: LayerTreeNodeId,
    selected: &mut BTreeSet<LayerTreeNodeId>,
    revealed_path: &mut BTreeSet<LayerTreeNodeId>,
) -> bool {
    for child in &group.children {
        if child.id() == target {
            include_subtree(child, selected);
            revealed_path.insert(child.id());
            return true;
        }
        if let LayerTreeNode::Group(nested) = child
            && seed_solo(nested, target, selected, revealed_path)
        {
            selected.insert(child.id());
            revealed_path.insert(child.id());
            return true;
        }
    }
    false
}

fn seed_references(
    group: &GroupNode,
    ancestors_visible: bool,
    selected: &mut BTreeSet<LayerTreeNodeId>,
) -> bool {
    let active = ancestors_visible && group.visible && group.opacity_u16 != 0;
    let mut found = false;
    for child in &group.children {
        let wanted = match child {
            LayerTreeNode::Raster(layer) => {
                active && layer.visible && layer.opacity_u16 != 0 && layer.reference
            }
            LayerTreeNode::Group(nested) => seed_references(nested, active, selected),
        };
        if wanted {
            selected.insert(child.id());
            found = true;
        }
    }
    found
}

fn dependencies(
    group: &GroupNode,
    revealed_path: &BTreeSet<LayerTreeNodeId>,
    selected: &mut BTreeSet<LayerTreeNodeId>,
) {
    for stack in layer_stacks(group) {
        let reveal_base = stack
            .clips
            .iter()
            .any(|node| revealed_path.contains(&node.id()));
        if stack.clips.iter().any(|node| selected.contains(&node.id()))
            && (reveal_base || visible(stack.base))
        {
            include_subtree(stack.base, selected);
        }
    }
    for child in &group.children {
        if selected.contains(&child.id())
            && let LayerTreeNode::Group(nested) = child
        {
            dependencies(nested, revealed_path, selected);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayerNode, LayerTree};
    use nyatidraw_api::{ContentRootId, GroupId, LayerId};

    fn raster(id: u128, clip: bool, visible: bool, reference: bool) -> LayerTreeNode {
        LayerTreeNode::Raster(LayerNode {
            id: LayerId(id),
            name: id.to_string(),
            visible,
            reference,
            locked: false,
            alpha_locked: false,
            clip_to_below: clip,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn clipping_dependencies_preserve_bindings_hidden_bases_and_unrelated_artwork() {
        // Product risk: filtering or Solo silently rebinds clipped artwork,
        // reveals hidden reference bases, or includes unrelated painted clips.
        for hidden_base in [false, true] {
            let root = GroupNode {
                id: GroupId(100),
                name: "root".into(),
                visible: true,
                opacity_u16: u16::MAX,
                clip_to_below: false,
                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                children: vec![
                    raster(0, true, true, false),
                    raster(1, false, true, false),
                    raster(2, true, true, false),
                    LayerTreeNode::Group(GroupNode {
                        id: GroupId(4),
                        name: "clipped group".into(),
                        visible: true,
                        opacity_u16: u16::MAX,
                        clip_to_below: true,
                        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                        children: vec![
                            raster(5, false, !hidden_base, false),
                            raster(6, true, true, true),
                            raster(8, true, true, false),
                        ],
                    }),
                    raster(7, false, true, false),
                ],
            };
            let tree = LayerTree::new(root).unwrap();
            let stacks: Vec<_> = layer_stacks(tree.root()).collect();
            assert_eq!(stacks.len(), 2);
            assert_eq!(stacks[0].base.id(), LayerTreeNodeId::Raster(LayerId(1)));
            assert_eq!(stacks[0].clips.len(), 2);
            let target = LayerTreeNodeId::Raster(LayerId(6));
            let solo = composition_scope_nodes(tree.root(), CompositionScope::Solo(target));
            let expected: BTreeSet<_> = [
                LayerTreeNodeId::Raster(LayerId(1)),
                LayerTreeNodeId::Group(GroupId(4)),
                LayerTreeNodeId::Raster(LayerId(5)),
                target,
            ]
            .into_iter()
            .collect();
            assert_eq!(solo, expected);
            let reference = composition_scope_nodes(tree.root(), CompositionScope::Reference);
            let mut expected_reference = expected;
            if hidden_base {
                expected_reference.remove(&LayerTreeNodeId::Raster(LayerId(5)));
            }
            assert_eq!(reference, expected_reference);
            // Removing the first base leaves true orphans, not a cross-parent binding.
            let mut orphaned = tree.clone();
            orphaned
                .remove(LayerTreeNodeId::Raster(LayerId(1)))
                .unwrap();
            assert_eq!(layer_stacks(orphaned.root()).count(), 1);
            assert_eq!(
                layer_stacks(orphaned.root()).next().unwrap().base.id(),
                LayerTreeNodeId::Raster(LayerId(7))
            );
            assert!(
                !composition_scope_nodes(orphaned.root(), CompositionScope::Solo(target))
                    .contains(&LayerTreeNodeId::Raster(LayerId(7)))
            );
        }

        // A group supplied as clipping-base context must not reveal hidden
        // internal artwork merely because its visible children include clips.
        // Explicitly soloing that internal clip still reveals its own base.
        let contextual = GroupNode {
            id: GroupId(100),
            name: "root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            children: vec![
                LayerTreeNode::Group(GroupNode {
                    id: GroupId(10),
                    name: "contextual base".into(),
                    visible: true,
                    opacity_u16: u16::MAX,
                    clip_to_below: false,
                    blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                    children: vec![
                        raster(11, false, false, false),
                        raster(12, true, true, false),
                    ],
                }),
                raster(13, true, true, false),
            ],
        };
        for (target, reveals_inner_base, includes_outer_clip) in [
            (LayerTreeNodeId::Group(GroupId(100)), false, true),
            (LayerTreeNodeId::Group(GroupId(10)), false, false),
            (LayerTreeNodeId::Raster(LayerId(13)), false, true),
            (LayerTreeNodeId::Raster(LayerId(12)), true, false),
        ] {
            let scope = composition_scope_nodes(&contextual, CompositionScope::Solo(target));
            assert!(scope.contains(&LayerTreeNodeId::Group(GroupId(10))));
            assert!(scope.contains(&LayerTreeNodeId::Raster(LayerId(12))));
            assert_eq!(
                scope.contains(&LayerTreeNodeId::Raster(LayerId(11))),
                reveals_inner_base,
                "hidden internal base for {target:?}"
            );
            assert_eq!(
                scope.contains(&LayerTreeNodeId::Raster(LayerId(13))),
                includes_outer_clip
            );
        }
    }
}
