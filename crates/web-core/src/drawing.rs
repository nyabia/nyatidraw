use std::collections::BTreeMap;

use nyatidraw_brush::{
    BrushDab, BrushEvaluator, LiveDabLimits, RoundBrushStroke, begin_round_stroke,
};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StrokePath, StrokePathError, StylusSample};
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
    token: Option<RoundBrushStroke>,
    path: StrokePath,
    last: StylusSample,
    evaluated_samples: usize,
    color: PremultipliedRgba8,
    erase: bool,
    alpha_locked: bool,
    mask: Option<nyatidraw_document::LayerMask>,
    mask_work: nyatidraw_paint_cpu::MaskStrokeBudget,
    selection: Option<std::sync::Arc<nyatidraw_paint_cpu::SelectionMask>>,
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
        self.begin_stroke_at_scale(point, 1.0)
    }

    /// Captures document-to-logical-screen scale for bounded stroke correction.
    /// # Errors
    /// Rejects invalid input or scale; painting failures cancel the whole gesture.
    pub fn begin_stroke_at_scale(
        &mut self,
        point: StrokePoint,
        scale: f64,
    ) -> Result<CanvasUpdate, WebError> {
        if self.stroke.is_some() {
            self.cancel_stroke();
            return Err(WebError::StrokeInProgress);
        }
        self.require_idle()?;
        let mut sample = make_sample(point, 1, PointerPhase::Begin)?;
        let layer = self
            .render_layers()
            .raster(self.active_layer())
            .ok_or(WebError::InvalidLayer)?;
        let mask = matches!(
            self.raster_target(),
            nyatidraw_api::RasterEditTarget::Mask { .. }
        )
        .then_some(layer.mask)
        .flatten();
        if !layer.visible
            || layer.locked
            || (mask.is_none() && layer.alpha_locked && self.tool == WebTool::Eraser)
        {
            return Err(WebError::LayerUnavailable);
        }
        let visible = nyatidraw_document::composition_scope_nodes(
            self.render_layers().root(),
            nyatidraw_document::CompositionScope::Solo(self.solo.unwrap_or(
                nyatidraw_api::LayerTreeNodeId::Group(self.render_layers().root_id()),
            )),
        );
        if !visible.contains(&nyatidraw_api::LayerTreeNodeId::Raster(self.active_layer())) {
            return Err(WebError::LayerUnavailable);
        }
        sample.eraser = self.tool == WebTool::Eraser;
        let alpha_locked = mask.is_none() && layer.alpha_locked;
        let preset = self.brush_preset();
        let mut path = StrokePath::with_scale(self.brush_settings().smoothing, scale);
        let evaluated = path.process(sample).map_err(path_error)?;
        let sample = evaluated.as_slice()[0];
        let mut dabs = Vec::with_capacity(1);
        let token = begin_round_stroke(&mut self.evaluator, &preset, sample, &mut dabs);
        self.stroke = Some(ActiveStroke {
            before: self.artwork.clone(),
            token: Some(token),
            path,
            last: sample,
            evaluated_samples: 1,
            color: PremultipliedRgba8(srgb8_to_linear_premultiplied(self.foreground)),
            erase: sample.eraser,
            alpha_locked,
            mask,
            mask_work: nyatidraw_paint_cpu::MaskStrokeBudget::default(),
            selection: self.selection.clone(),
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
        let token = self
            .stroke
            .as_mut()
            .and_then(|stroke| stroke.token.take())
            .ok_or(WebError::NoStroke)?;
        let mut dabs = Vec::new();
        self.evaluator.end(token, &mut dabs);
        if !dabs.is_empty() {
            self.paint_or_cancel(&dabs)?;
        }
        let stroke = self.stroke.take().ok_or(WebError::NoStroke)?;
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
        let pixel = nyatidraw_paint_cpu::sample_display_pixel(
            self.snapshot(),
            self.render_layers(),
            [x, y],
            self.solo,
        )
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
        let evaluated = stroke.path.process(sample).map_err(path_error)?;
        stroke.evaluated_samples += evaluated.as_slice().len();
        if stroke.evaluated_samples as u64 > MAX_SAMPLES_PER_STROKE {
            return Err(WebError::LimitExceeded);
        }
        let mut dabs = Vec::new();
        let token = stroke.token.as_mut().ok_or(WebError::NoStroke)?;
        for evaluated in evaluated.as_slice() {
            let remaining = u64::from(MAX_DABS_PER_UPDATE).saturating_sub(dabs.len() as u64);
            self.evaluator.push_live(
                token,
                &[*evaluated],
                &mut dabs,
                LiveDabLimits {
                    per_segment: remaining,
                    per_stroke: MAX_DABS_PER_STROKE as u64,
                },
            );
            if token.is_discontinuous() {
                return Err(WebError::LimitExceeded);
            }
        }
        stroke.dabs += dabs.len();
        if token.is_discontinuous() || stroke.dabs > MAX_DABS_PER_STROKE {
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

    fn paint_mask_dabs(&mut self, dabs: &[BrushDab]) -> Result<CanvasUpdate, WebError> {
        let stroke = self.stroke.as_mut().ok_or(WebError::NoStroke)?;
        let mask = stroke.mask.ok_or(WebError::InvalidLayer)?;
        stroke.mask_work.admit(dabs).map_err(|_| WebError::EditRejected(
            "마스크 획의 안전한 작업 한도를 넘어 현재 선을 취소했습니다. 더 짧게 나누어 그려주세요.".into()
        ))?;
        let mut next = self.artwork.clone();
        next.tiles = nyatidraw_paint_cpu::paint_mask_dabs(
            &next.tiles,
            mask,
            dabs,
            stroke.color,
            stroke.erase,
            stroke.selection.as_deref(),
            nyatidraw_paint_cpu::EditLimits::default(),
        )
        .map_err(|error| WebError::EditRejected(format!("마스크 획: {error:?}")))?;
        if next.tiles.total_len() > MAX_RESIDENT_TILES {
            return Err(WebError::LimitExceeded);
        }
        let update = difference(&self.artwork, &next, false);
        self.artwork = next;
        Ok(update)
    }

    #[allow(clippy::cast_possible_truncation)]
    fn paint_dabs(&mut self, dabs: &[BrushDab]) -> Result<CanvasUpdate, WebError> {
        let stroke = self.stroke.as_ref().ok_or(WebError::NoStroke)?;
        if stroke.mask.is_some() {
            return self.paint_mask_dabs(dabs);
        }
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
                    apply_image_dab(canvas, stroke, key, dab);
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
            .with_replacements(tiles.into_iter().map(|(key, canvas)| {
                let pixels = canvas.pixels_rgba8_premultiplied().to_vec();
                (key, pixels)
            }))
            .map_err(|_| WebError::InvalidPixels)?;
        if next.tiles.total_len() > MAX_RESIDENT_TILES {
            return Err(WebError::LimitExceeded);
        }
        let update = difference(&self.artwork, &next, false);
        if update.changed {
            self.artwork = next;
        }
        Ok(update)
    }
}

fn path_error(error: StrokePathError) -> WebError {
    match error {
        StrokePathError::SampleBudgetExceeded => WebError::LimitExceeded,
        StrokePathError::NonFinitePosition
        | StrokePathError::InvalidContinuity
        | StrokePathError::InvalidScale => WebError::InvalidInput,
    }
}

fn apply_image_dab(canvas: &mut CpuCanvas, stroke: &ActiveStroke, key: TileKey, dab: BrushDab) {
    let (ox, oy) = key.pixel_origin();
    let local = dab.to_local(ox, oy);
    if let Some(selection) = &stroke.selection {
        let operation = if stroke.erase {
            nyatidraw_paint_cpu::DabOperation::Erase
        } else if stroke.alpha_locked {
            nyatidraw_paint_cpu::DabOperation::Recolor(stroke.color)
        } else {
            nyatidraw_paint_cpu::DabOperation::Paint(stroke.color)
        };
        canvas.apply_dab_with_coverage(local, operation, |x, y| {
            match [ox + i64::from(x), oy + i64::from(y)].map(i32::try_from) {
                [Ok(x), Ok(y)] => selection.coverage_signed(x, y),
                _ => 0,
            }
        });
    } else if stroke.erase {
        canvas.erase_dab(local);
    } else if stroke.alpha_locked {
        canvas.apply_dab_alpha_locked(local, stroke.color);
    } else {
        canvas.apply_dab(local, stroke.color);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrected_curves_keep_atomic_cancel_undo_and_portable_pixels() {
        for scale in [0.25, 0.5, 1.0, 2.0] {
            let mut doc = WebDocument::import_rgba8(32, 32, &[0; 32 * 32 * 4]).unwrap();
            doc.select_tool(WebTool::Pen).unwrap();
            doc.set_brush_settings(crate::BrushSettings {
                smoothing: 100,
                size_px: 2.0,
                ..doc.brush_settings()
            })
            .unwrap();
            let before = doc.snapshot().clone();
            let begin = StrokePoint {
                x: 4.0,
                y: 8.0,
                pressure: 0.5,
                time_ms: 1.0,
            };
            let middle = StrokePoint {
                x: 5.0,
                y: 8.3,
                pressure: 0.7,
                time_ms: 5.0,
            };
            let end = StrokePoint {
                x: 6.0,
                y: 8.8,
                pressure: 0.8,
                time_ms: 9.0,
            };
            doc.begin_stroke_at_scale(begin, scale).unwrap();
            doc.move_stroke(middle).unwrap();
            doc.cancel_stroke();
            assert_eq!(doc.snapshot(), &before);
            assert_eq!(doc.history_len(), 0);
            doc.begin_stroke_at_scale(begin, scale).unwrap();
            doc.move_stroke(middle).unwrap();
            assert!(doc.end_stroke(StrokePoint { x: f64::NAN, ..end }).is_err());
            assert_eq!(doc.snapshot(), &before);
            assert!(!doc.is_drawing());
            doc.begin_stroke_at_scale(begin, scale).unwrap();
            doc.move_stroke(middle).unwrap();
            doc.end_stroke(end).unwrap();
            assert_eq!(doc.history_len(), 1);
            let painted = doc.export_rgba8().unwrap();
            let bytes = doc.encode_portable().unwrap();
            let reopened = WebDocument::decode_portable(&bytes).unwrap();
            assert_eq!(reopened.export_rgba8().unwrap(), painted);
            doc.undo().unwrap();
            assert_eq!(doc.snapshot(), &before);
            doc.redo().unwrap();
            assert_eq!(doc.export_rgba8().unwrap(), painted);
        }
    }

    #[test]
    fn mask_work_rejection_restores_original_and_keeps_history_usable() {
        for end in [false, true] {
            let mut doc = WebDocument::import_rgba8(16, 16, &[255; 16 * 16 * 4]).unwrap();
            doc.apply_mask_command(nyatidraw_api::MaskCommand::AddRevealAll(doc.active_layer()))
                .unwrap();
            doc.select_tool(WebTool::Pen).unwrap();
            doc.set_foreground([0, 0, 0, 255]);
            let before = doc.snapshot().clone();
            let history = doc.undo.len();
            let point = StrokePoint {
                x: 8.0,
                y: 8.0,
                pressure: 1.0,
                time_ms: 1.0,
            };
            doc.begin_stroke(point).unwrap();
            assert_ne!(doc.snapshot(), &before);
            doc.stroke.as_mut().unwrap().mask_work =
                nyatidraw_paint_cpu::MaskStrokeBudget::new(nyatidraw_paint_cpu::EditLimits {
                    max_filter_work: 0,
                    ..Default::default()
                });
            let next = StrokePoint {
                x: 24.0,
                time_ms: 2.0,
                ..point
            };
            let result = if end {
                doc.end_stroke(next)
            } else {
                doc.move_stroke(next)
            };
            assert!(matches!(result, Err(WebError::EditRejected(_))));
            assert_eq!(doc.snapshot(), &before);
            assert_eq!(doc.undo.len(), history);
            assert!(doc.stroke.is_none());
            assert!(matches!(doc.end_stroke(next), Err(WebError::NoStroke)));
            doc.begin_stroke(point).unwrap();
            assert!(doc.end_stroke(next).unwrap().committed);
            doc.undo().unwrap();
            assert_eq!(doc.snapshot(), &before);
        }
    }
}
