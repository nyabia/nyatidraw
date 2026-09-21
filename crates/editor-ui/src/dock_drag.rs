//! WebView-owned pointer capture emits only a final semantic dock command.
use crate::UiHost;
use dioxus::prelude::*;
use nyatidraw_api::{
    DockAxis, DockCommand, DockNode, DockPosition, DockTree, EditorCommand, PanelKind, Revision,
};

pub fn split_key(first: &DockNode, second: &DockNode) -> String {
    fn anchor(node: &DockNode) -> PanelKind {
        match node {
            DockNode::Panel(panel) => *panel,
            DockNode::Tabs { panels, .. } => panels[0],
            DockNode::Split { first, .. } => anchor(first),
        }
    }
    format!(
        "{}|{}",
        crate::panel_slug(anchor(first)),
        crate::panel_slug(anchor(second))
    )
}

pub fn height_panel(node: &DockNode) -> PanelKind {
    match node {
        DockNode::Panel(panel) => *panel,
        DockNode::Tabs { panels, .. } => panels[0],
        DockNode::Split { first, .. } => height_panel(first),
    }
}

pub fn minimum_width(node: &DockNode) -> u16 {
    match node {
        DockNode::Panel(panel) => match panel {
            PanelKind::Tools => 48,
            PanelKind::Brush | PanelKind::ToolProperties | PanelKind::BrushSizes => 224,
            PanelKind::Canvas => 240,
            PanelKind::Layers => 280,
            _ => 160,
        },
        DockNode::Tabs { panels, .. } => panels
            .iter()
            .map(|panel| minimum_width(&DockNode::Panel(*panel)))
            .max()
            .unwrap_or(160),
        DockNode::Split {
            axis,
            first,
            second,
            ..
        } => {
            let first = minimum_width(first);
            let second = minimum_width(second);
            if *axis == DockAxis::Horizontal {
                first.saturating_add(second).saturating_add(6)
            } else {
                first.max(second)
            }
        }
    }
}

fn resize_split(node: &mut DockNode, key: &str, ratio: u16) -> bool {
    if let DockNode::Split {
        axis,
        first_per_mille,
        first,
        second,
    } = node
    {
        if *axis == DockAxis::Horizontal && split_key(first, second) == key {
            *first_per_mille = ratio;
            return true;
        }
        return resize_split(first, key, ratio) || resize_split(second, key, ratio);
    }
    false
}

#[allow(clippy::too_many_lines)] // One revision-checked semantic event dispatcher.
pub fn use_dock_drag(live_ink: UiHost) {
    let mut heights = use_context_provider(|| Signal::new(live_ink.panel_heights()));
    use_effect(move || {
        let live_ink = live_ink.clone();
        spawn(async move {
            let probe = live_ink.dock_cancel_probe();
            let script = include_str!("dock_drag.js").replace(
                "__DOCK_CANCEL_PROBE__",
                if probe { "true" } else { "false" },
            );
            let mut events = document::eval(&script);
            while let Ok([source, target, action, revision]) = events.recv::<[String; 4]>().await {
                if matches!(
                    action.as_str(),
                    "capture" | "cancel" | "synthetic-cancel-probe"
                ) {
                    println!("desktop-dock event={action} source={source} detail={target}");
                    continue;
                }
                if action == "resize" {
                    let (Ok(revision), Ok(ratio)) =
                        (revision.parse::<u64>(), target.parse::<u16>())
                    else {
                        continue;
                    };
                    let (current, _) = live_ink.protocol_snapshot();
                    if current.revision != Revision(revision) || !(20..=980).contains(&ratio) {
                        continue;
                    }
                    let mut root = current.dock.root().clone();
                    if !resize_split(&mut root, &source, ratio) {
                        continue;
                    }
                    let Ok(tree) = DockTree::with_top(root, current.dock.top().to_vec()) else {
                        continue;
                    };
                    let result = live_ink.push_editor_command(
                        Revision(revision),
                        EditorCommand::Dock(DockCommand::Replace(tree)),
                    );
                    println!(
                        "desktop-dock event=resize source={source} ratio={ratio} admission={result:?}"
                    );
                    continue;
                }
                if action == "height" {
                    let (Some(panel), Ok(revision), Ok(height)) = (
                        panel(&source),
                        revision.parse::<u64>(),
                        target.parse::<u16>(),
                    ) else {
                        continue;
                    };
                    if live_ink.protocol_snapshot().0.revision != Revision(revision) {
                        continue;
                    }
                    let mut next = heights.peek().clone();
                    match next.set(panel, height) {
                        Ok(()) => {
                            live_ink.submit_panel_heights(&next);
                            heights.set(next);
                        }
                        Err(error) => {
                            eprintln!("desktop-dock event=height-save-failed error={error}");
                        }
                    }
                    continue;
                }
                let (Some(panel), Ok(revision)) = (panel(&source), revision.parse::<u64>()) else {
                    continue;
                };
                let command = if action == "top-row" {
                    let before = if target.is_empty() {
                        None
                    } else if let Some(target) = self::panel(&target) {
                        Some(target)
                    } else {
                        continue;
                    };
                    DockCommand::MoveToolbarToTop { panel, before }
                } else {
                    let Some(target) = self::panel(&target) else {
                        continue;
                    };
                    let position = match action.as_str() {
                        "left" => DockPosition::Left,
                        "top" => DockPosition::Top,
                        "right" => DockPosition::Right,
                        _ => continue,
                    };
                    DockCommand::MovePanel {
                        panel,
                        target,
                        position,
                    }
                };
                let result =
                    live_ink.push_editor_command(Revision(revision), EditorCommand::Dock(command));
                println!(
                    "desktop-dock event=drop source={source} target={target} position={action} admission={result:?}"
                );
            }
        });
    });
}

fn panel(slug: &str) -> Option<PanelKind> {
    Some(match slug {
        "canvas" => PanelKind::Canvas,
        "tools" => PanelKind::Tools,
        "navigator" => PanelKind::Navigator,
        "layers" => PanelKind::Layers,
        "brush" => PanelKind::Brush,
        "tool-properties" => PanelKind::ToolProperties,
        "brush-sizes" => PanelKind::BrushSizes,
        "color" => PanelKind::Color,
        "history" => PanelKind::History,
        "canvas-actions" => PanelKind::CanvasActions,
        "viewport" => PanelKind::Viewport,
        "quick-colors" => PanelKind::QuickColors,
        _ => return None,
    })
}
