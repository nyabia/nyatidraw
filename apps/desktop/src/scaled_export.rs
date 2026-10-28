#[cfg(windows)]
use dioxus::prelude::*;
#[cfg(windows)]
use nyatidraw_api::{EditorCommand, ProjectCommand};

#[cfg(windows)]
use crate::live_ink::LiveInkBridge;

#[cfg(windows)]
#[component]
pub(crate) fn ScaledExportButton() -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let window = dioxus_desktop::window().window.clone();
    let mut busy = use_signal(|| false);
    let projection = live_ink.protocol_snapshot().0;
    let canvas = projection.canvas;
    rsx! {
        nyatidraw_editor_ui::scaled_export::ScaledExportButton {
            source: [canvas.width_px, canvas.height_px], disabled: busy() || projection.edit.busy || projection.edit.transform.is_some() || projection.edit.color_adjustment.is_some(),
            onexport: move |size: [u32; 2]| {
                let live_ink = live_ink.clone();
                let window = window.clone();
                let epoch = live_ink.project_epoch();
                spawn(async move {
                    if busy() { return; }
                    busy.set(true);
                    let selected = rfd::AsyncFileDialog::new().set_parent(window.as_ref())
                        .set_title("PNG 크기 지정 · 새 파일로 내보내기")
                        .add_filter("PNG 이미지", &["png"])
                        .set_file_name(format!("그림-{}x{}.png", size[0], size[1]))
                        .save_file().await;
                    if let Some(file) = selected {
                        let mut path = file.path().to_path_buf();
                        if path.extension().is_none() { path.set_extension("png"); }
                        let result = if live_ink.project_epoch() == epoch {
                            validate_target(&path).and_then(|()| {
                                live_ink.push_ui_editor_command(EditorCommand::Project(ProjectCommand::ExportScaledPng { path, size, project_epoch: epoch }))
                                    .map(|_| ()).map_err(|_| "현재 작업을 마친 뒤 다시 내보내주세요.".into())
                            })
                        } else {
                            Err("그림이 바뀌어 내보내기를 취소했습니다.".into())
                        };
                        if let Err(error) = result { live_ink.publish_activation_notice(error); }
                    }
                    busy.set(false);
                });
            }
        }
    }
}

pub(crate) fn validate_target(path: &std::path::Path) -> Result<(), String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        return Err(".png 확장자의 새 파일 이름을 선택해주세요.".into());
    }
    if path.exists() || path.with_extension("ntdr").exists() {
        return Err("원본과 기존 파일을 보존하려면 다른 PNG 이름을 선택해주세요.".into());
    }
    Ok(())
}
