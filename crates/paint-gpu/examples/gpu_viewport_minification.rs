//! Synthetic real-device acceptance: zooming out must not invent/remove fine ink
//! or mutate base artwork. No project files, window, or physical input are used.
//! Run: `cargo run -p nyatidraw-paint-gpu --example gpu_viewport_minification --release --locked`

use nyatidraw_api::{ContentRootId, GroupId, LayerBlendMode, LayerId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_input::{Point, ViewportTransform};
use nyatidraw_paint_gpu::GpuCompositeScene;
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey};

type ProbeResult<T> = Result<T, Box<dyn std::error::Error>>;
const PAGE: [u32; 2] = [1_025, 769];
const DISPLAY: [u32; 2] = [256, 192];
const LAYER: LayerId = LayerId(1);
const MAX_ERROR: f64 = 8.0;

#[derive(Clone, Copy, Debug)]
enum Pattern {
    Checker,
    FineLines,
    Grain,
}

impl Pattern {
    fn gray(self, x: u32, y: u32) -> u8 {
        match self {
            Self::Checker => {
                if (x + y).is_multiple_of(2) {
                    0
                } else {
                    255
                }
            }
            Self::FineLines => {
                if x.is_multiple_of(4) {
                    0
                } else {
                    255
                }
            }
            Self::Grain => {
                let permutation = ((x % 4 + (y % 4) * 4) * 7) % 16;
                u8::try_from(32 + permutation * 12).expect("bounded synthetic grain")
            }
        }
    }

    const fn mean(self) -> f64 {
        match self {
            Self::Checker => 127.5,
            Self::FineLines => 191.25,
            Self::Grain => 122.0,
        }
    }

    fn tile(self) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(TILE_BYTE_LEN);
        for y in 0..TILE_EDGE {
            for x in 0..TILE_EDGE {
                let gray = self.gray(x, y);
                pixels.extend_from_slice(&[gray, gray, gray, 255]);
            }
        }
        pixels
    }
}

fn main() -> ProbeResult<()> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nyati-viewport-minification-probe"),
        ..Default::default()
    }))?;
    let keys: Vec<_> = (0..7)
        .flat_map(|y| {
            (-2..9).map(move |x| TileKey {
                layer: LAYER,
                mip: 0,
                x,
                y,
            })
        })
        .collect();
    let mut passed = true;
    for pattern in [Pattern::Checker, Pattern::FineLines, Pattern::Grain] {
        let source = pattern.tile();
        let mut scene =
            GpuCompositeScene::new(&device, &queue, PAGE, tree()?).map_err(debug_error)?;
        for key in &keys {
            scene
                .upload_closed_tile(*key, &source)
                .map_err(debug_error)?;
        }
        let all_tiles = ViewportTransform {
            physical_size: [1_408, 896],
            pan: Point { x: 256.0, y: 0.0 },
            ..viewport(1.0, 0.0, Point::default(), false)
        };
        scene.render_viewport(all_tiles).map_err(debug_error)?;
        require_base_pixels(&scene, &keys, &source)?;
        require_unscaled_pixels(&mut scene, pattern)?;
        for (zoom, rotation, offset) in [
            (0.25, 0.0, Point::default()),
            (0.125, 0.0, Point::default()),
            (0.19, 0.0, Point { x: 0.37, y: -0.21 }),
            (0.19, 0.31, Point { x: -0.43, y: 0.29 }),
        ] {
            for sparse in [false, true] {
                let view = viewport(zoom, rotation, offset, sparse);
                scene.render_viewport(view).map_err(debug_error)?;
                let pixels = scene
                    .readback_display()
                    .map_err(debug_error)?
                    .ok_or("missing display")?
                    .pixels;
                passed &= report_quality(&pixels, view, pattern, sparse)?;
            }
        }
        require_base_pixels(&scene, &keys, &source)?;
    }
    require_hidden_update(&device, &queue)?;
    require_premultiplied_edges(&device, &queue)?;
    require_odd_page_tail(&device, &queue)?;
    println!(
        "{{\"event\":\"gpu_viewport_minification\",\"adapter\":{:?},\"backend\":\"{:?}\",\"os\":{:?},\"release_profile\":{},\"base_tiles_unchanged\":true,\"unscaled_exact\":true,\"hidden_update\":true,\"premultiplied_edges\":true,\"odd_page_tail\":true,\"pass\":{passed}}}",
        info.name,
        info.backend,
        std::env::consts::OS,
        !cfg!(debug_assertions),
    );
    if !passed {
        return Err("minification changed the apparent coverage of synthetic fine ink".into());
    }
    Ok(())
}

