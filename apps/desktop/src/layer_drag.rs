//! `WebView` layer drag/drop sends one revision-bound semantic reorder on drop.
use crate::live_ink::LiveInkBridge;
use dioxus::prelude::*;
use nyatidraw_api::{
    EditorCommand, GroupId, LayerCommand, LayerProjection, LayerTreeNodeId, Revision, UiProjection,
};

// Pointer capture does not require the composition WebView's OS/OLE drag host.
// Only a completed, revision-bound semantic drop crosses IPC, never pointer moves.
pub(super) fn use_layer_pointer_drag(mut error: Signal<Option<String>>) {
    let live_ink = use_context::<LiveInkBridge>();
    use_effect(move || {
        let live_ink = live_ink.clone();
        spawn(async move {
            let mut events = document::eval(include_str!("layer_pointer_drag.js"));
            while let Ok([source, target, position, revision]) = events.recv::<[String; 4]>().await
            {
                let (Some(source), Some(target), Ok(revision)) = (
                    node_key(&source),
                    node_key(&target),
                    revision.parse::<u64>(),
                ) else {
                    continue;
                };
                let position = match position.as_str() {
                    "above" => DropPosition::Above,
                    "inside" => DropPosition::Inside,
                    "below" => DropPosition::Below,
                    _ => continue,
                };
                let current = live_ink.protocol_snapshot().0;
                if current.revision != Revision(revision) {
                    error.set(Some(
                        "드래그 중 문서가 변경됐습니다. 다시 끌어 주세요.".into(),
                    ));
                    continue;
                }
                let (Some(source), Some(target)) = (
                    current.layers.iter().find(|layer| layer.id == source),
                    current.layers.iter().find(|layer| layer.id == target),
                ) else {
                    continue;
                };
                let drag = LayerDrag::begin(source, current.revision);
                if let Some(command) = drop_command(&current, drag, target, position) {
                    match live_ink
                        .push_editor_command(current.revision, EditorCommand::Layer(command))
                    {
                        Ok(_) => error.set(None),
                        Err(reason) => {
                            error.set(Some(format!("레이어를 이동하지 못했습니다: {reason:?}")));
                        }
                    }
                }
            }
        });
    });
}

