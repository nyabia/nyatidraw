// Integer, premultiplied linear RGBA8 composition; mirrors CPU byte rounding.
// params = opacity_u16, operation, source origin x/y.
@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(1) @binding(0) var backdrop_texture: texture_2d<f32>;
struct StackParameters { blend: vec4<u32>, effect: vec4<u32>, }
@group(2) @binding(0) var<uniform> parameters: StackParameters;
@group(3) @binding(0) var<storage, read> color_transfer: array<u32>;
@group(3) @binding(1) var<storage, read> mask_coverage: array<u32>;

fn unpack_rgb(value: u32) -> vec3<u32> {
    return vec3<u32>(value & 255u, (value >> 8u) & 255u, (value >> 16u) & 255u);
}

fn layer_color(raw: vec4<u32>) -> vec4<u32> {
    if parameters.effect.x == 0u { return raw; }
    if raw.a == 0u { return vec4<u32>(0u); }
    let offset = raw.a * 256u;
    let straight = vec3<u32>(color_transfer[offset + raw.r],
        color_transfer[offset + raw.g], color_transfer[offset + raw.b]);
    let luma = (straight.r * 2126u + straight.g * 7152u + straight.b * 722u + 5000u) / 10000u;
    let srgb = (unpack_rgb(parameters.effect.y) * (255u - luma)
        + unpack_rgb(parameters.effect.z) * luma + vec3<u32>(127u)) / 255u;
    let decode = 65536u + offset;
    return vec4<u32>(color_transfer[decode + srgb.r], color_transfer[decode + srgb.g],
        color_transfer[decode + srgb.b], raw.a);
}

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(positions[index], 0.0, 1.0);
}

fn source_bytes(at: vec2<u32>) -> vec4<u32> {
    let dimensions = textureDimensions(source_texture);
    if any(at >= dimensions) { return vec4<u32>(0u); }
    return vec4<u32>(round(textureLoad(source_texture, vec2<i32>(at), 0) * 255.0));
}

fn blend_overlap(s: u32, d: u32, a: u32, b: u32, mode: u32) -> u32 {
    switch mode {
        case 0u: { return s * b; }
        case 1u: { return s * d; }
        case 2u: { return s * b + d * a - s * d; }
        case 3u: { return min(s * b + d * a, a * b); }
        case 4u: {
            if 2u * d <= b { return 2u * s * d; }
            return a * b - 2u * (a - s) * (b - d);
        }
        default: { return s * b; }
    }
}

@fragment
fn fragment_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let params = parameters.blend;
    let local = vec2<u32>(position.xy);
    var raw = layer_color(source_bytes(local + params.zw));
    let mask = parameters.effect.w;
    if (mask & 0x80000000u) != 0u {
        var coverage = mask & 255u;
        if (mask & 0x40000000u) != 0u {
            let pixel = local.y * 128u + local.x;
            let offset = (mask & 0x3fffff00u) >> 8u;
            coverage = (mask_coverage[offset + pixel / 4u] >> ((pixel % 4u) * 8u)) & 255u;
        }
        raw = (raw * coverage + vec4<u32>(127u)) / 255u;
    }
    if params.y == 256u { return vec4<f32>(raw) / 255.0; }
    let s = (raw * params.x + vec4<u32>(32767u)) / 65535u;
    let d = vec4<u32>(round(textureLoad(backdrop_texture, vec2<i32>(local), 0) * 255.0));
    let inverse = 255u - s.a;
    let atop = (params.y & 16u) != 0u;
    let mode = params.y & 15u;
    let overlap = vec3<u32>(
        blend_overlap(s.r, d.r, s.a, d.a, mode),
        blend_overlap(s.g, d.g, s.a, d.a, mode),
        blend_overlap(s.b, d.b, s.a, d.a, mode));
    let outside = select(s.rgb * (255u - d.a), vec3<u32>(0u), atop);
    let rgb = (outside + d.rgb * inverse + overlap + vec3<u32>(127u)) / 255u;
    var alpha = s.a + (d.a * inverse + 127u) / 255u;
    if atop { alpha = d.a; }
    return vec4<f32>(vec4<u32>(min(rgb, vec3<u32>(alpha)), alpha)) / 255.0;
}
