use crate::{
    Artwork, BrushSettings, CanvasSpec, LayerId, LayerTree, MAX_LAYERS, MAX_RESIDENT_TILES,
    TileSnapshot, WebDocument, WebError, WebTool,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WebPreferences {
    pub edit_settings: nyatidraw_api::EditSettings,
    pub tool: WebTool,
    pub brushes: [BrushSettings; 5],
    pub foreground: [u8; 4],
    pub background: [u8; 4],
}

impl WebDocument {
    /// Adopts exact linear-premultiplied native pixels without flattening or cropping.
    /// # Errors
    /// Rejects missing layers, non-base tiles and browser limits.
    pub fn from_snapshot(
        canvas: CanvasSpec,
        tiles: TileSnapshot,
        layers: LayerTree,
        active: LayerId,
    ) -> Result<Self, WebError> {
        let mut document = Self::new(canvas)?;
        if layers.root().children.is_empty()
            || crate::editing::node_count(&layers) > MAX_LAYERS
            || tiles.len() > MAX_RESIDENT_TILES
        {
            return Err(WebError::LimitExceeded);
        }
        let next = nyatidraw_editor::layer_edit::next_layer_node_id(&layers)
            .map_err(WebError::EditRejected)?;
        if layers.raster(active).is_none() {
            return Err(WebError::InvalidLayer);
        }
        for (key, tile) in tiles.iter() {
            if key.mip != 0 || key.x.unsigned_abs() > 257 || key.y.unsigned_abs() > 257 {
                return Err(WebError::LimitExceeded);
            }
            if layers.raster(key.layer).is_none() {
                return Err(WebError::InvalidLayer);
            }
            if tile
                .pixels()
                .chunks_exact(4)
                .any(|pixel| pixel[..3].iter().any(|c| *c > pixel[3]))
            {
                return Err(WebError::InvalidPixels);
            }
        }
        document.artwork = Artwork {
            canvas,
            tiles,
            layers,
            active,
        };
        document.next_layer_id = next;
        Ok(document)
    }

    #[must_use]
    pub const fn preferences(&self) -> WebPreferences {
        WebPreferences {
            edit_settings: self.edit_settings,
            tool: self.tool,
            brushes: self.settings,
            foreground: self.foreground,
            background: self.background,
        }
    }

    /// Replaces all remembered settings atomically, outside artwork undo.
    /// # Errors
    /// Rejects invalid settings and active strokes without partial replacement.
    pub fn restore_preferences(&mut self, preferences: WebPreferences) -> Result<(), WebError> {
        self.require_idle()?;
        for settings in preferences.brushes {
            settings.validate()?;
        }
        if preferences.edit_settings.fill.gap_close_px > 8
            || preferences.edit_settings.fill.expand_px > 64
        {
            return Err(WebError::InvalidInput);
        }
        self.tool = preferences.tool;
        self.edit_settings = preferences.edit_settings;
        self.settings = preferences.brushes;
        self.foreground = preferences.foreground;
        self.background = preferences.background;
        Ok(())
    }
}