fn require_unscaled_pixels(scene: &mut GpuCompositeScene, pattern: Pattern) -> ProbeResult<()> {
    for zoom in [1.0, 3.0] {
        for (x, y) in [(257, 385), (262, 389), (271, 394)] {
            let point = Point {
                x: f64::from(x) + 0.5,
                y: f64::from(y) + 0.5,
            };
            let gray = pattern.gray(x, y);
            require_rgba(
                render_center(scene, zoom, point)?,
                [gray, gray, gray, 255],
                0,
                "1:1/enlargement must retain the exact source pixel",
            )?;
        }
    }
    Ok(())
}

fn require_hidden_update(device: &wgpu::Device, queue: &wgpu::Queue) -> ProbeResult<()> {
    let old_color = [17, 91, 33, 255];
    let new_color = [193, 29, 11, 255];
    let mut scene = solid_scene(device, queue, old_color)?;
    require_rgba(
        render_center(&mut scene, 2.0, Point { x: 64.5, y: 64.5 })?,
        old_color,
        0,
        "initial visible page tile",
    )?;
    let key = TileKey {
        layer: LAYER,
        mip: 0,
        x: 6,
        y: 4,
    };
    let source = new_color.repeat(TILE_BYTE_LEN / 4);
    scene
        .upload_closed_tile(key, &source)
        .map_err(debug_error)?;
    for zoom in [0.125, 0.25] {
        require_rgba(
            render_center(&mut scene, zoom, Point { x: 832.0, y: 576.0 })?,
            new_color,
            2,
            "hidden edit must invalidate the root and every display mip",
        )?;
    }
    require_rgba(
        render_center(&mut scene, 1.0, Point { x: 64.5, y: 64.5 })?,
        old_color,
        0,
        "hidden edit must not change unrelated source pixels",
    )?;
    require_base_pixels(&scene, &[key], &source)?;
    println!("{{\"event\":\"minification_hidden_update\",\"pass\":true}}");
    Ok(())
}

fn require_premultiplied_edges(device: &wgpu::Device, queue: &wgpu::Queue) -> ProbeResult<()> {
    let mut scene = solid_scene(device, queue, [0; 4])?;
    let mut source = Vec::with_capacity(TILE_BYTE_LEN);
    for _ in 0..TILE_EDGE {
        for x in 0..TILE_EDGE {
            source.extend_from_slice(if x % 4 < 2 {
                &[128, 0, 0, 128]
            } else {
                &[0; 4]
            });
        }
    }
    let key = TileKey {
        layer: LAYER,
        mip: 0,
        x: 3,
        y: 3,
    };
    scene
        .upload_closed_tile(key, &source)
        .map_err(debug_error)?;
    for zoom in [0.25, 0.125, 0.19] {
        for point in [Point { x: 424.0, y: 424.0 }, Point { x: 440.0, y: 424.0 }] {
            require_rgba(
                render_center(&mut scene, zoom, point)?,
                over_checker([64, 0, 0, 64], point),
                3,
                "transparent red edges must average premultiplied color and alpha together",
            )?;
        }
    }
    require_base_pixels(&scene, &[key], &source)?;
    println!("{{\"event\":\"minification_premultiplied_edges\",\"pass\":true}}");
    Ok(())
}

fn require_odd_page_tail(device: &wgpu::Device, queue: &wgpu::Queue) -> ProbeResult<()> {
    let source = [37, 61, 89, 128];
    let mut scene = solid_scene(device, queue, source)?;
    for zoom in [1.0, 0.25, 0.125, 0.19] {
        for point in [
            Point {
                x: 1_024.5,
                y: 384.5,
            },
            Point { x: 512.5, y: 768.5 },
            Point {
                x: 1_024.5,
                y: 768.5,
            },
        ] {
            require_rgba(
                render_center(&mut scene, zoom, point)?,
                over_checker(source, point),
                2,
                "odd final row/column must keep constant premultiplied coverage",
            )?;
        }
    }
    println!("{{\"event\":\"minification_odd_page_tail\",\"pass\":true}}");
    Ok(())
}

fn solid_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    color: [u8; 4],
) -> ProbeResult<GpuCompositeScene> {
    let mut scene = GpuCompositeScene::new(device, queue, PAGE, tree()?).map_err(debug_error)?;
    let count = usize::try_from(PAGE[0] * PAGE[1])?;
    scene
        .upload_layer_rgba8(LAYER, &color.repeat(count))
        .map_err(debug_error)?;
    Ok(scene)
}

