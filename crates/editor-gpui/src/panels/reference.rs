use std::{cell::Cell, rc::Rc};

use super::*;
use nyatidraw_tiles::FlattenedRgba8;

#[cfg(any(windows, target_arch = "wasm32"))]
const MAX_BYTES: usize = 72 * 1024 * 1024;
#[cfg(any(windows, target_arch = "wasm32"))]
const MAX_EDGE: u32 = 4096;

struct Frame {
    name: String,
    image: ImageSource,
    surface: FlattenedRgba8,
}

#[derive(Clone, Copy)]
enum Contact {
    Pan {
        start: Point<Pixels>,
        pan: Point<Pixels>,
    },
    Pick {
        revision: Revision,
        color: Option<[u8; 4]>,
    },
}

pub(super) struct Reference {
    pub(super) show: bool,
    frame: Option<Frame>,
    loading: bool,
    generation: u64,
    picking: bool,
    zoom: f32,
    pan: Point<Pixels>,
    contact: Option<Contact>,
    error: Option<String>,
}

impl Default for Reference {
    fn default() -> Self {
        Self {
            show: false,
            frame: None,
            loading: false,
            generation: 0,
            picking: false,
            zoom: 1.,
            pan: Point::default(),
            contact: None,
            error: None,
        }
    }
}

impl Reference {
    pub(super) fn cancel_contact(&mut self) -> bool {
        self.contact.take().is_some()
    }

    fn fit(&mut self) {
        self.zoom = 1.;
        self.pan = Point::default();
        self.contact = None;
    }

    fn sample(&self, point: Point<Pixels>, bounds: Bounds<Pixels>) -> Option<[u8; 4]> {
        let s = &self.frame.as_ref()?.surface;
        if !bounds.contains(&point) {
            return None;
        }
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let scale = (w / s.width as f32).min(h / s.height as f32) * self.zoom;
        if scale <= 0. {
            return None;
        }
        let local = point - bounds.origin - self.pan;
        let x = ((f32::from(local.x) - (w - s.width as f32 * scale) / 2.) / scale).floor();
        let y = ((f32::from(local.y) - (h - s.height as f32 * scale) / 2.) / scale).floor();
        if !(0. ..s.width as f32).contains(&x) || !(0. ..s.height as f32).contains(&y) {
            return None;
        }
        let i = (y as usize * s.width as usize + x as usize) * 4;
        let rgb = nyatidraw_tiles::color::linear_premultiplied_to_srgb8(
            s.pixels.get(i..i + 4)?.try_into().ok()?,
        )?;
        Some([rgb[0], rgb[1], rgb[2], 255])
    }
}

pub(super) fn tabs(ui: &PanelUi) -> Div {
    row().children(
        [("내비게이터", false), ("참조 이미지", true)].map(|(label, show)| {
            Button::new(label)
                .small()
                .compact()
                .ghost()
                .label(label)
                .selected(ui.view.panels.reference.show == show)
                .on_click(ui.cx.listener(move |v, _, window, cx| {
                    v.panels.reference.show = show;
                    v.panels.reference.cancel_contact();
                    window.focus(&v.focus, cx);
                    cx.notify();
                }))
        }),
    )
}

