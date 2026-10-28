use dioxus::prelude::*;
use js_sys::{Array, Uint8Array};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::{browser, runtime::Editor};
use nyatidraw_editor_ui::UiHost;

#[component]
pub(crate) fn SpriteExportButton() -> Element {
    let editor = use_context::<Editor>();
    let host = use_context::<UiHost>();
    let projection = host.protocol_snapshot().0;
    rsx! {
        button {
            class: "command",
            disabled: !(editor.ready)() || (editor.exporting_png)() || projection.animation.frames.is_empty()
                || projection.animation.playing || projection.edit.busy || projection.edit.transform.is_some()
                || projection.edit.color_adjustment.is_some(),
            title: "모든 프레임을 PNG 시트와 JSON 메타데이터로 내보내기",
            onclick: move |_| download(editor, None),
            "스프라이트 시트 내보내기"
        }
    }
}

pub(crate) fn download(mut editor: Editor, columns: Option<u32>) {
    if (editor.exporting_png)() {
        return;
    }
    let Some(result) = editor.read(|runtime| {
        if runtime.document.is_drawing()
            || runtime.gesture.is_some()
            || runtime.animation_projection().playing
            || runtime.document.transform_projection().is_some()
            || runtime.document.color_adjustment_projection().is_some()
        {
            return Err("현재 획·변형·색 보정 또는 재생을 마친 뒤 내보내주세요.".to_owned());
        }
        if runtime.document.animation_timeline().is_none() {
            return Err("먼저 애니메이션 프레임을 만들어주세요.".to_owned());
        }
        runtime
            .document
            .prepare_recovery(runtime.project_epoch, 1, None)
            .map(|request| (runtime.project_epoch, request.bytes))
    }) else {
        return;
    };
    let (epoch, packet) = match result {
        Ok(value) => value,
        Err(error) => {
            editor.warn(error);
            return;
        }
    };
    editor.exporting_png.set(true);
    editor.report("스프라이트 시트 내보내기 중…");
    let pending =
        browser::export_sprite_sheet_in_worker(&Uint8Array::from(packet.as_slice()), columns);
    spawn_local(async move {
        let result = JsFuture::from(pending).await;
        editor.exporting_png.set(false);
        if editor.read(|runtime| runtime.project_epoch) != Some(epoch) {
            editor.warn("그림이 바뀌어 이전 그림의 시트 내보내기를 취소했습니다.");
            return;
        }
        match result {
            Ok(files) => {
                let files = Array::from(&files);
                if files.length() != 4 {
                    editor.warn("스프라이트 시트 응답이 올바르지 않습니다.");
                    return;
                }
                let (Some(png_name), Some(json_name)) =
                    (files.get(2).as_string(), files.get(3).as_string())
                else {
                    editor.warn("스프라이트 시트 파일 이름이 올바르지 않습니다.");
                    return;
                };
                browser::download_bytes(&Uint8Array::new(&files.get(0)), &png_name, "image/png");
                browser::download_bytes(
                    &Uint8Array::new(&files.get(1)),
                    &json_name,
                    "application/json",
                );
                editor.report(
                    "스프라이트 시트 PNG·JSON 다운로드 요청 완료 · 두 파일을 모두 확인해주세요.",
                );
            }
            Err(error) => editor.warn(format!(
                "스프라이트 시트 내보내기 실패 · 원본 유지: {}",
                browser::error_text(&error)
            )),
        }
    });
}