fn render_center(scene: &mut GpuCompositeScene, zoom: f64, point: Point) -> ProbeResult<[u8; 4]> {
    let view = ViewportTransform {
        pan: Point {
            x: 128.5 - point.x * zoom,
            y: 96.5 - point.y * zoom,
        },
        ..viewport(zoom, 0.0, Point::default(), false)
    };
    scene.render_viewport(view).map_err(debug_error)?;
    let readback = scene
        .readback_display()
        .map_err(debug_error)?
        .ok_or("missing display")?;
    let index = usize::try_from((96 * DISPLAY[0] + 128) * 4)?;
    Ok(readback.pixels[index..index + 4].try_into()?)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn over_checker(source: [u8; 4], point: Point) -> [u8; 4] {
    let light = ((point.x / 16.0).floor() + (point.y / 16.0).floor()).rem_euclid(2.0) < 1.0;
    let backdrop = if light { 0.72 } else { 0.58 };
    let mut result = [255; 4];
    for channel in 0..3 {
        result[channel] =
            (f64::from(source[channel]) + backdrop * f64::from(255 - source[3])).round() as u8;
    }
    result
}

fn require_rgba(actual: [u8; 4], expected: [u8; 4], tolerance: u8, label: &str) -> ProbeResult<()> {
    if actual
        .into_iter()
        .zip(expected)
        .any(|(actual, expected)| actual.abs_diff(expected) > tolerance)
    {
        return Err(format!("{label}: expected {expected:?}, got {actual:?}").into());
    }
    Ok(())
}

fn require_base_pixels(
    scene: &GpuCompositeScene,
    keys: &[TileKey],
    source: &[u8],
) -> ProbeResult<()> {
    for key in keys {
        let actual = scene
            .readback_resident_raster_tile(*key)
            .map_err(debug_error)?
            .ok_or_else(|| format!("missing authoritative base tile {key:?}"))?;
        if actual.pixels != source {
            return Err(format!("base artwork changed at {key:?}").into());
        }
    }
    Ok(())
}

fn viewport(zoom: f64, rotation: f64, offset: Point, sparse: bool) -> ViewportTransform {
    let anchor = Point {
        x: if sparse { -128.0 } else { 512.0 },
        y: 384.0,
    };
    let (sin, cos) = rotation.sin_cos();
    ViewportTransform {
        revision: 1,
        window_origin_physical: Point::default(),
        physical_size: DISPLAY,
        dpi_scale: 1.0,
        pan: Point {
            x: f64::from(DISPLAY[0]) / 2.0 - zoom * (cos * anchor.x - sin * anchor.y) + offset.x,
            y: f64::from(DISPLAY[1]) / 2.0 - zoom * (sin * anchor.x + cos * anchor.y) + offset.y,
        },
        zoom,
        rotation_radians: rotation,
        mirrored_horizontal: false,
    }
}

fn report_quality(
    pixels: &[u8],
    view: ViewportTransform,
    pattern: Pattern,
    sparse: bool,
) -> ProbeResult<bool> {
    let mut count = 0_u32;
    let mut sum = 0.0_f64;
    let mut max_error = 0.0_f64;
    let mut seam_max_error = 0.0_f64;
    let mut seam_count = 0_u32;
    for y in 0..DISPLAY[1] {
        for x in 0..DISPLAY[0] {
            let point = view
                .window_to_document(Point {
                    x: f64::from(x) + 0.5,
                    y: f64::from(y) + 0.5,
                })
                .ok_or("invalid synthetic viewport")?;
            let inside = if sparse {
                point.x > -240.0 && point.x < -16.0
            } else {
                point.x > 16.0 && point.x < f64::from(PAGE[0]) - 16.0
            };
            if !inside || point.y <= 16.0 || point.y >= f64::from(PAGE[1]) - 16.0 {
                continue;
            }
            let index = usize::try_from((y * DISPLAY[0] + x) * 4)?;
            let error = pixels[index..index + 3]
                .iter()
                .map(|channel| (f64::from(*channel) - pattern.mean()).abs())
                .fold(0.0_f64, f64::max);
            if pixels[index + 3] != 255 {
                return Err("opaque synthetic display lost alpha".into());
            }
            count += 1;
            sum += error;
            max_error = max_error.max(error);
            if sparse && (point.x + 128.0).abs() <= 8.0 {
                seam_count += 1;
                seam_max_error = seam_max_error.max(error);
            }
        }
    }
    if count < 64 || (sparse && seam_count == 0) {
        return Err("synthetic viewport did not cover the required artwork/seam".into());
    }
    let passed = max_error <= MAX_ERROR;
    println!(
        "{{\"event\":\"minification_case\",\"pattern\":\"{pattern:?}\",\"sparse\":{sparse},\"zoom\":{},\"rotation\":{},\"pixels\":{count},\"mean_error\":{:.3},\"max_error\":{max_error:.3},\"seam_pixels\":{seam_count},\"seam_max_error\":{seam_max_error:.3},\"pass\":{passed}}}",
        view.zoom,
        view.rotation_radians,
        sum / f64::from(count),
    );
    Ok(passed)
}

fn tree() -> ProbeResult<LayerTree> {
    LayerTree::new(GroupNode {
        id: GroupId(1),
        name: "Synthetic root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        children: vec![LayerTreeNode::Raster(LayerNode {
            id: LAYER,
            name: "Synthetic ink".into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            color_effect: None,
        })],
    })
    .map_err(|error| debug_error(error).into())
}

fn debug_error(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{error:?}"))
}
