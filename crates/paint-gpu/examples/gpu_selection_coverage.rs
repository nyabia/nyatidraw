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
    for mode in 0..3 {
        for hardness in [0.25, 1.0] {
            for grain in [
                None,
                Some(PencilGrain {
                    deposit: 90,
                    origin: [0, 0],
                    undercoat: true,
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
                    for (a, b) in expected.iter().zip(observed) {
                        case_delta = case_delta.max(a.abs_diff(*b));
                    }
                    changed += usize::from(original != observed);
                }
                max_delta = max_delta.max(case_delta);
                println!(
                    "mode={mode} hardness={hardness} pencil={} changes={changed} selected_delta={case_delta} unselected_baseline_delta={baseline_delta}",
                    grain.is_some()
                );
                let limit = GPU_UNORM_CLOSED_STROKE_TOLERANCE
                    .max_channel_delta
                    .max(baseline_delta.saturating_add(1));
                if changed == 0 || case_delta > limit || case_delta > u8::try_from(dabs.len())? {
                    return Err(format!("Coverage parity regression: changes={changed} delta={case_delta} baseline={baseline_delta}").into());
                }
                cases += 1;
            }
        }
    }
    println!(
        "adapter={:?} backend={:?} release={} cases={} max_delta={} physical_pen_proof=false PASS",
        info.name,
        info.backend,
        !cfg!(debug_assertions),
        cases,
        max_delta
    );
    Ok(())
}
