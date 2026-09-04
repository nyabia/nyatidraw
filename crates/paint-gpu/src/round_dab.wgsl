struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) center_px: vec2<f32>,
    @location(1) @interpolate(flat) radius_px: f32,
    @location(2) @interpolate(flat) opacity: f32,
};

@group(0) @binding(0)
var<uniform> brush_color: vec4<f32>;

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
) -> VertexOutput {
    let local = QUAD[vertex_index];
    var output: VertexOutput;
    output.position = vec4<f32>(center_ndc + local * radius_ndc, 0.0, 1.0);
    output.center_px = center_px;
    output.radius_px = radius_px;
    output.opacity = opacity;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let pixel_origin = input.position.xy - vec2<f32>(0.5, 0.5);
    let radius_squared = input.radius_px * input.radius_px;
    var covered = 0.0;
    for (var y = 0u; y < 4u; y += 1u) {
        for (var x = 0u; x < 4u; x += 1u) {
            let point = pixel_origin + vec2<f32>(COVERAGE_OFFSETS[x], COVERAGE_OFFSETS[y]);
            let delta = point - input.center_px;
            if dot(delta, delta) <= radius_squared {
                covered += 1.0;
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
