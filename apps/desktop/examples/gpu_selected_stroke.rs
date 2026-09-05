//! Real GPU/CPU selected stroke comparison, including sparse padding and Cancel.
use nyatidraw_api::{ContentRootId, GroupId, LayerId, SnapshotId};
use nyatidraw_brush::{
    BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot, ROUND_BRUSH_ENGINE_VERSION,
    RoundBrushEvaluator, begin_round_stroke,
};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_paint_gpu::{GpuCompositeScene, GpuRoundDabPainter, LiveStrokeDisposition};
use nyatidraw_stroke::{
    MaterializationStrategy, StrokeColor, StrokeCommit, StrokeSelection, materialize_with_strategy,
};
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::sync::Arc;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const SIZE: u32 = 129;

fn debug_error(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{error:?}"))
}

fn tree() -> LayerTree {
    LayerTree::new(GroupNode {
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![LayerTreeNode::Raster(LayerNode {
            id: LayerId(1),
            name: "Ink".into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })],
    })
    .unwrap()
}

fn selection(empty: bool) -> Arc<StrokeSelection> {
    let mut bits = vec![0; (129_usize * 129).div_ceil(8)];
    if !empty {
        for y in 17..129 {
            for x in 62..129 {
                if (x + y) % 7 == 0 {
                    continue;
                }
                let index = y * 129 + x;
                bits[index / 8] |= 1 << (index % 8);
            }
        }
    }
    Arc::new(StrokeSelection::from_packed_bits(SIZE, SIZE, &bits).unwrap())
}

fn compare(
    scene: &GpuCompositeScene,
    expected: &TileSnapshot,
    selection: Option<&StrokeSelection>,
    tolerance: u8,
) -> Result<u8> {
    let mut maximum = 0;
    for (key, tile) in expected.iter() {
        let actual = scene
            .readback_resident_raster_tile(key)
            .map_err(debug_error)?
            .ok_or_else(|| format!("tile not resident: {key:?}"))?;
        if actual.width != 128 || actual.height != 128 {
            return Err("partial diagnostic tile".into());
        }
        let (origin_x, origin_y) = key.pixel_origin();
        for (index, (actual, expected)) in actual
            .pixels
            .chunks_exact(4)
            .zip(tile.pixels().chunks_exact(4))
            .enumerate()
        {
            let x = origin_x + i64::try_from(index % 128)?;
            let y = origin_y + i64::try_from(index / 128)?;
            let selected =
                selection.is_none_or(|mask| match (u32::try_from(x), u32::try_from(y)) {
                    (Ok(x), Ok(y)) => mask.contains(x, y),
                    _ => false,
                });
            for (actual, expected) in actual.iter().zip(expected) {
                let delta = actual.abs_diff(*expected);
                maximum = maximum.max(delta);
                if delta > if selected { tolerance } else { 0 } {
                    return Err(format!("GPU pixel drift key={key:?} ({x},{y}) actual={actual} expected={expected} selected={selected} delta={delta}").into());
                }
            }
        }
    }
    Ok(maximum)
}

