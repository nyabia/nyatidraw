use super::*;

impl EditorView {
    pub(super) fn canvas_workspace(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = &self.projection;
        let mut workspace = div().size_full().flex().flex_col().min_h_0();
        if p.edit.has_selection || p.edit.transform.is_some() {
            let enabled = !p.edit.busy && p.edit.color_adjustment.is_none() && !p.animation.playing;
            let bar = div()
                .flex()
                .flex_wrap()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .px_2()
                .bg(rgb(0x424242));
            let bar = if let Some(transform) = &p.edit.transform {
                bar.child("변형 미리보기")
                    .child(self.command_button(
                        "적용",
                        EditorCommand::Edit(EditCommand::FreeTransform(TransformCommand::Commit {
                            generation: transform.generation,
                        })),
                        enabled && transform.can_commit && !self.panels.has_unapplied_preview(),
                        cx,
                    ))
                    .child(
                        self.command_button(
                            "취소",
                            EditorCommand::Edit(EditCommand::FreeTransform(
                                TransformCommand::Cancel,
                            )),
                            enabled && p.edit.can_cancel,
                            cx,
                        )
                        .label("취소"),
                    )
            } else {
                let can_copy = enabled
                    && p.edit.selected_pixels > 0
                    && !matches!(p.raster_target, Some(RasterEditTarget::Mask { .. }));
                bar.child("선택 영역 안에 그리는 중")
                    .child(
                        self.command_button(
                            "변형",
                            EditorCommand::Edit(EditCommand::FreeTransform(
                                TransformCommand::Begin,
                            )),
                            nyatidraw_editor_ui::can_begin_transform(p),
                            cx,
                        )
                        .label("변형"),
                    )
                    .child(self.command_button(
                        "복사",
                        EditorCommand::Edit(EditCommand::CopySelection),
                        can_copy,
                        cx,
                    ))
                    .child(self.command_button(
                        "해제",
                        EditorCommand::Edit(EditCommand::ClearSelection),
                        enabled,
                        cx,
                    ))
            };
            workspace = workspace.child(bar);
        }
        let focus = self.focus.clone();
        workspace
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&focus, cx)
                    })
                    .child(self.host.canvas(window, cx)),
            )
            .into_any_element()
    }
}
