use nyatidraw_api::GroupId;
use nyatidraw_document::{
    AnimationCel, AnimationFrame, AnimationTimeline, CelId, FrameId, LayerTree,
    MAX_ANIMATION_FRAMES,
};

use crate::WireError;

pub const MAX_ANIMATION_RECORD_BYTES: usize = 32 * 1024;

/// Encodes timeline artwork metadata; `None` represents a static document.
/// # Errors
/// Rejects records larger than the fixed metadata limit.
pub fn encode_animation_timeline(
    timeline: Option<&AnimationTimeline>,
) -> Result<Vec<u8>, WireError> {
    let mut bytes = vec![1, 0, u8::from(timeline.is_some())];
    if let Some(timeline) = timeline {
        bytes.extend_from_slice(
            &u32::try_from(timeline.frames().len())
                .map_err(|_| WireError::InvalidData("animation frame limit"))?
                .to_le_bytes(),
        );
        for frame in timeline.frames() {
            bytes.extend_from_slice(&frame.id.0.to_le_bytes());
            bytes.extend_from_slice(&frame.cel.0.to_le_bytes());
            bytes.extend_from_slice(&frame.duration_ms.to_le_bytes());
        }
        bytes.extend_from_slice(
            &u32::try_from(timeline.cels().len())
                .map_err(|_| WireError::InvalidData("animation cel limit"))?
                .to_le_bytes(),
        );
        for cel in timeline.cels() {
            bytes.extend_from_slice(&cel.id.0.to_le_bytes());
            bytes.extend_from_slice(&cel.root_group.0.to_le_bytes());
        }
    }
    if bytes.len() + 52 > MAX_ANIMATION_RECORD_BYTES {
        return Err(WireError::InvalidData("animation record limit"));
    }
    Ok(bytes)
}

/// # Errors
/// Rejects unsupported versions, malformed records and invalid cel/tree references.
pub fn decode_animation_timeline(
    bytes: &[u8],
    tree: &LayerTree,
) -> Result<Option<AnimationTimeline>, WireError> {
    if bytes.len() + 52 > MAX_ANIMATION_RECORD_BYTES {
        return Err(WireError::InvalidData("animation record limit"));
    }
    let mut reader = Reader(bytes);
    if reader.take::<2>()? != [1, 0] {
        return Err(WireError::InvalidData("unsupported animation version"));
    }
    let timeline = match reader.take::<1>()? {
        [0] => None,
        [1] => {
            let count = reader.count()?;
            let mut frames = Vec::with_capacity(count);
            for _ in 0..count {
                frames.push(AnimationFrame {
                    id: FrameId(u128::from_le_bytes(reader.take()?)),
                    cel: CelId(u128::from_le_bytes(reader.take()?)),
                    duration_ms: u32::from_le_bytes(reader.take()?),
                });
            }
            let count = reader.count()?;
            let mut cels = Vec::with_capacity(count);
            for _ in 0..count {
                cels.push(AnimationCel {
                    id: CelId(u128::from_le_bytes(reader.take()?)),
                    root_group: GroupId(u128::from_le_bytes(reader.take()?)),
                });
            }
            Some(
                AnimationTimeline::new(frames, cels, tree)
                    .map_err(|_| WireError::InvalidData("invalid animation timeline"))?,
            )
        }
        _ => return Err(WireError::InvalidData("invalid animation presence")),
    };
    if !reader.0.is_empty() {
        return Err(WireError::InvalidData("trailing animation bytes"));
    }
    Ok(timeline)
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let value = self
            .0
            .get(..N)
            .ok_or(WireError::InvalidData("truncated animation"))?;
        let value = value
            .try_into()
            .map_err(|_| WireError::InvalidData("truncated animation"))?;
        self.0 = &self.0[N..];
        Ok(value)
    }
    fn count(&mut self) -> Result<usize, WireError> {
        let count = u32::from_le_bytes(self.take()?) as usize;
        if count == 0 || count > MAX_ANIMATION_FRAMES {
            return Err(WireError::InvalidData("animation count limit"));
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::LayerBlendMode;
    use nyatidraw_document::{GroupNode, LayerTreeNode};

    #[test]
    fn malformed_timing_and_cel_references_cannot_decode_as_other_artwork() {
        let group = |id| GroupNode {
            id: GroupId(id),
            name: format!("Group {id}"),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            children: Vec::new(),
        };
        let mut root = group(1);
        root.children = vec![
            LayerTreeNode::Group(group(2)),
            LayerTreeNode::Group(group(3)),
        ];
        let tree = LayerTree::new(root).unwrap();
        let timeline = AnimationTimeline::new(
            vec![
                AnimationFrame {
                    id: FrameId(1),
                    cel: CelId(1),
                    duration_ms: 100,
                },
                AnimationFrame {
                    id: FrameId(2),
                    cel: CelId(2),
                    duration_ms: 200,
                },
            ],
            vec![
                AnimationCel {
                    id: CelId(1),
                    root_group: GroupId(2),
                },
                AnimationCel {
                    id: CelId(2),
                    root_group: GroupId(3),
                },
            ],
            &tree,
        )
        .unwrap();
        let original = encode_animation_timeline(Some(&timeline)).unwrap();
        assert_eq!(
            decode_animation_timeline(&original, &tree).unwrap(),
            Some(timeline.clone())
        );
        assert_eq!(
            decode_animation_timeline(&encode_animation_timeline(None).unwrap(), &tree).unwrap(),
            None
        );
        for corruption in 0..8 {
            let mut bytes = original.clone();
            match corruption {
                0 => bytes[0] = 2,
                1 => bytes.truncate(5),
                2 => bytes[3..7].copy_from_slice(&257_u32.to_le_bytes()),
                3 => bytes[43..59].copy_from_slice(&1_u128.to_le_bytes()),
                4 => bytes[23..39].copy_from_slice(&99_u128.to_le_bytes()),
                5 => bytes[39..43].copy_from_slice(&0_u32.to_le_bytes()),
                6 => bytes[99..115].copy_from_slice(&99_u128.to_le_bytes()),
                _ => bytes.push(0),
            }
            assert!(
                decode_animation_timeline(&bytes, &tree).is_err(),
                "corruption {corruption}"
            );
        }
        let mut changed = tree.root().clone();
        if let LayerTreeNode::Group(group) = &mut changed.children[0] {
            group.visible = false;
        }
        assert!(
            timeline
                .validate_against(&LayerTree::new(changed).unwrap())
                .is_err()
        );
        assert_eq!(
            encode_animation_timeline(Some(&timeline)).unwrap(),
            original
        );
    }
}
