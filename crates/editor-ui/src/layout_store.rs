use nyatidraw_api::PanelKind;
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
