use dioxus::prelude::*;

/// Export sizing is session UI state; it never becomes canvas metadata or Undo.
#[component]
pub fn ScaledExportButton(
    source: [u32; 2],
    disabled: bool,
    onexport: EventHandler<[u32; 2]>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut size = use_signal(|| source);
    let mut error = use_signal(|| None::<String>);
    rsx! {
        button { class: "command", disabled,
            onclick: move |_| { size.set(source); error.set(None); open.set(true); },
            "PNG 크기 지정…"
        }
        if open() {
            div { class: "page-backdrop", onclick: move |_| open.set(false),
            section { id: "scaled-export-dialog", class: "page-dialog transform-properties", role: "dialog", aria_modal: "true", aria_label: "PNG 크기 지정",
                onclick: move |event| event.stop_propagation(),
                onmounted: move |_| crate::contain_overlay_focus("scaled-export-dialog"),
                onkeydown: move |event| { event.stop_propagation(); if event.key() == Key::Escape { open.set(false); } },
                h2 { "PNG 크기 지정" }
                p { "원본 {source[0]} × {source[1]} px · 그림과 실행 취소 기록은 유지됩니다." }
                label { "비율 (%)"
                    input { r#type: "number", min: "1", max: "100", step: "0.1", autofocus: true,
                        value: format!("{:.2}", f64::from(size()[0]) * 100.0 / f64::from(source[0].max(1))),
                        onchange: move |event| {
                            match event.value().parse::<f64>() {
                                Ok(percent) if percent.is_finite() && percent > 0.0 && percent <= 100.0 => {
                                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                                    let next = source.map(|dimension| (f64::from(dimension) * percent / 100.0).round().max(1.0) as u32);
                                    size.set(next); error.set(None);
                                }
                                _ => error.set(Some("비율은 0보다 크고 100 이하로 입력하세요.".into())),
                            }
                        }
                    }
                }
                for axis in 0..2 {
                    label { if axis == 0 { "너비 (px)" } else { "높이 (px)" }
                        input { r#type: "number", min: "1", max: "{source[axis]}", value: "{size()[axis]}",
                            onchange: move |event| {
                                match event.value().parse::<u32>() {
                                    Ok(value) if value > 0 && value <= source[axis] => {
                                        let other = 1 - axis;
                                        let mut next = [0; 2];
                                        next[axis] = value;
                                        next[other] = u32::try_from((u64::from(source[other]) * u64::from(value) + u64::from(source[axis]) / 2) / u64::from(source[axis].max(1))).unwrap_or(source[other]).max(1);
                                        size.set(next); error.set(None);
                                    }
                                    _ => error.set(Some("크기는 1 px부터 원본 크기까지 입력하세요.".into())),
                                }
                            }
                        }
                    }
                }
                p { "가로세로 비율 유지 · 축소 전용 · 투명 가장자리 보존" }
                if let Some(message) = error() { p { role: "alert", "{message}" } }
                div { class: "page-dialog-actions",
                    button { class: "command", onclick: move |_| open.set(false), "취소" }
                    button { class: "command", disabled: disabled || error().is_some(), onclick: move |_| { onexport.call(size()); open.set(false); }, "PNG 내보내기" }
                }
            }
            }
        }
    }
}
