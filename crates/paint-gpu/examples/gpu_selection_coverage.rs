use nyatidraw_brush::{BrushDab, PencilGrain};
use nyatidraw_input::Point;
use nyatidraw_paint_cpu::{
    CpuCanvas, DabOperation, GPU_UNORM_CLOSED_STROKE_TOLERANCE, PremultipliedRgba8, SelectionMask,
};
use nyatidraw_paint_gpu::GpuRoundDabPainter;

fn error(value: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{value:?}"))
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    let coverage: Vec<u8> = (0..160 * 96)
        .map(|i| [0, 1, 63, 128, 192, 255][i % 6])
        .collect();
    let mask = SelectionMask::from_coverage_at([-16, -16], [160, 96], &coverage).map_err(error)?;
    let mut max_delta = 0;
    let mut cases = 0;
    let mut total_blend_controls = 0;
    for mode in 0..3 {
        for hardness in [0.25, 1.0] {
            for grain in [
                None,
                Some(PencilGrain {
                    deposit: 90,
                    origin: [0, 0],
                    undercoat: true,
                    continuous: false,
                }),
                Some(PencilGrain {
                    deposit: 208,
                    origin: [0, 0],
                    undercoat: false,
                    continuous: true,
                }),
            ] {
                let mut painter = match mode {
                    0 => GpuRoundDabPainter::new(&device, &queue, [0.0, 1.0, 0.5, 128.0 / 255.0]),
                    1 => GpuRoundDabPainter::new_alpha_locked(
                        &device,
                        &queue,
                        [0.0, 1.0, 0.5, 128.0 / 255.0],
                    ),
                    _ => GpuRoundDabPainter::new_eraser(&device, &queue),
                };
                let selection = painter
                    .create_selection_coverage_at(
                        mask.origin(),
                        mask.dimensions(),
                        mask.coverage_bytes(),
                    )
                    .map_err(error)?;
                painter.set_selection_mask(Some(&selection));
                let before: Vec<u8> = (0..128 * 64)
                    .flat_map(|i| {
                        let alpha = [0, 1, 64, 128, 255][i % 5];
                        [alpha / 2, alpha / 3, 0, alpha]
                    })
                    .collect();
                let tile = painter
                    .restore_closed_rgba8(128, 64, &before)
                    .map_err(error)?;
                let mut cpu =
                    CpuCanvas::from_rgba8_premultiplied(128, 64, before.clone()).map_err(error)?;
                let color = PremultipliedRgba8([0, 128, 64, 128]);
                let operation = match mode {
                    0 => DabOperation::Paint(color),
                    1 => DabOperation::Recolor(color),
                    _ => DabOperation::Erase,
                };
                let dabs: Vec<_> = (0..12)
                    .map(|index| BrushDab {
                        center: Point {
                            x: 20.0 + f64::from(index) * 4.0,
                            y: 30.0 + f64::from(index % 3) * 3.0,
                        },
                        radius_px: 11.5,
                        opacity: 0.65,
                        flow: 0.55,
                        hardness,
                        grain,
                    })
                    .collect();
                painter.apply(&tile, &dabs);
                for &dab in &dabs {
                    cpu.apply_dab_with_coverage(dab, operation, |x, y| {
                        mask.coverage_signed(i32::try_from(x).unwrap(), i32::try_from(y).unwrap())
                    });
                }
                let actual = tile.readback_rgba8(&device, &queue).map_err(error)?;
                painter.set_selection_mask(None);
                let baseline = painter
                    .restore_closed_rgba8(128, 64, &before)
                    .map_err(error)?;
                painter.apply(&baseline, &dabs);
                let unselected_readback =
                    baseline.readback_rgba8(&device, &queue).map_err(error)?;
                let mut baseline_cpu =
                    CpuCanvas::from_rgba8_premultiplied(128, 64, before.clone()).map_err(error)?;
                for &dab in &dabs {
                    baseline_cpu.apply_dab_with_coverage(dab, operation, |_, _| 255);
                }
                let baseline_delta = baseline_cpu
                    .pixels_rgba8_premultiplied()
                    .iter()
                    .zip(&unselected_readback.pixels)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap_or(0);
                let mut changed = 0;
                let mut case_delta = 0;
                let mut outliers = Vec::new();
                for (index, (expected, observed)) in cpu
                    .pixels_rgba8_premultiplied()
                    .chunks_exact(4)
                    .zip(actual.pixels.chunks_exact(4))
                    .enumerate()
                {
                    let original = &before[index * 4..][..4];
                    let coverage = mask
                        .coverage_signed(i32::try_from(index % 128)?, i32::try_from(index / 128)?);
                    if coverage == 0 && original != observed {
                        return Err("GPU escaped zero coverage".into());
                    }
                    if mode == 1 && original[3] != observed[3] {
                        return Err("Recolor changed alpha".into());
                    }
                    if observed[..3].iter().any(|value| *value > observed[3]) {
                        return Err("Not premultiplied".into());
                    }
                    let delta = expected
                        .iter()
                        .zip(observed)
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap_or(0);
                    case_delta = case_delta.max(delta);
                    if delta > GPU_UNORM_CLOSED_STROKE_TOLERANCE.max_channel_delta {
                        if grain.is_none() || outliers.len() == 256 {
                            return Err("Unexplained blend error or control budget exceeded".into());
                        }
                        outliers.push((index, delta));
                    }
                    changed += usize::from(original != observed);
                }
                max_delta = max_delta.max(case_delta);
                for &(index, delta) in &outliers {
                    let control = coverage_blend_control(
                        &device, &queue, &dabs, &before, &mask, index, mode,
                    )?;
                    if control.cpu != cpu.pixels_rgba8_premultiplied()[index * 4..][..4]
                        || control.gpu != actual.pixels[index * 4..][..4]
                        || usize::from(delta) > control.contributing
                    {
                        return Err(format!("Coverage differs from blend-only control: mode={mode} pixel={index} delta={delta} control={control:?}").into());
                    }
                }
                total_blend_controls += outliers.len();
                println!(
                    "mode={mode} hardness={hardness} pencil={} continuous={} changes={changed} selected_delta={case_delta} unselected_baseline_delta={baseline_delta} verified_blend_outliers={}",
                    grain.is_some(),
                    grain.is_some_and(|grain| grain.continuous),
                    outliers.len()
                );
                if changed == 0 {
                    return Err("Expected artwork change is absent".into());
                }
                cases += 1;
            }
        }
    }
    println!(
        "adapter={:?} backend={:?} release={} cases={} max_delta={} verified_blend_outliers={total_blend_controls} global_tolerance_passed={} physical_pen_proof=false coverage_contract=PASS",
        info.name,
        info.backend,
        !cfg!(debug_assertions),
        cases,
        max_delta,
        max_delta <= GPU_UNORM_CLOSED_STROKE_TOLERANCE.max_channel_delta
    );
    Ok(())
}

