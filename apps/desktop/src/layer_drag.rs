//! `WebView` layer drag/drop sends one revision-bound semantic reorder on drop.
use crate::live_ink::LiveInkBridge;
use dioxus::prelude::*;
use nyatidraw_api::{
    EditorCommand, GroupId, LayerCommand, LayerProjection, LayerTreeNodeId, Revision, UiProjection,
};

pub(super) fn use_layer_drag_probe() {
    use_effect(|| {
        let Ok(mode) = std::env::var("NAYATI_LAYER_DRAG_PROBE") else {
            return;
        };
        if !matches!(
            mode.as_str(),
            "inside" | "below" | "above" | "group" | "cancel" | "stale"
        ) {
            eprintln!("desktop-layers event=drag-probe-failed mode={mode} error=unsupported-mode");
            return;
        }
        let Some(project) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
            eprintln!(
                "desktop-layers event=drag-probe-failed mode={mode} error=missing-project-argument"
            );
            return;
        };
        // Windows canonical paths may carry a verbatim prefix; compare both
        // sides in the same representation and fail closed on lookup errors.
        let (Ok(project), Ok(temporary)) =
            (project.canonicalize(), std::env::temp_dir().canonicalize())
        else {
            eprintln!(
                "desktop-layers event=drag-probe-failed mode={mode} error=scratch-path-resolution"
            );
            return;
        };
        let Some(directory) = project.parent() else {
            eprintln!(
                "desktop-layers event=drag-probe-failed mode={mode} error=missing-project-parent"
            );
            return;
        };
        if !directory.starts_with(&temporary)
            || !directory.join(".nyatidraw-scratch-export-probe").is_file()
        {
            eprintln!(
                "desktop-layers event=drag-probe-failed mode={mode} error=unmarked-or-outside-temp"
            );
            return;
        }
        println!("desktop-layers event=drag-probe-started mode={mode}");
        spawn(async move {
            let script = include_str!("layer_drag_probe.js").replace("__PROBE_MODE__", &mode);
            let result = document::eval(&script).recv::<String>().await;
            match result {
                Ok(result) => println!(
                    "desktop-layers event=drag-probe-complete mode={mode} result={result} browser_events=synthetic physical_drag_proof=false"
                ),
                Err(error) => {
                    eprintln!("desktop-layers event=drag-probe-failed mode={mode} error={error}");
                }
            }
        });
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LayerDrag {
    pub node: LayerTreeNodeId,
    parent: GroupId,
    index: usize,
    revision: Revision,
    hovered: Option<(LayerTreeNodeId, DropPosition)>,
}

impl LayerDrag {
    pub fn begin(layer: &LayerProjection, revision: Revision) -> Self {
        Self {
            node: layer.id,
            parent: layer.parent,
            index: layer.index,
            revision,
            hovered: None,
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

#[component]
pub(super) fn LayerDropTargets(
    layer: LayerProjection,
    projection: Signal<UiProjection>,
    drag: Signal<Option<LayerDrag>>,
    error: Signal<Option<String>>,
) -> Element {
    if drag.read().is_none() {
        return rsx! {};
    }
    let group = matches!(layer.id, LayerTreeNodeId::Group(_));
    rsx! {
        div { class: if group { "layer-drop-targets group" } else { "layer-drop-targets" },
            LayerDropZone { layer: layer.clone(), position: DropPosition::Above, projection, drag, error }
            if group {
                LayerDropZone { layer: layer.clone(), position: DropPosition::Inside, projection, drag, error }
            }
            LayerDropZone { layer, position: DropPosition::Below, projection, drag, error }
        }
    }
}

#[component]
fn LayerDropZone(
    layer: LayerProjection,
    position: DropPosition,
    projection: Signal<UiProjection>,
    mut drag: Signal<Option<LayerDrag>>,
    mut error: Signal<Option<String>>,
) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let command = (*drag.read())
        .and_then(|source| drop_command(&projection.read(), source, &layer, position));
    let valid = command.is_some();
    let target = (layer.id, position);
    let active = valid
        && drag
            .read()
            .is_some_and(|source| source.hovered == Some(target));
    let label = match position {
        DropPosition::Above => "위로 이동",
        DropPosition::Inside => "그룹 안에 넣기",
        DropPosition::Below => "아래로 이동",
    };
    let position_class = match position {
        DropPosition::Above => "above",
        DropPosition::Inside => "inside",
        DropPosition::Below => "below",
    };
    rsx! {
        div {
            class: "layer-drop-zone {position_class}", class: if active { "active" } else if valid { "available" } else { "unavailable" },
            title: "{label}",
            ondragover: move |event| {
                event.stop_propagation();
                if valid { event.prevent_default(); }
                let current = *drag.read();
                if let Some(mut source) = current {
                    let hovered = valid.then_some(target);
                    if source.hovered != hovered {
                        source.hovered = hovered;
                        drag.set(Some(source));
                    }
                }
            },
            ondragleave: move |_| {
                let current = *drag.read();
                if let Some(mut source) = current && source.hovered == Some(target) {
                    source.hovered = None;
                    drag.set(Some(source));
                }
            },
            ondrop: move |event| {
                event.prevent_default();
                event.stop_propagation();
                let current = *drag.read();
                drag.set(None);
                if let Some(source) = current {
                    let current_projection = projection.read();
                    if let Some(command) = drop_command(&current_projection, source, &layer, position) {
                        match live_ink.push_editor_command(source.revision, EditorCommand::Layer(command)) {
                            Ok(_) => error.set(None),
                            Err(reason) => error.set(Some(format!("레이어를 이동하지 못했습니다: {reason:?}"))),
                        }
                    } else if source.revision != current_projection.revision {
                        error.set(Some("드래그 중 문서가 변경됐습니다. 다시 끌어 주세요.".into()));
                    }
                }
            },
            if active { span { "{label}" } }
        }
    }
}
