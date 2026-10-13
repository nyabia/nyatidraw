use dioxus::prelude::*;
use nyatidraw_editor_ui::reference_panel::ReferenceState;

#[component]
pub(crate) fn ReferenceImageButton(reference: ReferenceState) -> Element {
    #[cfg(windows)]
    {
        let window = dioxus_desktop::window().window.clone();
        return rsx! {
            button { class: "command compact", disabled: reference.loading(), title: "참조용 PNG · 현재 세션에서만 표시",
                onclick: move |_| {
                    let Some(request) = reference.begin_load() else { return };
                    let window = window.clone();
                    dioxus::core::spawn_forever(async move {
                        let selected = rfd::AsyncFileDialog::new().set_parent(window.as_ref())
                            .set_title("참조 이미지 열기").add_filter("PNG 이미지", &["png"])
                            .pick_file().await;
                        let result = match selected {
                            Some(file) => decode_file(file.path().to_path_buf()).await.map(Some),
                            None => Ok(None),
                        };
                        reference.finish_load(request, result);
                    });
                }, "PNG 열기"
            }
        };
    }
    #[cfg(not(windows))]
    {
        let _ = reference;
        rsx! { button { disabled: true, "PNG 열기" } }
    }
}

#[cfg(windows)]
async fn decode_file(
    path: std::path::PathBuf,
) -> Result<std::sync::Arc<nyatidraw_editor_ui::reference_panel::ReferenceFrame>, String> {
    use nyatidraw_editor_ui::reference_panel::{
        MAX_REFERENCE_BYTES, ReferenceFrame, decode_reference_png,
    };
    use std::{
        io::Read,
        sync::{Arc, Mutex},
        task::{Poll, Waker},
    };

    #[derive(Default)]
    struct DecodeJob {
        result: Option<Result<Arc<ReferenceFrame>, String>>,
        wake: Option<Waker>,
    }
    let job = Arc::new(Mutex::new(DecodeJob::default()));
    let worker = job.clone();
    std::thread::Builder::new()
        .name("reference-png".into())
        .spawn(move || {
            let result = (|| {
                let file = std::fs::File::open(&path).map_err(|error| error.to_string())?;
                let mut bytes = Vec::new();
                file.take(MAX_REFERENCE_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                decode_reference_png(&bytes, name)
            })();
            let wake = {
                let mut job = worker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                job.result = Some(result);
                job.wake.take()
            };
            if let Some(wake) = wake {
                wake.wake();
            }
        })
        .map_err(|error| error.to_string())?;
    std::future::poll_fn(move |context| {
        let mut job = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(result) = job.result.take() {
            Poll::Ready(result)
        } else {
            job.wake = Some(context.waker().clone());
            Poll::Pending
        }
    })
    .await
}
