use super::*;

pub(super) fn animation(ui: &mut PanelUi, p: &UiProjection) -> Div {
    let animation = &p.animation;
    let busy = p.edit.busy || p.edit.transform.is_some() || p.edit.color_adjustment.is_some();
    let enabled = !busy && !animation.playing;
    let Some((index, active)) = animation
        .frames
        .iter()
        .enumerate()
        .find(|(_, f)| Some(f.id) == animation.active_frame)
    else {
        return row().child(ui.button(
            "애니메이션 시작",
            EditorCommand::Animation(AnimationCommand::Enable),
            false,
            !busy,
        ));
    };
    let id = active.id;
    const PAGE_SIZE: usize = 12;
    let pages = animation.frames.len().div_ceil(PAGE_SIZE);
    let follow_active = ui.view.panels.last_frame != Some(id);
    if follow_active {
        ui.view.panels.frames_page = index / PAGE_SIZE;
        ui.view.panels.last_frame = Some(id);
    }
    let page = ui.view.panels.frames_page.min(pages - 1);
    let first = page * PAGE_SIZE;
    let visible = &animation.frames[first..(first + PAGE_SIZE).min(animation.frames.len())];
    let snapshot = ui.view.host.frame_thumbnails();
    let ids: Vec<_> = visible.iter().map(|frame| frame.id).collect();
    ui.keep_thumbnails(|key| match key {
        Thumbnail::Frame(id) => ids.contains(&id),
        _ => true,
    });
    let request = (snapshot.project_epoch, snapshot.generation, ids);
    if !animation.playing && ui.view.panels.requested.as_ref() != Some(&request) {
        ui.view.host.request_frame_thumbnails(&request.2);
        ui.view.panels.requested = Some(request);
    }
    let playback = if animation.playing {
        AnimationCommand::Stop
    } else {
        AnimationCommand::Play
    };
    let controls = row().child(ui.heading("프레임")).children(
        [
            (
                if animation.playing {
                    "정지"
                } else {
                    "재생"
                },
                playback,
                animation.playing,
                !busy,
            ),
            (
                "반복",
                AnimationCommand::SetLoop(!animation.looping),
                animation.looping,
                !busy,
            ),
            (
                "어니언 스킨",
                AnimationCommand::SetOnionSkin(!animation.onion_skin),
                animation.onion_skin,
                enabled,
            ),
            (
                "빈 프레임",
                AnimationCommand::InsertBlank { after: id },
                false,
                enabled,
            ),
            ("복제", AnimationCommand::Duplicate(id), false, enabled),
            (
                "삭제",
                AnimationCommand::Delete(id),
                false,
                enabled && animation.frames.len() > 1,
            ),
            (
                "앞으로",
                AnimationCommand::Move {
                    frame: id,
                    index: u16::try_from(index.saturating_sub(1)).unwrap_or(0),
                },
                false,
                enabled && index > 0,
            ),
            (
                "뒤로",
                AnimationCommand::Move {
                    frame: id,
                    index: u16::try_from(index + 1).unwrap_or(u16::MAX),
                },
                false,
                enabled && index + 1 < animation.frames.len(),
            ),
        ]
        .map(|(label, cmd, active, enabled)| {
            ui.button(label, EditorCommand::Animation(cmd), active, enabled)
        }),
    );
    let focus = ui
        .view
        .panels
        .frames_focus
        .get_or_insert_with(|| ui.cx.focus_handle().tab_stop(true))
        .clone();
    let scroll = &ui.view.panels.frames_scroll;
    if follow_active {
        scroll.scroll_to_item(index - first);
    }
    let mut frames = div()
        .flex()
        .flex_row()
        .gap_1()
        .items_center()
        .id("animation-frames")
        .overflow_x_scroll()
        .track_scroll(scroll)
        .track_focus(&focus)
        .on_key_down(ui.cx.listener(move |v, event: &KeyDownEvent, _, cx| {
            if !enabled || event.keystroke.modifiers.modified() {
                return;
            }
            let snapshot = v.host.snapshot();
            let animation = &snapshot.animation;
            let Some(current) = animation
                .frames
                .iter()
                .position(|f| Some(f.id) == animation.active_frame)
            else {
                return;
            };
            let next = match event.keystroke.key.as_str() {
                "left" => current.saturating_sub(1),
                "right" => (current + 1).min(animation.frames.len() - 1),
                "home" => 0,
                "end" => animation.frames.len() - 1,
                _ => return,
            };
            if next != current {
                v.send_at(
                    snapshot.revision,
                    EditorCommand::Animation(AnimationCommand::Select(animation.frames[next].id)),
                    cx,
                );
            }
            cx.stop_propagation();
        }));
    for (n, frame) in visible.iter().enumerate() {
        let frame_id = frame.id;
        let focus = focus.clone();
        let select = EditorCommand::Animation(AnimationCommand::Select(frame_id));
        let mut frame_button = Button::new(("timeline-frame", n))
            .label(format!("{} · {} ms", first + n + 1, frame.duration_ms))
            .small()
            .compact()
            .ghost()
            .selected(frame_id == id)
            .disabled(!enabled || !ui.view.host.supports_command(&select))
            .flex_shrink_0()
            .on_click(ui.cx.listener(move |v, _, window, cx| {
                window.focus(&focus, cx);
                v.send(select.clone(), cx);
            }));
        if let Some(preview) = snapshot.displayed_frame(frame.id)
            && let Some(image) = ui.thumbnail(Thumbnail::Frame(frame.id), &preview.data_uri)
        {
            frame_button = frame_button.child(image.w(px(34.)).h(px(24.)));
        }
        frames = frames.child(frame_button);
    }
    let duration_editable = enabled
        && ui.view.host.supports_command(&EditorCommand::Animation(
            AnimationCommand::SetDuration {
                frame: id,
                duration_ms: active.duration_ms,
            },
        ));
    let duration = ui.field(
        "재생 시간 (ms) · Enter",
        id.0.to_string(),
        active.duration_ms.to_string(),
        duration_editable,
        move |view, t, cx| {
            let duration_ms = t
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=60_000).contains(n))
                .ok_or("프레임 시간은 1~60000 ms입니다.")?;
            view.send(
                EditorCommand::Animation(AnimationCommand::SetDuration {
                    frame: id,
                    duration_ms,
                }),
                cx,
            );
            Ok(())
        },
    );
    let page_controls = (pages > 1).then(|| {
        row()
            .child(
                Button::new("previous-frame-page")
                    .small()
                    .compact()
                    .label("이전 목록")
                    .disabled(page == 0)
                    .on_click(ui.cx.listener(move |v, _, _, cx| {
                        v.panels.frames_page = page.saturating_sub(1);
                        cx.notify();
                    })),
            )
            .child(format!("{} / {}", page + 1, pages))
            .child(
                Button::new("next-frame-page")
                    .small()
                    .compact()
                    .label("다음 목록")
                    .disabled(page + 1 >= pages)
                    .on_click(ui.cx.listener(move |v, _, _, cx| {
                        v.panels.frames_page = page + 1;
                        cx.notify();
                    })),
            )
    });
    column()
        .gap_1()
        .child(
            row()
                .child(controls)
                .child(div().w(px(136.)).child(duration)),
        )
        .children(page_controls)
        .child(frames)
}
