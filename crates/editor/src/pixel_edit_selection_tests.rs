use super::*;
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey};

#[test]
fn select_all_fill_domain_ignores_tile_padding_but_preserves_signed_artwork() {
    let canvas = CanvasSpec {
        width_px: 32,
        height_px: 32,
        pixels_per_inch: 96,
    };
    let target = LayerId(1);
    let limits = EditLimits::default();
    for (point, expected) in [
        (Some([31, 31]), ([0, 0], [32, 32])),
        (Some([-129, -1]), ([-129, -1], [161, 33])),
        (None, ([0, 0], [32, 32])),
    ] {
        let mut entries = vec![(
            TileKey {
                layer: target,
                mip: 0,
                x: 10_000_000,
                y: -10_000_000,
            },
            vec![0; TILE_BYTE_LEN],
        )];
        if let Some([x, y]) = point {
            let key = TileKey::from_pixel(target, 128, x, y);
            let mut pixels = vec![0; TILE_BYTE_LEN];
            let offset = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
            pixels[offset..offset + 4].copy_from_slice(&[255, 0, 0, 255]);
            entries.push((key, pixels));
        }
        let tiles = TileSnapshot::from_tiles(entries).unwrap();
        let domain = selection_domain(canvas, &tiles, target, None, limits).unwrap();
        assert_eq!(domain, expected);
        let selection = nyatidraw_paint_cpu::selection_all(domain.0, domain.1, limits).unwrap();
        assert!(
            !selection.contains_signed(32, 0),
            "a later fill must not paint transparent tile padding"
        );
        assert!(!selection.contains_signed(0, 32));
        if let Some([x, y]) = point {
            assert!(selection.contains_signed(x, y));
        }
        if !tiles.is_empty() {
            let tight = EditLimits {
                max_pixels: u64::from(TILE_EDGE).pow(2) - 1,
                ..limits
            };
            assert!(
                selection_domain(canvas, &tiles, target, None, tight).is_err(),
                "occupied bounds scanning must remain bounded"
            );
        }
    }
    let selected = nyatidraw_paint_cpu::selection_all([-2, -3], [1, 1], limits).unwrap();
    assert_eq!(
        selection_domain(
            canvas,
            &TileSnapshot::empty(),
            target,
            Some(&selected),
            limits
        )
        .unwrap(),
        ([-2, -3], [34, 35]),
        "existing off-page selection remains reachable"
    );
}
