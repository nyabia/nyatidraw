//! WebView-owned pointer capture emits only a final semantic dock command.
use crate::live_ink::LiveInkBridge;
use dioxus::prelude::*;
use nyatidraw_api::{DockCommand, DockPosition, EditorCommand, PanelKind, Revision};

pub(super) fn use_dock_drag(live_ink: LiveInkBridge) {
    use_effect(move || {
        let live_ink = live_ink.clone();
        spawn(async move {
            let probe = cancel_probe_enabled();
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
        "color" => PanelKind::Color,
        "history" => PanelKind::History,
        "canvas-actions" => PanelKind::CanvasActions,
        "viewport" => PanelKind::Viewport,
        "quick-colors" => PanelKind::QuickColors,
        _ => return None,
    })
}

// Controlled cancellation signals supplement actual computer-use drags. They
// are not evidence of physical capture loss or a real OS focus transition.
fn cancel_probe_enabled() -> bool {
    if std::env::var("NAYATI_DOCK_CANCEL_PROBE").as_deref() != Ok("sequence") {
        return false;
    }
    let Some(project) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
        return false;
    };
    project
        .file_name()
        .is_some_and(|name| name == "edit-source-scratch.ntdr")
        && project
            .parent()
            .is_some_and(|parent| parent.join(".nyatidraw-scratch-dock-probe").is_file())
}
