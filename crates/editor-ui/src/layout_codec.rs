use crate::layout_store::PANELS;
use nyatidraw_api::{DockAxis, DockLayoutError, DockNode, DockTree, PanelKind};
const MAGIC: &[u8; 8] = b"NYDOCK03";
const MAX_BYTES: usize = 4096;

/// # Panics
/// Requires a validated `DockTree` with bounded, known panel identifiers.
#[must_use]
pub fn encode(tree: &DockTree) -> Vec<u8> {
    fn node(value: &DockNode, bytes: &mut Vec<u8>) {
        let panel_id =
            |panel| u8::try_from(PANELS.iter().position(|p| *p == panel).unwrap()).unwrap();
        match value {
            DockNode::Panel(panel) => bytes.extend([0, panel_id(*panel)]),
            DockNode::Tabs { active, panels } => {
                bytes.extend([
                    1,
                    u8::try_from(*active).unwrap(),
                    u8::try_from(panels.len()).unwrap(),
                ]);
                bytes.extend(panels.iter().map(|p| panel_id(*p)));
            }
            DockNode::Split {
                axis,
                first_per_mille,
                first,
                second,
            } => {
                bytes.extend([2, u8::from(*axis == DockAxis::Vertical)]);
                bytes.extend(first_per_mille.to_le_bytes());
                node(first, bytes);
                node(second, bytes);
            }
        }
    }
    let mut bytes = MAGIC.to_vec();
    bytes.push(u8::try_from(tree.top().len()).unwrap());
    for panel in tree.top() {
        bytes.push(u8::try_from(PANELS.iter().position(|p| p == panel).unwrap()).unwrap());
    }
    node(tree.root(), &mut bytes);
    bytes
}

/// # Errors
/// Rejects corrupt, oversized or invalid saved layouts.
pub fn decode(bytes: &[u8]) -> Result<DockTree, DockLayoutError> {
    fn byte(bytes: &mut &[u8]) -> Result<u8, DockLayoutError> {
        let (value, rest) = bytes.split_first().ok_or(DockLayoutError::CorruptPayload)?;
        *bytes = rest;
        Ok(*value)
    }
    fn panel(bytes: &mut &[u8]) -> Result<PanelKind, DockLayoutError> {
        PANELS
            .get(usize::from(byte(bytes)?))
            .copied()
            .ok_or(DockLayoutError::CorruptPayload)
    }
    fn node(bytes: &mut &[u8], depth: usize) -> Result<DockNode, DockLayoutError> {
        if depth > 16 {
            return Err(DockLayoutError::TooDeep);
        }
        Ok(match byte(bytes)? {
            0 => DockNode::Panel(panel(bytes)?),
            1 => {
                let active = usize::from(byte(bytes)?);
                let len = usize::from(byte(bytes)?);
                if len == 0 || len > PANELS.len() {
                    return Err(DockLayoutError::CorruptPayload);
                }
                let panels = (0..len)
                    .map(|_| panel(bytes))
                    .collect::<Result<Vec<_>, _>>()?;
                DockNode::Tabs { active, panels }
            }
            2 => {
                let axis = match byte(bytes)? {
                    0 => DockAxis::Horizontal,
                    1 => DockAxis::Vertical,
                    _ => return Err(DockLayoutError::CorruptPayload),
                };
                let first_per_mille = u16::from_le_bytes([byte(bytes)?, byte(bytes)?]);
                DockNode::Split {
                    axis,
                    first_per_mille,
                    first: Box::new(node(bytes, depth + 1)?),
                    second: Box::new(node(bytes, depth + 1)?),
                }
            }
            _ => return Err(DockLayoutError::CorruptPayload),
        })
    }
    let legacy = bytes.starts_with(b"NYDOCK01");
    let combined_brush = legacy || bytes.starts_with(b"NYDOCK02");
    if bytes.len() > MAX_BYTES || !(bytes.starts_with(MAGIC) || combined_brush) {
        return Err(DockLayoutError::CorruptPayload);
    }
    let mut remaining = &bytes[MAGIC.len()..];
    let top = if legacy {
        PanelKind::TOOLBARS.to_vec()
    } else {
        let len = usize::from(byte(&mut remaining)?);
        if len > PanelKind::TOOLBARS.len() {
            return Err(DockLayoutError::CorruptPayload);
        }
        (0..len)
            .map(|_| panel(&mut remaining))
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut root = node(&mut remaining, 0)?;
    if !remaining.is_empty() {
        return Err(DockLayoutError::CorruptPayload);
    }
    if combined_brush {
        fn expand(node: DockNode) -> DockNode {
            match node {
                DockNode::Panel(PanelKind::Brush) => DockTree::brush_stack(node),
                DockNode::Tabs { ref panels, .. } if panels.contains(&PanelKind::Brush) => {
                    DockTree::brush_stack(node)
                }
                DockNode::Split {
                    axis,
                    first_per_mille,
                    first,
                    second,
                } => DockNode::Split {
                    axis,
                    first_per_mille,
                    first: Box::new(expand(*first)),
                    second: Box::new(expand(*second)),
                },
                _ => node,
            }
        }
        root = expand(root);
    }
    DockTree::with_top(root, top)
}