#[allow(clippy::cast_possible_truncation)]
fn verify_guide(scene: &mut GpuCompositeScene, before: &TileSnapshot) -> Result<()> {
    let viewport = nyatidraw_input::ViewportTransform {
        revision: 3,
        window_origin_physical: Point {
            x: -900.0,
            y: 100.0,
        },
        physical_size: [384, 384],
        dpi_scale: 1.5,
        pan: Point { x: 80.0, y: 80.0 },
        zoom: 0.8,
        rotation_radians: 0.3,
    };
    scene.render_viewport(viewport).map_err(debug_error)?;
    let baseline = scene
        .readback_display()
        .map_err(debug_error)?
        .ok_or("display missing")?;
    for closed in [false, true] {
        if !scene.set_gesture_preview(&[[20, 20], [100, 20], [60, 80]], closed) {
            return Err("guide rejected".into());
        }
        scene.render_viewport(viewport).map_err(debug_error)?;
        let shown = scene
            .readback_display()
            .map_err(debug_error)?
            .ok_or("display missing")?;
        if shown.pixels == baseline.pixels {
            return Err("guide invisible".into());
        }
        let midpoint = viewport
            .document_to_window(Point { x: 60.0, y: 20.0 })
            .ok_or("invalid projection")?;
        let center = [
            (midpoint.x - viewport.window_origin_physical.x).floor() as i32,
            (midpoint.y - viewport.window_origin_physical.y).floor() as i32,
        ];
        let mut cyan = false;
        for y in center[1] - 2..=center[1] + 2 {
            for x in center[0] - 2..=center[0] + 2 {
                let offset = (usize::try_from(y)? * 384 + usize::try_from(x)?) * 4;
                let pixel = &shown.pixels[offset..offset + 4];
                cyan |= pixel[0] < 40 && pixel[1] > 220 && pixel[2] == 255;
            }
        }
        if !cyan {
            return Err("guide projected to the wrong location".into());
        }
        compare(scene, before, None, 0)?;
        scene.set_gesture_preview(&[], false);
        scene.render_viewport(viewport).map_err(debug_error)?;
        if scene
            .readback_display()
            .map_err(debug_error)?
            .ok_or("display")?
            .pixels
            != baseline.pixels
        {
            return Err("guide did not clear exactly".into());
        }
    }
    if scene.set_gesture_preview(&vec![[0, 0]; 4097], true) {
        return Err("unbounded guide accepted".into());
    }
    println!("gpu-gesture-preview location=exact clear=exact artwork=exact bounded=true");
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let backend = match std::env::args().nth(1).as_deref() {
        Some("dx12") => wgpu::Backends::DX12,
        Some("vulkan") => wgpu::Backends::VULKAN,
        _ => return Err("specify dx12 or vulkan".into()),
    };
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: backend,
        ..Default::default()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    let before = TileSnapshot::from_tiles((-1..=1).flat_map(|y| {
        (-1..=1).map(move |x| {
            (
                TileKey {
                    layer: LayerId(1),
                    mip: 0,
                    x,
                    y,
                },
                [80, 60, 40, 160].repeat(TILE_BYTE_LEN / 4),
            )
        })
    }))
    .map_err(debug_error)?;
    let preset = BrushPreset {
        id: BrushPresetId(1),
        schema_version: 1,
        engine_version: ROUND_BRUSH_ENGINE_VERSION,
        size_px: 40.0,
        opacity: 0.8,
        flow: 0.5,
        spacing_ratio: 0.25,
    };
    for eraser in [false, true] {
        for empty in [false, true] {
            let mask = selection(empty);
            let mut scene = GpuCompositeScene::new(&device, &queue, [SIZE, SIZE], tree())
                .map_err(debug_error)?;
            for (key, tile) in before.iter() {
                scene
                    .upload_closed_tile(key, tile.pixels())
                    .map_err(debug_error)?;
            }
            // Sparse closed tiles enter the GPU atlas when their viewport is visible.
            scene
                .render_viewport(nyatidraw_input::ViewportTransform {
                    revision: 1,
                    window_origin_physical: Point::default(),
                    physical_size: [384, 384],
                    dpi_scale: 1.0,
                    pan: Point { x: 128.0, y: 128.0 },
                    zoom: 1.0,
                    rotation_radians: 0.0,
                })
                .map_err(debug_error)?;
            let mut painter = if eraser {
                GpuRoundDabPainter::new_eraser(&device, &queue)
            } else {
                GpuRoundDabPainter::new(&device, &queue, [0.75, 0.375, 0.1875, 32.0 / 255.0])
            };
            let gpu_mask = painter
                .create_selection_mask(mask.dimensions(), &mask.packed_bits())
                .map_err(debug_error)?;
            painter.set_selection_mask(Some(&gpu_mask));
            scene.set_selection_overlay(Some(&gpu_mask));
            // Display chrome must compile on this backend and leave all artwork
            // surfaces byte-identical, including after viewport rotation.
            scene
                .render_viewport(nyatidraw_input::ViewportTransform {
                    revision: 2,
                    window_origin_physical: Point::default(),
                    physical_size: [384, 384],
                    dpi_scale: 1.5,
                    pan: Point { x: 80.0, y: 80.0 },
                    zoom: 0.8,
                    rotation_radians: 0.3,
                })
                .map_err(debug_error)?;
            compare(&scene, &before, None, 0)?;
            verify_guide(&mut scene, &before)?;
            let samples = [
                (PointerPhase::Begin, -8.0, 5.0, 0.4),
                (PointerPhase::Move, 60.0, 75.0, 0.7),
                (PointerPhase::End, 140.0, 127.0, 1.0),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (phase, x, y, pressure))| {
                let sequence = u64::try_from(index).unwrap() + 1;
                StylusSample {
                    sequence,
                    timestamp_ns: sequence * 1_000,
                    device_id: 1,
                    phase,
                    position_document: Point { x, y },
                    pressure,
                    tilt: None,
                    twist_radians: None,
                    tangential_pressure: None,
                    buttons: PenButtons::default(),
                    eraser,
                    viewport_revision: 1,
                }
            })
            .collect::<Vec<_>>();
            let mut evaluator = RoundBrushEvaluator::new(1);
            let mut dabs = Vec::new();
            let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
            evaluator.push(&mut token, &samples[1..], &mut dabs);
            let recorded = evaluator.end(token, &mut dabs);
            let seal = |selection| {
                StrokeCommit::seal_with_selection(
                    SnapshotId(1),
                    LayerId(1),
                    &before,
                    BrushSnapshot { preset },
                    recorded.clone(),
                    StrokeColor([24, 12, 6, 32]),
                    samples.clone(),
                    selection,
                )
                .map_err(debug_error)
            };
            let expected = materialize_with_strategy(
                MaterializationStrategy::CpuReplay,
                &seal(Some(mask.clone()))?,
                &before,
            )
            .map_err(debug_error)?
            .after;
            let mut maximum = 0;
            for disposition in [LiveStrokeDisposition::Cancel, LiveStrokeDisposition::Commit] {
                let token = scene.begin_live_stroke(LayerId(1)).map_err(debug_error)?;
                for chunk in dabs.chunks(5) {
                    scene
                        .apply_live_dabs(&mut painter, token, chunk)
                        .map_err(debug_error)?;
                }
                maximum = maximum.max(compare(&scene, &expected, Some(&mask), 2)?);
                scene
                    .finish_live_stroke(token, disposition)
                    .map_err(debug_error)?;
                if disposition == LiveStrokeDisposition::Cancel {
                    compare(&scene, &before, None, 0)?;
                }
            }
            // A new preview's Cancel must preserve the earlier committed GPU preview.
            let saved: Vec<_> = before
                .iter()
                .map(|(key, _)| {
                    Ok((
                        key,
                        scene
                            .readback_resident_raster_tile(key)
                            .map_err(debug_error)?
                            .ok_or("resident tile")?
                            .pixels,
                    ))
                })
                .collect::<Result<_>>()?;
            let saved = TileSnapshot::from_tiles(saved).map_err(debug_error)?;
            let token = scene.begin_live_stroke(LayerId(1)).map_err(debug_error)?;
            scene
                .apply_live_dabs(&mut painter, token, &dabs)
                .map_err(debug_error)?;
            scene
                .finish_live_stroke(token, LiveStrokeDisposition::Cancel)
                .map_err(debug_error)?;
            compare(&scene, &saved, None, 0)?;
            for (key, tile) in before.iter() {
                scene
                    .upload_closed_tile(key, tile.pixels())
                    .map_err(debug_error)?;
            }
            painter.set_selection_mask(None);
            let all = materialize_with_strategy(
                MaterializationStrategy::CpuReplay,
                &seal(None)?,
                &before,
            )
            .map_err(debug_error)?
            .after;
            let token = scene.begin_live_stroke(LayerId(1)).map_err(debug_error)?;
            scene
                .apply_live_dabs(&mut painter, token, &dabs)
                .map_err(debug_error)?;
            compare(&scene, &all, None, 2)?;
            scene
                .finish_live_stroke(token, LiveStrokeDisposition::Cancel)
                .map_err(debug_error)?;
            println!(
                "gpu-selected-stroke adapter={:?} backend={:?} eraser={eraser} empty={empty} max_delta={maximum} outside=exact cancel=exact clear=passed",
                info.name, info.backend
            );
        }
    }
    Ok(())
}