fn node_key(key: &str) -> Option<LayerTreeNodeId> {
    let (kind, id) = key.split_once('-')?;
    let id = id.parse().ok()?;
    match kind {
        "r" => Some(LayerTreeNodeId::Raster(nyatidraw_api::LayerId(id))),
        "g" => Some(LayerTreeNodeId::Group(GroupId(id))),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LayerDrag {
    pub node: LayerTreeNodeId,
    parent: GroupId,
    index: usize,
    revision: Revision,
}

impl LayerDrag {
    pub fn begin(layer: &LayerProjection, revision: Revision) -> Self {
        Self {
            node: layer.id,
            parent: layer.parent,
            index: layer.index,
            revision,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DropPosition {
    Above,
    Inside,
    Below,
}

fn adjacent_move(projection: &UiProjection, upward: bool) -> Option<LayerCommand> {
    let active = LayerTreeNodeId::Raster(projection.active_layer?);
    let layer = projection.layers.iter().find(|layer| layer.id == active)?;
    let count = projection
        .layers
        .iter()
        .filter(|other| other.parent == layer.parent)
        .count();
    let index = if upward {
        layer.index.checked_add(1).filter(|index| *index < count)?
    } else {
        layer.index.checked_sub(1)?
    };
    Some(LayerCommand::Reorder {
        node: active,
        new_parent: layer.parent,
        index,
    })
}

#[component]
pub(super) fn LayerMoveButtons(
    projection: Signal<UiProjection>,
    error: Signal<Option<String>>,
) -> Element {
    rsx! {
        for upward in [true, false] {
            LayerMoveButton { projection, error, upward }
        }
    }
}

#[component]
fn LayerMoveButton(
    projection: Signal<UiProjection>,
    mut error: Signal<Option<String>>,
    upward: bool,
) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let enabled = adjacent_move(&projection.read(), upward).is_some();
    rsx! {
        button {
            disabled: !enabled, title: if upward { "선택 레이어를 한 칸 위로" } else { "선택 레이어를 한 칸 아래로" },
            onclick: move |_| {
                let current = projection.read();
                if let Some(command) = adjacent_move(&current, upward) {
                    match live_ink.push_editor_command(current.revision, EditorCommand::Layer(command)) {
                        Ok(_) => error.set(None),
                        Err(reason) => error.set(Some(format!("레이어를 이동하지 못했습니다: {reason:?}"))),
                    }
                }
            },
            if upward { "↑" } else { "↓" }
        }
    }
}

fn drop_command(
    projection: &UiProjection,
    drag: LayerDrag,
    target: &LayerProjection,
    position: DropPosition,
) -> Option<LayerCommand> {
    if projection.revision != drag.revision || drag.node == target.id {
        return None;
    }
    let (parent, mut index) = match position {
        DropPosition::Above => (target.parent, target.index + 1),
        DropPosition::Below => (target.parent, target.index),
        DropPosition::Inside => {
            let LayerTreeNodeId::Group(group) = target.id else {
                return None;
            };
            (
                group,
                projection
                    .layers
                    .iter()
                    .filter(|layer| layer.parent == group)
                    .count(),
            )
        }
    };
    // Do not advertise a drop into the moving group's own descendants.
    let mut ancestor = parent;
    for _ in 0..=projection.layers.len() {
        if drag.node == LayerTreeNodeId::Group(ancestor) {
            return None;
        }
        let Some(layer) = projection
            .layers
            .iter()
            .find(|layer| layer.id == LayerTreeNodeId::Group(ancestor))
        else {
            break;
        };
        ancestor = layer.parent;
    }
    if drag.parent == parent && drag.index < index {
        index -= 1;
    }
    if drag.parent == parent && drag.index == index {
        return None;
    }
    Some(LayerCommand::Reorder {
        node: drag.node,
        new_parent: parent,
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_indices_preserve_compositing_order_and_reject_stale_or_cyclic_moves() {
        // Product risk: reversed visual order or a bad reparent index changes
        // artwork composition; stale/cyclic drops must never mutate the tree.
        let row = |key, parent, index| {
            let id = node_key(key).unwrap();
            LayerProjection {
                id,
                parent: GroupId(parent),
                index,
                depth: 1,
                kind: if matches!(id, LayerTreeNodeId::Group(_)) {
                    nyatidraw_api::LayerProjectionKind::Group
                } else {
                    nyatidraw_api::LayerProjectionKind::Raster
                },
                name: key.into(),
                visible: true,
                reference: false,
                opacity_u16: u16::MAX,
            }
        };
        let projection = UiProjection {
            revision: Revision(12),
            layers: vec![
                row("r-1", 100, 0),
                row("r-2", 100, 1),
                row("g-10", 100, 2),
                row("g-11", 10, 0),
            ],
            ..UiProjection::default()
        };
        for (source, target, position, destination) in [
            (0, 1, DropPosition::Above, Some((100, 1))),
            (1, 0, DropPosition::Below, Some((100, 0))),
            (0, 2, DropPosition::Inside, Some((10, 1))),
            (2, 3, DropPosition::Inside, None),
            (0, 0, DropPosition::Above, None),
            (0, 1, DropPosition::Below, None),
        ] {
            let drag = LayerDrag::begin(&projection.layers[source], projection.revision);
            let expected = destination.map(|(parent, index)| LayerCommand::Reorder {
                node: drag.node,
                new_parent: GroupId(parent),
                index,
            });
            assert_eq!(
                drop_command(&projection, drag, &projection.layers[target], position),
                expected
            );
            let stale = LayerDrag {
                revision: Revision(11),
                ..drag
            };
            assert_eq!(
                drop_command(&projection, stale, &projection.layers[target], position),
                None
            );
        }
    }
}
