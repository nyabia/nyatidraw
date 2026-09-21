use std::collections::BTreeMap;

use nyatidraw_brush::{BrushDab, BrushEvaluator, RoundBrushStroke, begin_round_stroke};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StrokeSmoother, StylusSample};
use nyatidraw_paint_cpu::{CpuCanvas, PremultipliedRgba8};

use crate::{
    Artwork, BrushPreset, CanvasUpdate, MAX_COORDINATE, MAX_RESIDENT_TILES, StrokePoint,
    TILE_BYTE_LEN, TILE_EDGE, TileKey, WebDocument, WebError, WebTool, difference,
    srgb8_to_linear_premultiplied,
};

const MAX_DABS_PER_UPDATE: u32 = 2048;
const MAX_SAMPLES_PER_STROKE: u64 = 65_536;
const MAX_DABS_PER_STROKE: usize = 262_144;
const MAX_UPDATE_RASTER_WORK: f64 = 8_388_608.0;
const MAX_UPDATE_TILES: usize = 128;

pub(super) struct ActiveStroke {
    pub(super) before: Artwork,
    token: RoundBrushStroke,
    preset: BrushPreset,
    smoother: StrokeSmoother,
    last: StylusSample,
    color: PremultipliedRgba8,
    erase: bool,
    alpha_locked: bool,
    dabs: usize,
}

impl WebDocument {
    #[must_use]
    pub fn brush_preset(&self) -> BrushPreset {
        let settings = self.brush_settings();
        BrushPreset {
            size_px: settings.size_px,
            opacity: settings.opacity,
            hardness: settings.hardness,
            size_pressure: settings.size_pressure,
            opacity_pressure: settings.opacity_pressure,
            size_min_ratio: f32::from(settings.size_minimum_u16) / f32::from(u16::MAX),
            opacity_min_ratio: f32::from(settings.opacity_minimum_u16) / f32::from(u16::MAX),
            ..self.tool.preset()
        }
    }

    /// Starts a captured stroke and immediately paints its tap dab.
    /// # Errors
    /// Rejects invalid input, hidden/locked layers, or an already-live stroke.
    /// On any paint error the entire gesture is cancelled; hosts must resync tiles.
    pub fn begin_stroke(&mut self, point: StrokePoint) -> Result<CanvasUpdate, WebError> {
        if self.stroke.is_some() {
            self.cancel_stroke();
            return Err(WebError::StrokeInProgress);
        }
        let mut sample = make_sample(point, 1, PointerPhase::Begin)?;
        let layer = self
            .layers()
            .raster(self.active_layer())
            .ok_or(WebError::InvalidLayer)?;
        if !layer.visible || layer.locked || (layer.alpha_locked && self.tool == WebTool::Eraser) {
            return Err(WebError::LayerUnavailable);
        }
        sample.eraser = self.tool == WebTool::Eraser;
        let alpha_locked = layer.alpha_locked;
        let preset = self.brush_preset();
        let mut smoother = StrokeSmoother::new(self.brush_settings().smoothing);
        let sample = smoother
            .process(sample)
            .map_err(|_| WebError::InvalidInput)?;
        let mut dabs = Vec::with_capacity(1);
        let token = begin_round_stroke(&mut self.evaluator, &preset, sample, &mut dabs);
        self.stroke = Some(ActiveStroke {
            before: self.artwork.clone(),
            token,
            preset,
            smoother,
            last: sample,
            color: PremultipliedRgba8(srgb8_to_linear_premultiplied(self.foreground)),
            erase: sample.eraser,
            alpha_locked,
            dabs: 1,
        });
        self.paint_or_cancel(&dabs)
    }

    /// Applies one document-space sample. Batch coalesced browser samples in order.
    /// # Errors
    /// Invalid/nonfinite input or exhausted work bounds cancel the whole live stroke.
    pub fn move_stroke(&mut self, point: StrokePoint) -> Result<CanvasUpdate, WebError> {
        self.advance_stroke(point, PointerPhase::Move)
    }

