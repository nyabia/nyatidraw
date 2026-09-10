// Integer, premultiplied linear RGBA8 composition; mirrors CPU byte rounding.
// params = opacity_u16, operation, source origin x/y.
@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(1) @binding(0) var backdrop_texture: texture_2d<f32>;
@group(2) @binding(0) var<uniform> params: vec4<u32>;

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

@fragment
fn fragment_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // A missing sparse source is explicit transparent, never another tile.
    if params.y == 5u { return vec4<f32>(0.0); }
    let local = vec2<u32>(position.xy);
    let raw = source_bytes(local + params.zw);
    if params.y == 4u { return vec4<f32>(raw) / 255.0; }
    let s = (raw * params.x + vec4<u32>(32767u)) / 65535u;
    let d = vec4<u32>(round(textureLoad(backdrop_texture, vec2<i32>(local), 0) * 255.0));
    let inverse = 255u - s.a;
    var rgb: vec3<u32>;
    var alpha = s.a + (d.a * inverse + 127u) / 255u;
    switch params.y {
        case 0u: { rgb = s.rgb + (d.rgb * inverse + vec3<u32>(127u)) / 255u; }
        case 1u: { rgb = (s.rgb * (255u - d.a) + d.rgb * inverse + s.rgb * d.rgb + vec3<u32>(127u)) / 255u; }
        case 2u: {
            rgb = (s.rgb * d.a + d.rgb * inverse + vec3<u32>(127u)) / 255u;
            alpha = d.a;
        }
        default: {
            rgb = (d.rgb * (vec3<u32>(inverse) + s.rgb) + vec3<u32>(127u)) / 255u;
            alpha = d.a;
        }
    }
    return vec4<f32>(vec4<u32>(min(rgb, vec3<u32>(alpha)), alpha)) / 255.0;
}
