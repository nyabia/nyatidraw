//! Document-space free-transform handles. A drag always evaluates its starting
//! projection; pointer release ends only the drag, never commits the artwork.
use nyatidraw_api::{AffineTransform, TransformProjection};

const SCALE_UNIT: f64 = 1_000_000.0;
const OFFSET_UNIT: f64 = 1000.0;

pub(crate) struct TransformGesture {
    original: AffineTransform,
    center: [f64; 2],
    size: [u32; 2],
    start: [f64; 2],
    kind: Handle,
}

enum Handle {
    Move,
    Scale(usize),
    Rotate,
}

struct Geometry {
    center: [f64; 2],
    corners: [[f64; 2]; 4],
    top_mid: [f64; 2],
    rotation: [f64; 2],
}

impl Geometry {
    fn new(projection: &TransformProjection, radius: f64) -> Option<Self> {
        if !radius.is_finite()
            || radius <= 0.0
            || projection.size.contains(&0)
            || projection.transform.scale_ppm.contains(&0)
        {
            return None;
        }
        // Drawable signed coordinates fit comfortably within exact f64 integer
        // range. Reject impossible projection values before converting i64.
        let limit = (i64::from(u32::MAX) + 1) * 1000;
        if projection
            .transform
            .offset_milli
            .iter()
            .chain(projection.corners_milli.iter().flatten())
            .any(|&v| !(-limit..=limit).contains(&v))
        {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        let center = std::array::from_fn(|axis| {
            f64::from(projection.origin[axis])
                + f64::from(projection.size[axis]) / 2.0
                + projection.transform.offset_milli[axis] as f64 / OFFSET_UNIT
        });
        #[allow(clippy::cast_precision_loss)]
        let corners = projection
            .corners_milli
            .map(|p| p.map(|v| v as f64 / OFFSET_UNIT));
        let top_mid = std::array::from_fn(|axis| f64::midpoint(corners[0][axis], corners[1][axis]));
        let outward = subtract(top_mid, center);
        let distance = outward[0].hypot(outward[1]);
        if distance <= f64::EPSILON {
            return None;
        }
        let rotation =
            std::array::from_fn(|axis| top_mid[axis] + outward[axis] / distance * radius * 3.0);
        rotation.iter().all(|v| v.is_finite()).then_some(Self {
            center,
            corners,
            top_mid,
            rotation,
        })
    }
}

impl TransformGesture {
    /// Radius is expressed in document units, normally a fixed screen hit size
    /// divided by the viewport zoom. Corner scaling has priority over rotation
    /// and body translation. The native owner must separately pin the viewport
    /// revision and reject updates if it changes during the drag.
    pub(crate) fn begin(
        projection: &TransformProjection,
        point: [f64; 2],
        hit_radius_document: f64,
    ) -> Option<Self> {
        if !point.iter().all(|v| v.is_finite()) {
            return None;
        }
        let geometry = Geometry::new(projection, hit_radius_document)?;
        let nearby = |p: [f64; 2]| {
            let delta = subtract(point, p);
            delta[0].hypot(delta[1]) <= hit_radius_document
        };
        let kind = if let Some(index) = geometry.corners.iter().position(|&p| nearby(p)) {
            Handle::Scale(index)
        } else if nearby(geometry.rotation) {
            Handle::Rotate
        } else {
            let local = unrotate(subtract(point, geometry.center), projection.transform);
            let inside = (0..2).all(|axis| {
                local[axis].abs()
                    <= f64::from(projection.size[axis])
                        * f64::from(projection.transform.scale_ppm[axis])
                        / (2.0 * SCALE_UNIT)
            });
            if !inside {
                return None;
            }
            Handle::Move
        };
        Some(Self {
            original: projection.transform,
            center: geometry.center,
            size: projection.size,
            start: point,
            kind,
        })
    }

