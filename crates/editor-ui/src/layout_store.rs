use nyatidraw_api::{DockAxis, DockNode, PanelKind};
pub const PANELS: [PanelKind; 12] = [
    PanelKind::Canvas,
    PanelKind::Tools,
    PanelKind::Navigator,
    PanelKind::Layers,
    PanelKind::Brush,
    PanelKind::Color,
    PanelKind::History,
    PanelKind::CanvasActions,
    PanelKind::Viewport,
    PanelKind::QuickColors,
    PanelKind::ToolProperties,
    PanelKind::BrushSizes,
];
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelHeights {
    pub values: [u16; 12],
}
impl PanelHeights {
    #[must_use]
    pub fn get(&self, panel: PanelKind) -> Option<u16> {
        PANELS
            .iter()
            .position(|p| *p == panel)
            .map(|i| self.values[i])
            .filter(|v| *v != 0)
    }
    /// # Errors
    /// Rejects heights outside the supported 64–4096 pixel range.
    pub fn set(&mut self, panel: PanelKind, height: u16) -> Result<(), String> {
        if !(64..=4096).contains(&height) {
            return Err("panel height out of range".into());
        }
        let i = PANELS
            .iter()
            .position(|p| *p == panel)
            .ok_or("unknown panel")?;
        self.values[i] = height;
        Ok(())
    }
}

#[must_use]
pub fn minimum_width(node: &DockNode) -> u16 {
    match node {
        DockNode::Panel(panel) => match panel {
            PanelKind::Tools => 48,
            PanelKind::Brush | PanelKind::ToolProperties | PanelKind::BrushSizes => 224,
            PanelKind::Canvas => 240,
            PanelKind::Layers => 280,
            _ => 160,
        },
        DockNode::Tabs { panels, .. } => panels
            .iter()
            .map(|panel| minimum_width(&DockNode::Panel(*panel)))
            .max()
            .unwrap_or(160),
        DockNode::Split {
            axis,
            first,
            second,
            ..
        } => {
            let first = minimum_width(first);
            let second = minimum_width(second);
            if *axis == DockAxis::Horizontal {
                first.saturating_add(second).saturating_add(6)
            } else {
                first.max(second)
            }
        }
    }
}
