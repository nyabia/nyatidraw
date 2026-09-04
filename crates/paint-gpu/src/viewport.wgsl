struct ViewportParams {
    document_x: vec4<f32>,
    document_y: vec4<f32>,
    document_size: vec4<f32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@group(1) @binding(0)
var source_sampler: sampler;

@group(1) @binding(1)
var<uniform> params: ViewportParams;

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

    // The disposable viewport projection owns page chrome. Raster/group
    // textures retain premultiplied artwork only; neither this checkerboard nor
    // the workspace/border colors enter durable pixels.
    let outside_x = max(max(-document.x, document.x - params.document_size.x), 0.0);
    let outside_y = max(max(-document.y, document.y - params.document_size.y), 0.0);
    let beyond_x = document.x < 0.0 || document.x >= params.document_size.x;
    let beyond_y = document.y < 0.0 || document.y >= params.document_size.y;
    if beyond_x || beyond_y {
        // Convert a document-space edge distance to physical display pixels.
        // This preserves a distinct two-pixel border under pan, zoom, and
        // rotation without another pass or per-frame allocation.
        let x_pixels = outside_x
            / max(length(vec2<f32>(params.document_x.x, params.document_x.y)), 0.000001);
        let y_pixels = outside_y
            / max(length(vec2<f32>(params.document_y.x, params.document_y.y)), 0.000001);
        let edge_distance_pixels = min(
            select(1e20, x_pixels, beyond_x),
            select(1e20, y_pixels, beyond_y),
        );
        let workspace = vec3<f32>(0.035);
        let border = vec3<f32>(0.0);
        return vec4<f32>(select(workspace, border, edge_distance_pixels <= 2.0), 1.0);
    }

    // Checkerboard is a page-only alpha backdrop. It cannot leak into the
    // workspace and it never alters the composite source texture.
    let checker_cell = 16.0;
    let checker = (i32(floor(position.x / checker_cell))
        + i32(floor(position.y / checker_cell))) & 1;
    let checker_value = select(0.58, 0.72, checker == 0);
    let checkerboard = vec3<f32>(checker_value);
    let uv = document / params.document_size.xy;
    let artwork = textureSampleLevel(source_texture, source_sampler, uv, 0.0);
    return vec4<f32>(artwork.rgb + checkerboard * (1.0 - artwork.a), 1.0);
}