    /// Calculates a candidate from the original drag state, not the last
    /// preview. Crossing the pivot during scale is rejected; flips remain
    /// explicit controls rather than an accidental side effect of dragging.
    pub(crate) fn update(&self, point: [f64; 2]) -> Result<AffineTransform, String> {
        if !point.iter().all(|v| v.is_finite()) {
            return Err("변형 좌표가 유효하지 않습니다.".into());
        }
        let mut next = self.original;
        match self.kind {
            Handle::Move => {
                for (axis, delta) in subtract(point, self.start).into_iter().enumerate() {
                    // Bound before float→integer conversion; checked_add then
                    // retains exact original milli offsets even at large pans.
                    let milli = (delta * OFFSET_UNIT).round();
                    let max = f64::from(u32::MAX) * OFFSET_UNIT;
                    if !milli.is_finite() || milli.abs() > max {
                        return Err("이동 거리가 허용 범위를 벗어났습니다.".into());
                    }
                    #[allow(clippy::cast_possible_truncation)]
                    let milli = milli as i64;
                    next.offset_milli[axis] = next.offset_milli[axis]
                        .checked_add(milli)
                        .ok_or("이동 거리가 허용 범위를 벗어났습니다.")?;
                }
            }
            Handle::Scale(index) => {
                let delta = unrotate(subtract(point, self.start), self.original);
                let corner_signs = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
                let flips = [self.original.flip_x, self.original.flip_y];
                for axis in 0..2 {
                    let half = f64::from(self.size[axis]) / 2.0;
                    let sign = corner_signs[index][axis] * if flips[axis] { -1.0 } else { 1.0 };
                    let ppm = (f64::from(self.original.scale_ppm[axis])
                        + delta[axis] / (half * sign) * SCALE_UNIT)
                        .round();
                    if !ppm.is_finite() || !(1.0..=f64::from(u32::MAX)).contains(&ppm) {
                        return Err(
                            "중심을 넘는 크기 조절은 적용하지 않습니다. 반전 도구를 사용하세요."
                                .into(),
                        );
                    }
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    {
                        next.scale_ppm[axis] = ppm as u32;
                    }
                }
            }
            Handle::Rotate => {
                let start = subtract(self.start, self.center);
                let current = subtract(point, self.center);
                if current[0].hypot(current[1]) <= f64::EPSILON {
                    return Err("회전 중심에서는 각도를 정할 수 없습니다.".into());
                }
                let delta = current[1].atan2(current[0]) - start[1].atan2(start[0]);
                let angle = (f64::from(self.original.rotation_millidegrees)
                    + delta.to_degrees() * 1000.0)
                    .round()
                    .rem_euclid(360_000.0);
                // Modulo is intentional for angles; bounded finite conversion.
                #[allow(clippy::cast_possible_truncation)]
                {
                    next.rotation_millidegrees = angle as i32;
                }
            }
        }
        Ok(next)
    }
}

/// One open polyline for the existing native guide overlay. It retraces short
/// connections to draw the outline, four corner handles, and rotation handle.
/// It is display-only; the mask itself is not changed by these guide points.
pub(crate) fn guide(projection: &TransformProjection, radius: f64) -> Vec<[f64; 2]> {
    let Some(geometry) = Geometry::new(projection, radius) else {
        return Vec::new();
    };
    let mut points = Vec::with_capacity(40);
    for center in geometry.corners {
        points.push(center);
        append_square(&mut points, center, radius * 0.5);
    }
    points.extend([geometry.corners[0], geometry.top_mid, geometry.rotation]);
    append_square(&mut points, geometry.rotation, radius * 0.5);
    points
}

fn append_square(points: &mut Vec<[f64; 2]>, center: [f64; 2], half: f64) {
    for offset in [
        [-half, -half],
        [half, -half],
        [half, half],
        [-half, half],
        [-half, -half],
        [0.0, 0.0],
    ] {
        points.push([center[0] + offset[0], center[1] + offset[1]]);
    }
}

fn subtract(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn unrotate(point: [f64; 2], transform: AffineTransform) -> [f64; 2] {
    let angle = f64::from(transform.rotation_millidegrees.rem_euclid(360_000)) / 1000.0;
    let (sin, cos) = angle.to_radians().sin_cos();
    [
        cos * point[0] + sin * point[1],
        -sin * point[0] + cos * point[1],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection(transform: AffineTransform) -> TransformProjection {
        let origin = [-20, -10];
        let size = [40, 20];
        let (sin, cos) = (f64::from(transform.rotation_millidegrees) / 1000.0)
            .to_radians()
            .sin_cos();
        let mut corners_milli =
            [[-20.0, -10.0], [20.0, -10.0], [20.0, 10.0], [-20.0, 10.0]].map(|point| {
                let x = point[0] * f64::from(transform.scale_ppm[0]) / SCALE_UNIT
                    * if transform.flip_x { -1.0 } else { 1.0 };
                let y = point[1] * f64::from(transform.scale_ppm[1]) / SCALE_UNIT
                    * if transform.flip_y { -1.0 } else { 1.0 };
                #[allow(clippy::cast_possible_truncation)]
                [cos * x - sin * y, sin * x + cos * y].map(|v| (v * 1000.0).round() as i64)
            });
        for point in &mut corners_milli {
            for (axis, coordinate) in point.iter_mut().enumerate() {
                *coordinate += transform.offset_milli[axis];
            }
        }
        TransformProjection {
            generation: 1,
            transform,
            origin,
            size,
            corners_milli,
            can_commit: true,
        }
    }

    #[test]
    fn drag_preserves_fixed_pivot_original_source_and_explicit_flips() {
        // Risk: incremental preview geometry, rotation sign, or flipped handles
        // silently move/distort selected artwork away from its intended pivot.
        for flip_x in [false, true] {
            let source = projection(AffineTransform {
                rotation_millidegrees: 90_000,
                offset_milli: [-7000, 4000],
                flip_x,
                ..AffineTransform::default()
            });
            let geometry = Geometry::new(&source, 1.0).unwrap();
            let start = geometry.corners[2];
            let drag = TransformGesture::begin(&source, start, 1.0).unwrap();
            assert_eq!(drag.update(start).unwrap(), source.transform);
            let scaled = drag
                .update(std::array::from_fn(|axis| {
                    geometry.center[axis] + (start[axis] - geometry.center[axis]) * 2.0
                }))
                .unwrap();
            assert_eq!(scaled.scale_ppm, [2_000_000; 2]);
            assert_eq!(scaled.offset_milli, source.transform.offset_milli);
            assert_eq!(scaled.flip_x, flip_x);
            assert_eq!(drag.update(start).unwrap(), source.transform);
            assert!(
                drag.update(std::array::from_fn(|axis| {
                    geometry.center[axis] - (start[axis] - geometry.center[axis])
                }))
                .is_err()
            );
        }
        let source = projection(AffineTransform::default());
        let drag = TransformGesture::begin(&source, [0.0, 0.0], 1.0).unwrap();
        assert_eq!(
            drag.update([-3.125, 2.5]).unwrap().offset_milli,
            [-3125, 2500]
        );
        assert_eq!(drag.update([1.0, 1.0]).unwrap().offset_milli, [1000, 1000]);
        let rotation = Geometry::new(&source, 1.0).unwrap().rotation;
        let drag = TransformGesture::begin(&source, rotation, 1.0).unwrap();
        assert_eq!(
            drag.update([-rotation[1], rotation[0]])
                .unwrap()
                .rotation_millidegrees,
            90_000
        );
        assert!(drag.update([0.0, 0.0]).is_err());
        assert!(drag.update([f64::NAN, 0.0]).is_err());
        assert!(TransformGesture::begin(&source, [1e10, 0.0], 1.0).is_none());
    }
}