    /// Finishes at the supplied endpoint, adding exactly one non-empty history step.
    /// # Errors
    /// Invalid input or exhausted work bounds cancel the stroke. No-op paint is not an error.
    pub fn end_stroke(&mut self, point: StrokePoint) -> Result<CanvasUpdate, WebError> {
        self.advance_stroke(point, PointerPhase::End)?;
        let stroke = self.stroke.take().ok_or(WebError::NoStroke)?;
        self.evaluator.end(stroke.token, &mut Vec::new());
        let update = difference(&stroke.before, &self.artwork, true);
        if update.changed {
            self.redo.clear();
            self.undo.push_back(stroke.before);
            self.trim_history();
        }
        Ok(update)
    }

    /// Restores the pre-stroke snapshot without changing history/tool preferences.
    pub fn cancel_stroke(&mut self) -> CanvasUpdate {
        let Some(stroke) = self.stroke.take() else {
            return CanvasUpdate::default();
        };
        let update = difference(&self.artwork, &stroke.before, false);
        self.artwork = stroke.before;
        update
    }

    /// Samples composed artwork, including outside-page pixels, without checkerboard.
    /// # Errors
    /// Rejects points outside the bounded web drawing coordinate range.
    pub fn sample_rgba8(&self, x: i32, y: i32) -> Result<[u8; 4], WebError> {
        if f64::from(x).abs() > MAX_COORDINATE || f64::from(y).abs() > MAX_COORDINATE {
            return Err(WebError::InvalidInput);
        }
        let pixel =
            nyatidraw_paint_cpu::sample_display_pixel(self.snapshot(), self.layers(), [x, y], None)
                .map_err(|_| WebError::InvalidPixels)?;
        Ok(crate::linear_premultiplied_to_srgb8(pixel).unwrap_or([0; 4]))
    }

    fn advance_stroke(
        &mut self,
        point: StrokePoint,
        phase: PointerPhase,
    ) -> Result<CanvasUpdate, WebError> {
        let result = self.evaluate_next(point, phase);
        match result {
            Ok(dabs) => self.paint_or_cancel(&dabs),
            Err(error) => {
                self.cancel_stroke();
                Err(error)
            }
        }
    }

    fn evaluate_next(
        &mut self,
        point: StrokePoint,
        phase: PointerPhase,
    ) -> Result<Vec<BrushDab>, WebError> {
        let stroke = self.stroke.as_mut().ok_or(WebError::NoStroke)?;
        if stroke.last.sequence >= MAX_SAMPLES_PER_STROKE {
            return Err(WebError::LimitExceeded);
        }
        let mut sample = make_sample(point, stroke.last.sequence + 1, phase)?;
        sample.eraser = stroke.erase;
        let sample = stroke
            .smoother
            .process(sample)
            .map_err(|_| WebError::InvalidInput)?;
        let distance = (sample.position_document.x - stroke.last.position_document.x)
            .hypot(sample.position_document.y - stroke.last.position_document.y);
        let spacing = f64::from((stroke.preset.size_px * stroke.preset.spacing_ratio).max(0.25));
        if distance / spacing + 2.0 > f64::from(MAX_DABS_PER_UPDATE) {
            return Err(WebError::LimitExceeded);
        }
        let mut dabs = Vec::new();
        self.evaluator.push(&mut stroke.token, &[sample], &mut dabs);
        stroke.dabs += dabs.len();
        if stroke.token.is_discontinuous() || stroke.dabs > MAX_DABS_PER_STROKE {
            return Err(WebError::LimitExceeded);
        }
        stroke.last = sample;
        Ok(dabs)
    }

