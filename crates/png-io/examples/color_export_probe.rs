//! Release CPU/file-encoding cost and exact import of a fixed 4K color scene.
//! No desktop, export worker, fsync, GPU, pen or visible-pixel timing claim.
use nyatidraw_api::LayerId;
use nyatidraw_png_io::{decode_png, encode_png, encode_png_bytes};
use nyatidraw_tiles::FlattenedRgba8;
use std::{fs, path::Path, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("measure this probe with --release".into());
    }
    let args: Vec<_> = std::env::args().collect();
    let directory = Path::new(args.get(1).ok_or("fresh output directory required")?);
    if directory.exists() {
        return Err("refusing an existing output directory".into());
    }
    fs::create_dir_all(directory)?;
    let mut pixels = Vec::with_capacity(3840 * 2160 * 4);
    for y in 0..2160_u32 {
        for x in 0..3840_u32 {
            let alpha = (y % 255) + 1;
            pixels.extend_from_slice(&[
                u8::try_from(x % (alpha + 1))?,
                u8::try_from((x + y) % (alpha + 1))?,
                u8::try_from((x * 3 + y * 5) % (alpha + 1))?,
                u8::try_from(alpha)?,
            ]);
        }
    }
    pixels[..4].copy_from_slice(&[128, 64, 32, 128]);
    let surface = FlattenedRgba8 {
        origin_x: 0,
        origin_y: 0,
        width: 3840,
        height: 2160,
        pixels,
    };
    let path = directory.join("color-export-probe.png");
    encode_png(&path, &surface)?; // Explicit single warmup, including lookup init.
    let mut times = Vec::with_capacity(20);
    for _ in 0..20 {
        let start = Instant::now();
        encode_png(&path, &surface)?;
        times.push(start.elapsed().as_micros());
    }
    let imported = decode_png(&path, LayerId(1))?;
    let restored = imported
        .tiles
        .crop_base_layer_rgba8_to_canvas(LayerId(1), imported.canvas)
        .map_err(|error| format!("crop: {error:?}"))?;
    if restored.pixels != surface.pixels {
        return Err("4K color transport changed premultiplied artwork bytes".into());
    }
    let preview = FlattenedRgba8 {
        origin_x: 0,
        origin_y: 0,
        width: 1,
        height: 1,
        pixels: surface.pixels[..4].to_vec(),
    };
    fs::write(directory.join("preview.png"), encode_png_bytes(&preview)?)?;
    println!(
        "color-export-probe profile=release-required canvas=3840x2160 samples=20 warmup=1 file_bytes={} raw_us={times:?} import=all-pixels-exact input=none timing=encode-file-close-no-fsync",
        fs::metadata(&path)?.len()
    );
    times.sort_unstable();
    println!(
        "color-export-percentiles method=nearest-rank p50_us={} p95_us={} p99_us={} conversion_table_bytes=131072 encoded_row_bytes=30720",
        times[9], times[18], times[19]
    );
    Ok(())
}