pub(super) fn panel(ui: &mut PanelUi, p: &UiProjection) -> Div {
    let state = &ui.view.panels.reference;
    let mut panel = column().flex_1().min_h_0();
    #[cfg(any(windows, target_arch = "wasm32"))]
    {
        panel = panel.child(
            Button::new("open-reference")
                .small()
                .compact()
                .label(if state.loading {
                    "PNG 여는 중…"
                } else {
                    "참조 PNG 열기"
                })
                .disabled(state.loading)
                .on_click(ui.cx.listener(open)),
        );
    }
    if let Some(error) = &state.error {
        panel = panel.child(div().text_color(ui.cx.theme().danger).child(error.clone()));
    }
    let Some(frame) = &state.frame else {
        return panel;
    };
    let image = img(frame.image.clone())
        .object_fit(ObjectFit::Contain)
        .absolute()
        .w(relative(state.zoom))
        .h(relative(state.zoom))
        .left(relative((1. - state.zoom) / 2.))
        .top(relative((1. - state.zoom) / 2.))
        .ml(state.pan.x)
        .mt(state.pan.y);
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
    let capture = bounds.clone();
    let down = bounds.clone();
    let moving = bounds.clone();
    let revision = p.revision;
    let area = div()
        .id("reference-view")
        .relative()
        .w_full()
        .flex_1()
        .min_h(px(48.))
        .overflow_hidden()
        .bg(rgb(0x323232))
        .child(image)
        .child(
            canvas(move |b, _, _| capture.set(b), |_, _, _, _| {})
                .absolute()
                .inset_0()
                .size_full(),
        )
        .on_mouse_down(
            MouseButton::Left,
            ui.cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                w.focus(&v.focus, cx);
                let r = &mut v.panels.reference;
                r.contact = Some(if r.picking {
                    Contact::Pick {
                        revision,
                        color: r.sample(e.position, down.get()),
                    }
                } else {
                    Contact::Pan {
                        start: e.position,
                        pan: r.pan,
                    }
                });
                cx.notify();
            }),
        )
        .on_mouse_move(ui.cx.listener(move |v, e: &MouseMoveEvent, _, cx| {
            let r = &mut v.panels.reference;
            match r.contact {
                Some(Contact::Pan { start, pan }) => r.pan = pan + e.position - start,
                Some(Contact::Pick { revision, .. }) => {
                    r.contact = Some(Contact::Pick {
                        revision,
                        color: r.sample(e.position, moving.get()),
                    })
                }
                None => return,
            }
            cx.notify();
        }))
        .on_mouse_up(
            MouseButton::Left,
            ui.cx.listener(move |v, e: &MouseUpEvent, _, cx| {
                let r = &mut v.panels.reference;
                if let Some(Contact::Pick { revision, .. }) = r.contact.take()
                    && let Some(color) = r.sample(e.position, bounds.get())
                {
                    v.send_at(
                        revision,
                        EditorCommand::Tool(ToolCommand::SetColor(color)),
                        cx,
                    );
                }
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            ui.cx.listener(|v, _, _, cx| {
                if v.panels.reference.cancel_contact() {
                    cx.notify();
                }
            }),
        );
    let can_pick = ui
        .view
        .host
        .supports_command(&EditorCommand::Tool(ToolCommand::SetColor(p.brush_color)));
    let mut controls = row().child(
        Button::new("reference-picker")
            .small()
            .compact()
            .label("색 채취")
            .selected(state.picking)
            .disabled(!can_pick)
            .on_click(ui.cx.listener(|v, _, w, cx| {
                let r = &mut v.panels.reference;
                r.picking = !r.picking;
                r.contact = None;
                w.focus(&v.focus, cx);
                cx.notify();
            })),
    );
    for (label, factor) in [("맞춤", 0.), ("축소", 0.8), ("확대", 1.25)] {
        controls = controls.child(Button::new(label).small().compact().label(label).on_click(
            ui.cx.listener(move |v, _, _, cx| {
                let r = &mut v.panels.reference;
                if factor == 0. {
                    r.fit();
                } else {
                    r.zoom = (r.zoom * factor).clamp(1., 16.);
                    r.contact = None;
                }
                cx.notify();
            }),
        ));
    }
    controls = controls.child(format!("{:.0}%", state.zoom * 100.)).child(
        Button::new("remove-reference")
            .small()
            .compact()
            .label("닫기")
            .on_click(ui.cx.listener(|v, _, _, cx| {
                let r = &mut v.panels.reference;
                r.generation = r.generation.wrapping_add(1);
                r.loading = false;
                r.error = None;
                r.fit();
                if let Some(frame) = r.frame.take() {
                    frame.image.remove_asset(cx);
                }
                cx.notify();
            })),
    );
    let area = if let Some(Contact::Pick { color: Some(c), .. }) = state.contact {
        area.child(
            row()
                .absolute()
                .top_1()
                .right_1()
                .p_1()
                .bg(rgb(0x333333))
                .child(
                    div()
                        .size(px(22.))
                        .border_1()
                        .border_color(white())
                        .bg(rgba(u32::from_be_bytes(c))),
                )
                .child(format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])),
        )
    } else {
        area
    };
    panel
        .child(ui.heading(frame.name.clone()))
        .child(area)
        .child(controls)
}

