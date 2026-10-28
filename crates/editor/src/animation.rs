use nyatidraw_api::{
    AnimationCommand, AnimationFrameProjection, AnimationProjection, CanvasSpec, CelId, FrameId,
    GroupId, LayerBlendMode, LayerId, LayerTreeNodeId,
};
use nyatidraw_document::{
    AnimationCel, AnimationFrame, AnimationTimeline, GroupNode, LayerTree, LayerTreeNode,
};
use nyatidraw_tiles::TileSnapshot;

use crate::layer_edit::{
    LayerPixelChange, empty_raster, next_layer_node_id, raster_layer_ids, valid_active_layer,
};

pub struct PreparedAnimationEdit {
    pub tree: LayerTree,
    pub tiles: TileSnapshot,
    pub timeline: Option<AnimationTimeline>,
    pub active_frame: Option<FrameId>,
    pub active_layer: LayerId,
    pub artwork_changed: bool,
}

fn failure(error: impl std::fmt::Debug) -> String {
    format!("애니메이션: {error:?}")
}

/// # Errors
/// Rejects missing cels instead of exposing another frame's artwork for editing.
pub fn active_edit_tree(
    tree: &LayerTree,
    timeline: Option<&AnimationTimeline>,
    frame: Option<FrameId>,
) -> Result<LayerTree, String> {
    let Some(timeline) = timeline else {
        return Ok(tree.clone());
    };
    timeline.validate_against(tree).map_err(failure)?;
    let frame = frame
        .or_else(|| timeline.frames().first().map(|frame| frame.id))
        .ok_or("프레임이 없습니다.")?;
    let cel = timeline.cel_for_frame(frame).ok_or("없는 프레임입니다.")?;
    let group = tree
        .root()
        .children
        .iter()
        .find_map(|node| match node {
            LayerTreeNode::Group(group) if group.id == cel.root_group => Some(group.clone()),
            _ => None,
        })
        .ok_or("셀을 찾지 못했습니다.")?;
    LayerTree::new(group).map_err(failure)
}

/// # Errors
/// Rejects cross-cel identifiers, root replacement and malformed ownership.
pub fn merge_active_edit_tree(
    tree: &LayerTree,
    timeline: Option<&AnimationTimeline>,
    frame: Option<FrameId>,
    edited: &LayerTree,
) -> Result<LayerTree, String> {
    let original = active_edit_tree(tree, timeline, frame)?;
    if original.root_id() != edited.root_id() {
        return Err("현재 셀의 루트를 교체할 수 없습니다.".into());
    }
    let Some(timeline) = timeline else {
        return Ok(edited.clone());
    };
    let mut root = tree.root().clone();
    let index = root
        .children
        .iter()
        .position(|node| node.id() == LayerTreeNodeId::Group(edited.root_id()))
        .ok_or("셀을 찾지 못했습니다.")?;
    root.children[index] = LayerTreeNode::Group(edited.root().clone());
    let merged = LayerTree::new(root).map_err(failure)?;
    timeline.validate_against(&merged).map_err(failure)?;
    Ok(merged)
}

/// Produces one immutable artwork candidate. The caller preflights resources and
/// persists it atomically before adoption; frame selection has no artwork edit.
/// # Errors
/// Rejects invalid identities, durations, last-frame deletion and exhausted IDs.
#[allow(clippy::too_many_arguments)]
pub fn prepare_animation_edit(
    command: AnimationCommand,
    tree: &LayerTree,
    tiles: &TileSnapshot,
    timeline: Option<&AnimationTimeline>,
    active_frame: Option<FrameId>,
    active_layer: LayerId,
    canvas: CanvasSpec,
    next_id: u128,
) -> Result<PreparedAnimationEdit, String> {
    let mut next_id = if matches!(
        command,
        AnimationCommand::Enable
            | AnimationCommand::InsertBlank { .. }
            | AnimationCommand::Duplicate(_)
    ) {
        next_identity(tree, timeline, next_id)?
    } else {
        next_id
    };
    let (new_tree, new_tiles, new_timeline, selected) = if command == AnimationCommand::Enable {
        if let Some(timeline) = timeline {
            (
                tree.clone(),
                tiles.clone(),
                timeline.clone(),
                active_frame.unwrap_or(timeline.frames()[0].id),
            )
        } else {
            enable(tree, tiles, &mut next_id)?
        }
    } else {
        let timeline = timeline.ok_or("먼저 애니메이션을 시작하세요.")?;
        timeline.validate_against(tree).map_err(failure)?;
        mutate(
            command,
            tree,
            tiles,
            timeline,
            active_frame,
            canvas,
            &mut next_id,
        )?
    };
    let active_tree = active_edit_tree(&new_tree, Some(&new_timeline), Some(selected))?;
    let active_layer = valid_active_layer(&active_tree, active_layer);
    if active_tree.raster(active_layer).is_none() {
        return Err("그릴 레이어가 없는 셀입니다.".into());
    }
    let artwork_changed =
        &new_tree != tree || new_tiles.root() != tiles.root() || Some(&new_timeline) != timeline;
    Ok(PreparedAnimationEdit {
        tree: new_tree,
        tiles: new_tiles,
        timeline: Some(new_timeline),
        active_frame: Some(selected),
        active_layer,
        artwork_changed,
    })
}

