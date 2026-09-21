use crate::UiHost;
use dioxus::prelude::*;
use nyatidraw_api::{EditorCommand, ToolCommand};

#[component]
pub fn ColorPicker(color: [u8; 4], epoch: u64) -> Element {
    let live_ink = use_context::<UiHost>();
    let rgba = format!("{},{},{},{}", color[0], color[1], color[2], color[3]);
    use_effect(move || {
        let live_ink = live_ink.clone();
        spawn(async move {
            let mut events = document::eval(include_str!("color_picker.js"));
            while let Ok([value, epoch, previous_color]) = events.recv::<[String; 3]>().await {
                let Some(values) = value
                    .split(',')
                    .map(|part| part.parse::<u8>().ok())
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                let (Ok(color), Ok(epoch)) = (<[u8; 4]>::try_from(values), epoch.parse::<u64>())
                else {
                    continue;
                };
                let projection = live_ink.protocol_snapshot().0;
                let current_color = projection
                    .brush_color
                    .map(|channel| channel.to_string())
                    .join(",");
                if color[3] == 0
                    || epoch != live_ink.project_epoch()
                    || previous_color != current_color
                {
                    continue;
                }
                let result = live_ink.push_editor_command(
                    projection.revision,
                    EditorCommand::Tool(ToolCommand::SetColor(color)),
                );
                println!("desktop-color event=commit rgba={color:?} admission={result:?}");
            }
        });
    });
    rsx! {
        div { id: "color-picker", "data-color-rgba": "{rgba}", "data-color-epoch": "{epoch}",
            div { class: "wheel-wrap",
                div { class: "color-wheel", "data-color-axis": "h", tabindex: 0, role: "slider", aria_label: "색조", aria_valuemin: 0, aria_valuemax: 359,
                    title: "색조: 드래그 또는 방향키 (Shift: 10도)",
                    span { class: "wheel-inner" }
                    div { class: "sv-square", "data-color-axis": "sv", tabindex: 0, role: "group", aria_label: "채도와 명도",
                        title: "채도: 좌우, 명도: 위아래 (Shift: 10%). Home/End: 채도 최소/최대",
                        span { class: "sv-picker" }
                    }
                    span { class: "wheel-picker" }
                }
                div { class: "value-strip", "data-color-axis": "v", tabindex: 0, role: "slider", aria_label: "명도", aria_valuemin: 0, aria_valuemax: 100,
                    title: "명도: 드래그 또는 방향키. Home/End: 최소/최대",
                    span { class: "value-picker" }
                }
            }
        }
    }
}
