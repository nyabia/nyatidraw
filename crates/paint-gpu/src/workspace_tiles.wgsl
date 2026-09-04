struct WorkspaceTileParams {
    document_x: vec4<f32>,
    document_y: vec4<f32>,
    document_size: vec4<f32>,
    tile_origin: vec4<f32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@group(1) @binding(0)
var<uniform> params: WorkspaceTileParams;

@vertex
fn vertex_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fragment_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let document = vec2<f32>(
        dot(params.document_x.xy, position.xy) + params.document_x.z,
        dot(params.document_y.xy, position.xy) + params.document_y.z,
    );

    // The finite page is composed by viewport.wgsl. Sparse signed tiles are
    // editor workspace content only and must never replace page pixels.
    let inside_page = document.x >= 0.0
        && document.y >= 0.0
        && document.x < params.document_size.x
        && document.y < params.document_size.y;
    if inside_page {
        discard;
    }

    // Keep the viewport shader's two-physical-pixel black page border above
    // off-page artwork. The border is presentation chrome, not artwork.
    let outside_x = max(max(-document.x, document.x - params.document_size.x), 0.0);
    let outside_y = max(max(-document.y, document.y - params.document_size.y), 0.0);
    let beyond_x = document.x < 0.0 || document.x >= params.document_size.x;
    let beyond_y = document.y < 0.0 || document.y >= params.document_size.y;
    let x_pixels = outside_x
        / max(length(vec2<f32>(params.document_x.x, params.document_x.y)), 0.000001);
    let y_pixels = outside_y
        / max(length(vec2<f32>(params.document_y.x, params.document_y.y)), 0.000001);
    let edge_distance_pixels = min(
        select(1e20, x_pixels, beyond_x),
        select(1e20, y_pixels, beyond_y),
    );
    if edge_distance_pixels <= 2.0 {
        discard;
    }

    let local = document - params.tile_origin.xy;
    if local.x < 0.0 || local.y < 0.0 || local.x >= 128.0 || local.y >= 128.0 {
        discard;
    }
    let artwork = textureLoad(source_texture, vec2<i32>(floor(local)), 0);
    if artwork.a <= 0.0 {
        discard;
    }
    return artwork;
}