fn next_identity(
    tree: &LayerTree,
    timeline: Option<&AnimationTimeline>,
    suggested: u128,
) -> Result<u128, String> {
    let mut next = next_layer_node_id(tree)?.max(suggested).max(1);
    if let Some(timeline) = timeline {
        for id in timeline
            .frames()
            .iter()
            .map(|frame| frame.id.0)
            .chain(timeline.cels().iter().map(|cel| cel.id.0))
        {
            next = next.max(id.checked_add(1).ok_or("ID가 소진되었습니다.")?);
        }
    }
    Ok(next)
}

fn take_id(next: &mut u128) -> Result<u128, String> {
    let id = *next;
    *next = next.checked_add(1).ok_or("ID가 소진되었습니다.")?;
    Ok(id)
}

fn cel_group(id: GroupId, children: Vec<LayerTreeNode>) -> GroupNode {
    GroupNode {
        id,
        name: "셀".into(),
        visible: true,
        opacity_u16: u16::MAX,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        children,
    }
}

type Candidate = (LayerTree, TileSnapshot, AnimationTimeline, FrameId);

fn enable(tree: &LayerTree, tiles: &TileSnapshot, next: &mut u128) -> Result<Candidate, String> {
    let group = GroupId(take_id(next)?);
    let cel = CelId(take_id(next)?);
    let frame = FrameId(take_id(next)?);
    let mut root = tree.root().clone();
    let mut children = std::mem::take(&mut root.children);
    if raster_layer_ids(tree).is_empty() {
        children.push(LayerTreeNode::Raster(empty_raster(
            LayerId(take_id(next)?),
            "레이어 1".into(),
        )));
    }
    root.children
        .push(LayerTreeNode::Group(cel_group(group, children)));
    let tree = LayerTree::new(root).map_err(failure)?;
    let timeline = AnimationTimeline::new(
        vec![AnimationFrame {
            id: frame,
            cel,
            duration_ms: 100,
        }],
        vec![AnimationCel {
            id: cel,
            root_group: group,
        }],
        &tree,
    )
    .map_err(failure)?;
    Ok((tree, tiles.clone(), timeline, frame))
}