#[cfg(any(windows, target_arch = "wasm32"))]
fn open(v: &mut EditorView, _: &ClickEvent, window: &mut Window, cx: &mut Context<EditorView>) {
    if v.panels.reference.loading {
        return;
    }
    let r = &mut v.panels.reference;
    r.generation = r.generation.wrapping_add(1);
    r.loading = true;
    r.error = None;
    r.cancel_contact();
    let token = r.generation;
    let picker = rfd::AsyncFileDialog::new()
        .add_filter("PNG", &["png"])
        .set_title("참조 이미지 열기");
    #[cfg(windows)]
    let picker = picker.set_parent(window);
    let pending = picker.pick_file();
    #[cfg(target_arch = "wasm32")]
    let pending = {
        use std::{
            future::Future,
            task::{Context as TaskContext, Poll, Waker},
        };
        let mut pending = Box::pin(pending);
        // Open the browser's chooser while the button still has transient user activation.
        let first = pending
            .as_mut()
            .poll(&mut TaskContext::from_waker(Waker::noop()));
        async move {
            match first {
                Poll::Ready(file) => file,
                Poll::Pending => pending.await,
            }
        }
    };
    cx.spawn(async move |view, cx| {
        let result = match pending.await {
            None => Ok(None),
            Some(file) => {
                #[cfg(windows)]
                {
                    let path = file.path().to_owned();
                    cx.background_executor()
                        .spawn(async move {
                            use std::io::Read;
                            let mut bytes = Vec::new();
                            let input = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                            if input.metadata().map_err(|e| e.to_string())?.len() > MAX_BYTES as u64
                            {
                                return Err("72 MiB 이하의 PNG를 선택하세요.".to_owned());
                            }
                            input
                                .take(MAX_BYTES as u64 + 1)
                                .read_to_end(&mut bytes)
                                .map_err(|e| e.to_string())?;
                            decode(
                                &bytes,
                                path.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                            )
                            .map(Some)
                        })
                        .await
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let picked = file.inner();
                    if picked.size() > MAX_BYTES as f64 {
                        Err("72 MiB 이하의 PNG를 선택하세요.".to_owned())
                    } else {
                        match wasm_bindgen_futures::JsFuture::from(picked.array_buffer()).await {
                            Ok(buffer) => {
                                decode(&js_sys::Uint8Array::new(&buffer).to_vec(), file.file_name())
                                    .map(Some)
                            }
                            Err(_) => Err("참조 PNG를 읽을 수 없습니다.".to_owned()),
                        }
                    }
                }
            }
        };
        let _ = view.update(cx, |v, cx| {
            let r = &mut v.panels.reference;
            if r.generation != token {
                return;
            }
            r.loading = false;
            match result {
                Ok(Some((name, surface, png))) => {
                    let image =
                        ImageSource::Image(Arc::new(Image::from_bytes(ImageFormat::Png, png)));
                    if let Some(old) = r.frame.replace(Frame {
                        name,
                        surface,
                        image,
                    }) {
                        old.image.remove_asset(cx);
                    }
                    r.fit();
                }
                Ok(None) => {}
                Err(message) => r.error = Some(message),
            }
            cx.notify();
        });
    })
    .detach();
    window.focus(&v.focus, cx);
    cx.notify();
}

#[cfg(any(windows, target_arch = "wasm32"))]
fn decode(bytes: &[u8], name: String) -> Result<(String, FlattenedRgba8, Vec<u8>), String> {
    if bytes.len() > MAX_BYTES || bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        return Err("72 MiB 이하의 PNG를 선택하세요.".to_owned());
    }
    let dimensions = bytes.get(16..24).ok_or("잘린 PNG 헤더입니다.")?;
    if dimensions
        .chunks_exact(4)
        .any(|d| u32::from_be_bytes([d[0], d[1], d[2], d[3]]) > MAX_EDGE)
    {
        return Err("참조 이미지는 가로·세로 4096 px까지 열 수 있습니다.".into());
    }
    let imported =
        nyatidraw_png_io::decode_png_bytes(bytes, LayerId(1), u64::from(MAX_EDGE).pow(2))
            .map_err(|e| e.to_string())?;
    let surface = imported
        .tiles
        .crop_base_layer_rgba8_to_canvas(LayerId(1), imported.canvas)
        .map_err(|e| format!("{e:?}"))?;
    let png = nyatidraw_png_io::encode_png_bytes(&surface).map_err(|e| e.to_string())?;
    Ok((name, surface, png))
}