#[derive(Debug)]
struct BlendControl {
    cpu: Vec<u8>,
    gpu: Vec<u8>,
    contributing: usize,
}

fn coverage_blend_control(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dabs: &[BrushDab],
    before: &[u8],
    mask: &SelectionMask,
    index: usize,
    mode: u8,
) -> Result<BlendControl, Box<dyn std::error::Error>> {
    let x = u32::try_from(index % 128)?;
    let y = u32::try_from(index / 128)?;
    let selected = mask.coverage_signed(i32::try_from(x)?, i32::try_from(y)?);
    let mut full_cover = Vec::new();
    for &dab in dabs {
        let units = nyatidraw_brush::pencil_coverage_units(dab, x, y);
        #[allow(clippy::cast_precision_loss)]
        let coverage = units as f32 / 4080.0;
        let minimum = if units != 0 && dab.grain.is_some_and(|grain| grain.undercoat) {
            1.0
        } else {
            0.0
        };
        let pigment = (coverage * (dab.opacity * dab.flow).clamp(0.0, 1.0) * 255.0)
            .round()
            .max(minimum);
        let opacity = (pigment * f32::from(selected) / 255.0).round() / 255.0;
        full_cover.push(BrushDab {
            center: Point { x: 0.5, y: 0.5 },
            radius_px: 2.0,
            opacity,
            flow: 1.0,
            hardness: 1.0,
            grain: None,
        });
    }
    let initial = before[index * 4..][..4].to_vec();
    let mut cpu = CpuCanvas::from_rgba8_premultiplied(1, 1, initial.clone()).map_err(error)?;
    let color = PremultipliedRgba8([0, 128, 64, 128]);
    let operation = match mode {
        0 => DabOperation::Paint(color),
        1 => DabOperation::Recolor(color),
        _ => DabOperation::Erase,
    };
    for &dab in &full_cover {
        cpu.apply_dab_with_coverage(dab, operation, |_, _| 255);
    }
    let mut painter = match mode {
        0 => GpuRoundDabPainter::new(device, queue, [0.0, 1.0, 0.5, 128.0 / 255.0]),
        1 => GpuRoundDabPainter::new_alpha_locked(device, queue, [0.0, 1.0, 0.5, 128.0 / 255.0]),
        _ => GpuRoundDabPainter::new_eraser(device, queue),
    };
    let tile = painter
        .restore_closed_rgba8(1, 1, &initial)
        .map_err(error)?;
    painter.apply(&tile, &full_cover);
    let gpu = tile.readback_rgba8(device, queue).map_err(error)?;
    Ok(BlendControl {
        cpu: cpu.pixels_rgba8_premultiplied().to_vec(),
        gpu: gpu.pixels,
        contributing: full_cover.iter().filter(|dab| dab.opacity > 0.0).count(),
    })
}
