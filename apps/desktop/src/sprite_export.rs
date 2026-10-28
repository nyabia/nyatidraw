#[cfg(windows)]
use dioxus::prelude::*;

#[cfg(windows)]
#[component]
pub(crate) fn SpriteExportButton() -> Element {
    let live_ink = use_context::<crate::live_ink::LiveInkBridge>();
    let window = dioxus_desktop::window().window.clone();
    let mut busy = use_signal(|| false);
    let projection = live_ink.protocol_snapshot().0;
    rsx! {
        button {
            class: "command",
            disabled: busy() || projection.animation.frames.is_empty() || projection.animation.playing
                || projection.edit.busy || projection.edit.transform.is_some() || projection.edit.color_adjustment.is_some(),
            title: "모든 프레임을 새 .frames 폴더의 PNG·JSON으로 내보내기",
            onclick: move |_| {
                let live_ink = live_ink.clone();
                let window = window.clone();
                let epoch = live_ink.project_epoch();
                spawn(async move {
                    if busy() { return; }
                    busy.set(true);
                    let selected = rfd::AsyncFileDialog::new().set_parent(window.as_ref())
                        .set_title("스프라이트 시트 · 같은 이름의 새 .frames 폴더 만들기")
                        .add_filter("스프라이트 시트 이름", &["json"])
                        .set_file_name("그림-시트.json").save_file().await;
                    if let Some(file) = selected {
                        let directory = file.path().with_extension("frames");
                        let result = if live_ink.project_epoch() == epoch {
                            validate_target(&directory).and_then(|()| {
                                live_ink.push_ui_editor_command(nyatidraw_api::EditorCommand::Project(
                                    nyatidraw_api::ProjectCommand::ExportSpriteSheet { directory, columns: None, project_epoch: epoch }
                                )).map(|_| ()).map_err(|_| "현재 작업을 마친 뒤 다시 내보내주세요.".into())
                            })
                        } else {
                            Err("그림이 바뀌어 내보내기를 취소했습니다.".into())
                        };
                        if let Err(error) = result { live_ink.publish_activation_notice(error); }
                    }
                    busy.set(false);
                });
            },
            "스프라이트 시트 내보내기"
        }
    }
}

pub(crate) fn validate_target(directory: &std::path::Path) -> Result<(), String> {
    if directory.exists() {
        return Err("기존 파일을 보존하려면 새 스프라이트 시트 폴더 이름을 선택해주세요.".into());
    }
    Ok(())
}
