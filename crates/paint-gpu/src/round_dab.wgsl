struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) center_px: vec2<f32>,
    @location(1) @interpolate(flat) radius_px: f32,
    @location(2) @interpolate(flat) opacity: f32,
    @location(3) @interpolate(flat) hardness: f32,
};

@group(0) @binding(0)
var<uniform> brush_color: vec4<f32>;

struct SelectionCoordinates {
    dimensions: vec4<u32>, // enabled, width, height, reserved
    origin: vec4<i32>,
};
@group(1) @binding(0) var<uniform> selection: SelectionCoordinates;
@group(1) @binding(1) var<storage, read> selection_bits: array<u32>;

const QUAD: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>( 1.0,  1.0),
);

const COVERAGE_OFFSETS: array<f32, 4> = array<f32, 4>(
    0.125,
    0.375,
    0.625,
    0.875,
);

@vertex
fn vertex_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) center_ndc: vec2<f32>,
    @location(1) radius_ndc: vec2<f32>,
    @location(2) opacity: f32,
    @location(3) center_px: vec2<f32>,
    @location(4) radius_px: f32,
    @location(5) hardness: f32,
) -> VertexOutput {
    let local = QUAD[vertex_index];
    var output: VertexOutput;
    output.position = vec4<f32>(center_ndc + local * radius_ndc, 0.0, 1.0);
    output.center_px = center_px;
    output.radius_px = radius_px;
    output.opacity = opacity;
    output.hardness = hardness;
    return output;
}

fn shade_dab(input: VertexOutput) -> vec4<f32> {
    if selection.dimensions.x != 0u {
        let document = vec2<i32>(floor(input.position.xy)) + selection.origin.xy;
        if any(document < selection.origin.zw) { discard; }
        let point = vec2<u32>(document) - vec2<u32>(selection.origin.zw);
        if any(point >= selection.dimensions.yz) { discard; }
        let index = point.y * selection.dimensions.y + point.x;
        if ((selection_bits[index / 32u] >> (index % 32u)) & 1u) == 0u { discard; }
    }
    let pixel_origin = input.position.xy - vec2<f32>(0.5, 0.5);
    let radius_squared = input.radius_px * input.radius_px;
    var covered = 0.0;
    for (var y = 0u; y < 4u; y += 1u) {
        for (var x = 0u; x < 4u; x += 1u) {
            let point = pixel_origin + vec2<f32>(COVERAGE_OFFSETS[x], COVERAGE_OFFSETS[y]);
            let delta = point - input.center_px;
            let distance_squared = dot(delta, delta);
            if distance_squared <= radius_squared {
                if input.hardness >= 1.0 {
                    covered += 1.0;
                } else {
                    let t = clamp((1.0 - sqrt(distance_squared) / input.radius_px) / (1.0 - input.hardness), 0.0, 1.0);
                    covered += t * t * (3.0 - 2.0 * t);
                }
            }
        }
    }

    let coverage = covered / 16.0;
    if coverage == 0.0 {
        discard;
    }

    return vec4<f32>(
        brush_color.rgb,
        brush_color.a * input.opacity * coverage,
    );
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return shade_dab(input);
}

@fragment
fn fragment_alpha_locked(input: VertexOutput) -> @location(0) vec4<f32> {
    let straight = shade_dab(input);
    return vec4<f32>(straight.rgb * straight.a, straight.a);
}
