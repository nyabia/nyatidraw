use std::{
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use dioxus::prelude::*;
use nyatidraw_api::{EditCommand, EditorCommand, LayerId, RasterImport, TransformCommand};

use crate::live_ink::LiveInkBridge;

const MAX_PNG_BYTES: u64 = 72 * 1024 * 1024;

#[component]
pub(crate) fn ImportImageButton() -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let busy = use_hook(|| Arc::new(AtomicBool::new(false)));
    let update = dioxus_core::schedule_update();
    let window = dioxus_desktop::window().window.clone();
    rsx! {
        button { class: "command", disabled: busy.load(Ordering::Acquire),
            title: "PNG를 새 레이어로 가져와 배치한 뒤 확정하거나 취소",
            onclick: move |_| {
                let busy = busy.clone();
                let live_ink = live_ink.clone();
                let window = window.clone();
                let update = update.clone();
                async move {
                    if busy.swap(true, Ordering::AcqRel) { return; }
                    update();
                    let epoch = live_ink.project_epoch();
                    let file = rfd::AsyncFileDialog::new().set_parent(window.as_ref())
                        .set_title("PNG를 레이어로 가져오기").add_filter("PNG 이미지", &["png"])
                        .pick_file().await;
                    let Some(file) = file else {
                        busy.store(false, Ordering::Release);
                        update();
                        return;
                    };
                    std::thread::spawn(move || {
                        let result = decode_import(file.path(), epoch).and_then(|import| {
                            if live_ink.project_epoch() != epoch {
                                return Err("그림이 바뀌어 이미지 가져오기를 취소했습니다.".into());
                            }
                            live_ink.push_ui_editor_command(EditorCommand::Edit(
                                EditCommand::FreeTransform(TransformCommand::ImportRaster(import))
                            )).map(|_| ()).map_err(|_| "현재 작업을 마친 뒤 다시 가져와주세요.".into())
                        });
                        if let Err(error) = result { live_ink.publish_activation_notice(error); }
                        busy.store(false, Ordering::Release);
                        update();
                    });
                }
            }, "PNG를 레이어로 가져오기"
        }
    }
}

fn decode_import(path: &Path, project_epoch: u64) -> Result<RasterImport, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_PNG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_PNG_BYTES {
        return Err("PNG 파일은 최대 72 MiB까지 가져올 수 있습니다.".into());
    }
    let imported = nyatidraw_png_io::decode_png_bytes(
        &bytes,
        LayerId(1),
        nyatidraw_paint_cpu::EditLimits::default().max_pixels,
    )
    .map_err(|e| e.to_string())?;
    let surface = imported
        .tiles
        .crop_base_layer_rgba8_to_canvas(LayerId(1), imported.canvas)
        .map_err(|e| format!("{e:?}"))?;
    Ok(RasterImport {
        project_epoch,
        name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        size: [surface.width, surface.height],
        pixels: surface.pixels.into(),
    })
}