#[allow(clippy::too_many_arguments)]
fn mutate(
    command: AnimationCommand,
    tree: &LayerTree,
    tiles: &TileSnapshot,
    timeline: &AnimationTimeline,
    active: Option<FrameId>,
    canvas: CanvasSpec,
    next: &mut u128,
) -> Result<Candidate, String> {
    let mut tree = tree.clone();
    let mut tiles = tiles.clone();
    let mut frames = timeline.frames().to_vec();
    let mut cels = timeline.cels().to_vec();
    let mut selected = active.unwrap_or(frames[0].id);
    match command {
        AnimationCommand::Select(frame) => {
            frame_index(&frames, frame)?;
            selected = frame;
        }
        AnimationCommand::InsertBlank { after } | AnimationCommand::Duplicate(after) => {
            let index = frame_index(&frames, after)?;
            let source = frames[index];
            let (cel, frame, group) = (
                CelId(take_id(next)?),
                FrameId(take_id(next)?),
                GroupId(take_id(next)?),
            );
            if matches!(command, AnimationCommand::Duplicate(_)) {
                let source_group = timeline
                    .cel_for_frame(after)
                    .ok_or("없는 셀입니다.")?
                    .root_group;
                let source_tree = tree.clone();
                let mapping = tree
                    .duplicate_group(source_group, group.0)
                    .map_err(failure)?;
                tiles = LayerPixelChange::DuplicateMany(mapping).apply_to_tree(
                    &tiles,
                    canvas,
                    &source_tree,
                    &tree,
                )?;
            } else {
                let layer = LayerId(take_id(next)?);
                let children = vec![LayerTreeNode::Raster(empty_raster(
                    layer,
                    "레이어 1".into(),
                ))];
                tree.insert(
                    tree.root_id(),
                    tree.root().children.len(),
                    LayerTreeNode::Group(cel_group(group, children)),
                )
                .map_err(failure)?;
            }
            cels.push(AnimationCel {
                id: cel,
                root_group: group,
            });
            frames.insert(
                index + 1,
                AnimationFrame {
                    id: frame,
                    cel,
                    duration_ms: source.duration_ms,
                },
            );
            selected = frame;
        }
        AnimationCommand::Delete(frame) => {
            let index = frame_index(&frames, frame)?;
            if frames.len() == 1 {
                return Err("마지막 프레임은 삭제할 수 없습니다.".into());
            }
            let removed = frames.remove(index);
            if !frames.iter().any(|frame| frame.cel == removed.cel) {
                let cel = cels
                    .iter()
                    .find(|cel| cel.id == removed.cel)
                    .ok_or("없는 셀입니다.")?;
                tiles = remove_cel_artwork(&mut tree, &tiles, cel.root_group)?;
                cels.retain(|cel| cel.id != removed.cel);
            }
            if selected == frame {
                selected = frames[index.min(frames.len() - 1)].id;
            }
        }
        AnimationCommand::Move { frame, index } => {
            let source = frame_index(&frames, frame)?;
            if usize::from(index) >= frames.len() {
                return Err("잘못된 프레임 위치입니다.".into());
            }
            let frame = frames.remove(source);
            frames.insert(usize::from(index), frame);
        }
        AnimationCommand::SetDuration { frame, duration_ms } => {
            let index = frame_index(&frames, frame)?;
            frames[index].duration_ms = duration_ms;
        }
        _ => return Err("재생 상태는 작품 편집 명령이 아닙니다.".into()),
    }
    let timeline = AnimationTimeline::new(frames, cels, &tree).map_err(failure)?;
    frame_index(timeline.frames(), selected)?;
    Ok((tree, tiles, timeline, selected))
}

fn remove_cel_artwork(
    tree: &mut LayerTree,
    tiles: &TileSnapshot,
    root: GroupId,
) -> Result<TileSnapshot, String> {
    tree.remove(LayerTreeNodeId::Group(root)).map_err(failure)?;
    let layers = raster_layer_ids(tree);
    let masks = tree.mask_ids();
    TileSnapshot::from_planes(
        tiles
            .iter()
            .filter(|(key, _)| layers.contains(&key.layer))
            .map(|(key, tile)| (key, tile.clone())),
        tiles
            .coverage_iter()
            .filter(|(key, _)| masks.contains(&key.mask))
            .map(|(key, tile)| (key, tile.clone())),
    )
    .map_err(failure)
}

fn frame_index(frames: &[AnimationFrame], frame: FrameId) -> Result<usize, String> {
    frames
        .iter()
        .position(|entry| entry.id == frame)
        .ok_or("없는 프레임입니다.".into())
}

#[must_use]
pub fn animation_projection(
    timeline: Option<&AnimationTimeline>,
    active_frame: Option<FrameId>,
    playing: bool,
    looping: bool,
    onion_skin: bool,
) -> AnimationProjection {
    AnimationProjection {
        frames: timeline.map_or_else(Vec::new, |timeline| {
            timeline
                .frames()
                .iter()
                .map(|frame| AnimationFrameProjection {
                    id: frame.id,
                    duration_ms: frame.duration_ms,
                })
                .collect()
        }),
        active_frame,
        playing,
        looping,
        onion_skin,
    }
}

/// Resolves elapsed time without tick accumulation or artwork/history mutation.
/// # Errors
/// Rejects an unknown starting frame.
pub fn playback_frame(
    timeline: &AnimationTimeline,
    start: FrameId,
    elapsed_ms: u64,
    looping: bool,
) -> Result<(FrameId, bool), String> {
    let index = frame_index(timeline.frames(), start)?;
    let frames = timeline.frames();
    let total: u64 = frames
        .iter()
        .map(|frame| u64::from(frame.duration_ms))
        .sum();
    let mut remaining = if looping {
        elapsed_ms % total
    } else {
        elapsed_ms
    };
    for frame in frames[index..].iter().chain(if looping {
        frames[..index].iter()
    } else {
        frames[0..0].iter()
    }) {
        if remaining < u64::from(frame.duration_ms) {
            return Ok((frame.id, false));
        }
        remaining -= u64::from(frame.duration_ms);
    }
    Ok((frames[frames.len() - 1].id, !looping))
}

#[cfg(test)]
mod tests;
