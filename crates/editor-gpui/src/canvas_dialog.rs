use super::*;
use gpui_kit::component::input::{Input, InputEvent};
use nyatidraw_editor_ui::canvas_size::{
    COMMON_PRESETS, CanvasDraft, CanvasPreset, MORE_PRESETS, scaled_export_axis,
    scaled_export_percent,
};

pub(super) struct PageDraft {
    model: CanvasDraft,
    dimensions: [Entity<InputState>; 2],
    source: [u32; 2],
    _inputs: Vec<Subscription>,
    more: bool,
    pub(super) export: bool,
    pending: Option<u128>,
    accepted: bool,
    percent: Entity<InputState>,
    export_axis: usize,
    error: Option<String>,
}

#[derive(Clone, Copy)]
enum SizeAction {
    Preset(CanvasPreset),
    Multiply(u32),
    Swap,
    Undo,
}

impl PageDraft {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    fn scaled_size(&self, cx: &App) -> Result<[u32; 2], String> {
        let axis = self.export_axis;
        if axis == 2 {
            return self
                .percent
                .read(cx)
                .value()
                .parse::<f64>()
                .ok()
                .and_then(|value| scaled_export_percent(self.source, value))
                .ok_or_else(|| "비율은 0보다 크고 100 이하로 입력하세요.".into());
        }
        self.dimensions[axis]
            .read(cx)
            .value()
            .parse::<u32>()
            .ok()
            .and_then(|value| scaled_export_axis(self.source, axis, value))
            .ok_or_else(|| format!("크기는 1~{} px로 입력하세요.", self.source[axis]))
    }
    fn read_fields(&mut self, cx: &App) {
        for (i, field) in self.dimensions.iter().enumerate() {
            let value = field.read(cx).value().to_string();
            if value != *[&self.model.current.width, &self.model.current.height][i] {
                self.model.type_dimension(i == 0, value);
            }
        }
    }
}