    fn paint_or_cancel(&mut self, dabs: &[BrushDab]) -> Result<CanvasUpdate, WebError> {
        match self.paint_dabs(dabs) {
            Ok(update) => Ok(update),
            Err(error) => {
                self.cancel_stroke();
                Err(error)
            }
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn paint_dabs(&mut self, dabs: &[BrushDab]) -> Result<CanvasUpdate, WebError> {
        let stroke = self.stroke.as_ref().ok_or(WebError::NoStroke)?;
        let mut work = 0.0;
        let mut tiles: BTreeMap<TileKey, CpuCanvas> = BTreeMap::new();
        for &dab in dabs {
            if dab.radius_px <= 0.0 || dab.opacity * dab.flow <= 0.0 {
                continue;
            }
            work += (f64::from(dab.radius_px) * 2.0 + 2.0).powi(2);
            if work > MAX_UPDATE_RASTER_WORK {
                return Err(WebError::LimitExceeded);
            }
            let radius = f64::from(dab.radius_px);
            let edge = f64::from(TILE_EDGE);
            let left = ((dab.center.x - radius) / edge).floor() as i32;
            let right = ((dab.center.x + radius) / edge).floor() as i32;
            let top = ((dab.center.y - radius) / edge).floor() as i32;
            let bottom = ((dab.center.y + radius) / edge).floor() as i32;
            for y in top..=bottom {
                for x in left..=right {
                    let key = TileKey {
                        layer: self.artwork.active,
                        x,
                        y,
                        mip: 0,
                    };
                    if !tiles.contains_key(&key) {
                        if tiles.len() >= MAX_UPDATE_TILES {
                            return Err(WebError::LimitExceeded);
                        }
                        let pixels = self
                            .artwork
                            .tiles
                            .get(key)
                            .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
                        tiles.insert(
                            key,
                            CpuCanvas::from_rgba8_premultiplied(TILE_EDGE, TILE_EDGE, pixels)
                                .map_err(|_| WebError::InvalidPixels)?,
                        );
                    }
                    let canvas = tiles.get_mut(&key).ok_or(WebError::InvalidPixels)?;
                    let (ox, oy) = key.pixel_origin();
                    let local = dab.to_local(ox, oy);
                    if stroke.erase {
                        canvas.erase_dab(local);
                    } else if stroke.alpha_locked {
                        canvas.apply_dab_alpha_locked(local, stroke.color);
                    } else {
                        canvas.apply_dab(local, stroke.color);
                    }
                }
            }
        }
        if tiles.is_empty() {
            return Ok(CanvasUpdate::default());
        }
        let mut next = self.artwork.clone();
        next.tiles = self
            .artwork
            .tiles
            .with_replacements(
                tiles
                    .into_iter()
                    .map(|(key, canvas)| (key, canvas.pixels_rgba8_premultiplied().to_vec())),
            )
            .map_err(|_| WebError::InvalidPixels)?;
        if next.tiles.len() > MAX_RESIDENT_TILES {
            return Err(WebError::LimitExceeded);
        }
        let update = difference(&self.artwork, &next, false);
        if update.changed {
            self.artwork = next;
        }
        Ok(update)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn make_sample(
    point: StrokePoint,
    sequence: u64,
    phase: PointerPhase,
) -> Result<StylusSample, WebError> {
    if !point.x.is_finite()
        || !point.y.is_finite()
        || point.x.abs() > MAX_COORDINATE
        || point.y.abs() > MAX_COORDINATE
        || !point.pressure.is_finite()
        || !(0.0..=1.0).contains(&point.pressure)
        || !point.time_ms.is_finite()
        || !(0.0..=1.0e12).contains(&point.time_ms)
    {
        return Err(WebError::InvalidInput);
    }
    Ok(StylusSample {
        sequence,
        timestamp_ns: (point.time_ms * 1_000_000.0) as u64,
        device_id: 1,
        phase,
        position_document: Point {
            x: point.x,
            y: point.y,
        },
        pressure: point.pressure,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 0,
    })
}