impl EditorView {
    pub(super) fn poll_page_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.page.as_ref().and_then(|page| page.pending) else {
            return;
        };
        let accepted = self.page.as_ref().is_some_and(|page| page.accepted);
        if accepted {
            let snapshot = self.host.snapshot();
            if let WorkspaceProjection::Error { summary } = snapshot.workspace {
                if let Some(page) = self.page.as_mut() {
                    page.pending = None;
                    page.accepted = false;
                    page.error = Some(summary);
                }
                return;
            }
            if snapshot.edit.busy {
                return;
            }
            if let Some(error) = snapshot.edit.error {
                if let Some(page) = self.page.as_mut() {
                    page.pending = None;
                    page.accepted = false;
                    page.error = Some(error);
                }
            } else {
                self.page = None;
                window.focus(&self.focus, cx);
            }
            return;
        }
        match self.host.take_command_result(id) {
            Some(Ok(())) => {
                if let Some(page) = self.page.as_mut() {
                    page.accepted = true;
                }
                cx.notify();
            }
            Some(Err(error)) => {
                if let Some(page) = self.page.as_mut() {
                    page.pending = None;
                    page.error = Some(error);
                }
            }
            None => {}
        }
    }
    pub(super) fn open_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.host.snapshot();
        let model = CanvasDraft::new(snapshot.canvas);
        let dimensions = [&model.current.width, &model.current.height]
            .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value.clone())));
        let percent = cx.new(|cx| InputState::new(window, cx).default_value("100"));
        let mut subscriptions: Vec<Subscription> = dimensions
            .iter()
            .enumerate()
            .map(|(axis, field)| {
                cx.subscribe_in(
                    field,
                    window,
                    move |view, _, event: &InputEvent, window, cx| match event {
                        InputEvent::Change => {
                            if let Some(page) = view.page.as_mut().filter(|page| page.export) {
                                page.export_axis = axis;
                                page.error = None;
                                if let Ok(size) = page.scaled_size(cx) {
                                    let other = 1 - axis;
                                    page.dimensions[other].update(cx, |input, cx| {
                                        input.set_value(size[other].to_string(), window, cx)
                                    });
                                    let percent = f64::from(size[axis]) * 100.0
                                        / f64::from(page.source[axis]);
                                    page.percent.update(cx, |input, cx| {
                                        input.set_value(format!("{percent:.2}"), window, cx)
                                    });
                                }
                            }
                            cx.notify();
                        }
                        InputEvent::PressEnter { .. } => view.commit_page(false, window, cx),
                        _ => {}
                    },
                )
            })
            .collect();
        subscriptions.push(cx.subscribe_in(
            &percent,
            window,
            |view, field, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change)
                    && let Some(page) = view.page.as_mut().filter(|page| page.export)
                {
                    if let Ok(value) = field.read(cx).value().parse::<f64>() {
                        if value.is_finite() && value > 0.0 && value <= 100.0 {
                            let size = scaled_export_percent(page.source, value)
                                .expect("validated percent");
                            for (field, value) in page.dimensions.iter().zip(size) {
                                field.update(cx, |input, cx| {
                                    input.set_value(value.to_string(), window, cx)
                                });
                            }
                            page.export_axis = 2;
                            page.error = None;
                        } else {
                            page.error = Some("비율은 0보다 크고 100 이하로 입력하세요.".into());
                        }
                    } else {
                        page.error = Some("비율을 입력하세요.".into());
                    }
                    cx.notify();
                }
            },
        ));
        let focus = dimensions[0].read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.page = Some(PageDraft {
            model,
            dimensions,
            source: [snapshot.canvas.width_px, snapshot.canvas.height_px],
            _inputs: subscriptions,
            export: false,
            more: false,
            pending: None,
            accepted: false,
            percent,
            export_axis: 0,
            error: None,
        });
        cx.notify();
    }

    fn size_button(
        &self,
        label: &'static str,
        action: SizeAction,
        cx: &Context<Self>,
    ) -> AnyElement {
        control(label, true)
            .on_click(cx.listener(move |view, _, window, cx| {
                let Some(page) = view.page.as_mut() else {
                    return;
                };
                page.read_fields(cx);
                match action {
                    SizeAction::Preset(preset) => page.model.choose_preset(preset),
                    SizeAction::Swap => page.model.swap_axes(),
                    SizeAction::Multiply(factor) => {
                        if page.model.current.multiplied(factor).is_none() {
                            page.error = Some(
                                "현재 출력 한도를 초과하거나 크기가 올바르지 않습니다.".into(),
                            );
                            cx.notify();
                            return;
                        }
                        page.model.multiply(factor);
                    }
                    SizeAction::Undo => page.model.undo(),
                }
                page.error = None;
                for (field, value) in page
                    .dimensions
                    .iter()
                    .zip([&page.model.current.width, &page.model.current.height])
                {
                    field.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                }
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn commit_page(&mut self, crop: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.poll_dock_resize();
        let Some(page) = self.page.as_mut() else {
            return;
        };
        if page.pending.is_some() {
            return;
        }
        page.read_fields(cx);
        if page.export {
            let result = page
                .scaled_size(cx)
                .and_then(|size| self.host.file_action(FileAction::ExportSizedPng(size)));
            match result {
                Ok(()) => {
                    self.page = None;
                    window.focus(&self.focus, cx);
                }
                Err(error) => {
                    page.error = Some(error);
                }
            }
            cx.notify();
            return;
        }
        if self.pending_dock_resize.is_some() {
            page.error = Some("작업공간 경계를 저장 중입니다. 잠시 후 다시 시도하세요.".into());
            cx.notify();
            return;
        }
        let command = if crop {
            EditCommand::CropPageToSelection
        } else {
            match page.model.current.size() {
                Ok(size) => EditCommand::ResizePage { size },
                Err(error) => {
                    page.error = Some(error.into());
                    cx.notify();
                    return;
                }
            }
        };
        let result = self
            .host
            .begin_tracked_command(self.host.snapshot().revision, EditorCommand::Edit(command));
        match result {
            Ok(Some(id)) => page.pending = Some(id),
            Ok(None) => {
                self.page = None;
                window.focus(&self.focus, cx);
            }
            Err(error) => page.error = Some(error),
        }
        cx.notify();
    }

    pub(super) fn page_dialog(&self, _window: &mut Window, cx: &Context<Self>) -> AnyElement {
        let page = self.page.as_ref().expect("open canvas dialog");
        if page.export {
            return self.export_dialog(page, cx);
        }
        if page.pending.is_some() {
            return div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x000000a8))
                .child(
                    div()
                        .p_4()
                        .rounded_md()
                        .bg(rgb(0x393939))
                        .child("캔버스 변경 적용 중…"),
                )
                .into_any_element();
        }
        let mut presets = div().flex().flex_wrap().gap_1();
        for preset in COMMON_PRESETS
            .into_iter()
            .chain(MORE_PRESETS.into_iter().filter(|_| page.more))
        {
            presets = presets.child(self.size_button(preset.name, SizeAction::Preset(preset), cx));
        }
        let more = if page.more { "접기" } else { "더 보기" };
        presets = presets.child(control(more, true).on_click(cx.listener(|view, _, _, cx| {
            if let Some(page) = view.page.as_mut() {
                page.more = !page.more;
                cx.notify();
            }
        })));
        let mut scale = div().flex().gap_1();
        for (label, action) in [
            ("되돌리기", SizeAction::Undo),
            ("×2", SizeAction::Multiply(2)),
            ("×3", SizeAction::Multiply(3)),
            ("×5", SizeAction::Multiply(5)),
            ("가로세로 교환", SizeAction::Swap),
        ] {
            scale = scale.child(self.size_button(label, action, cx));
        }
        let mut fields = div().flex().gap_2().items_center();
        for (field, label) in page.dimensions.iter().zip(["가로", "세로"]) {
            fields = fields
                .child(label)
                .child(div().w(px(104.)).child(Input::new(field)))
                .child("px");
        }
        let selection =
            page.pending.is_none() && !page.export && self.projection.edit.has_selection;
        let dialog = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .w(px(392.))
            .bg(rgb(0x393939))
            .rounded_md()
            .child(if page.export {
                "PNG 크기 지정"
            } else {
                "캔버스 크기"
            })
            .child(presets)
            .child(scale)
            .child(fields)
            .children(
                page.pending
                    .map(|_| "적용 중…".to_string())
                    .or_else(|| page.error.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        control("선택 영역으로 자르기", selection).when(selection, |b| {
                            b.on_click(cx.listener(|v, _, w, cx| v.commit_page(true, w, cx)))
                        }),
                    )
                    .child(control("닫기", page.pending.is_none()).when(
                        page.pending.is_none(),
                        |b| {
                            b.on_click(cx.listener(|v, _, w, cx| {
                                v.page = None;
                                w.focus(&v.focus, cx);
                                cx.notify();
                            }))
                        },
                    ))
                    .child(
                        control("적용", page.pending.is_none()).when(page.pending.is_none(), |b| {
                            b.on_click(cx.listener(|v, _, w, cx| v.commit_page(false, w, cx)))
                        }),
                    ),
            );
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .child(dialog)
            .into_any_element()
    }
    fn export_dialog(&self, page: &PageDraft, cx: &Context<Self>) -> AnyElement {
        let mut scale = div().flex().gap_2();
        for (label, percent) in [("25%", 25u32), ("50%", 50), ("75%", 75), ("100%", 100)] {
            scale = scale.child(
                control(label, true).on_click(cx.listener(move |v, _, w, cx| {
                    if let Some(page) = v.page.as_mut() {
                        page.percent
                            .update(cx, |field, cx| field.set_value(percent.to_string(), w, cx));
                        let size = scaled_export_percent(page.source, f64::from(percent))
                            .expect("preset percent");
                        for (field, value) in page.dimensions.iter().zip(size) {
                            field.update(cx, |input, cx| input.set_value(value.to_string(), w, cx));
                        }
                        page.export_axis = 2;
                        page.error = None;
                        cx.notify();
                    }
                })),
            );
        }
        let size = page.scaled_size(cx);
        let output = size
            .as_ref()
            .map(|size| format!("결과 {} × {} px", size[0], size[1]))
            .unwrap_or_default();
        let enabled = size.is_ok() && page.error.is_none();
        let dialog = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_md()
            .bg(rgb(0x393939))
            .child("PNG 크기 지정")
            .child(format!("원본 {} × {} px", page.source[0], page.source[1]))
            .child(scale)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child("비율 (%)")
                    .child(div().w(px(105.)).child(Input::new(&page.percent))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child("가로")
                    .child(div().w(px(105.)).child(Input::new(&page.dimensions[0])))
                    .child("세로")
                    .child(div().w(px(105.)).child(Input::new(&page.dimensions[1]))),
            )
            .child(output)
            .children(page.error.clone().or_else(|| size.err()))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(control("취소", true).on_click(cx.listener(|v, _, w, cx| {
                        v.page = None;
                        w.focus(&v.focus, cx);
                        cx.notify();
                    })))
                    .child(control("PNG 내보내기", enabled).when(enabled, |b| {
                        b.on_click(cx.listener(|v, _, w, cx| v.commit_page(false, w, cx)))
                    })),
            );
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .justify_center()
            .items_center()
            .bg(rgba(0x000000a8))
            .child(dialog)
            .into_any_element()
    }
}
