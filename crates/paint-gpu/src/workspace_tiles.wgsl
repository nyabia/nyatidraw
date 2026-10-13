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

fn load_or_transparent(pixel: vec2<i32>, level: i32) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(source_texture, level));
    if any(pixel < vec2(0)) || any(pixel >= size) { return vec4(0.0); }
    return textureLoad(source_texture, pixel, level);
}

fn filtered_level(document: vec2<f32>, local: vec2<f32>, level: i32) -> vec4<f32> {
    let scale = exp2(f32(level));
    if level > 7 {
        // Each 128px tile contributes its area to the same globally aligned coarse grid.
        let cell = floor(params.tile_origin.xy / scale);
        let weight = max(vec2(0.0), vec2(1.0) - abs(document / scale - vec2(0.5) - cell));
        let area = 128.0 / scale;
        return textureLoad(source_texture, vec2(0), 7) * area * area * weight.x * weight.y;
    }
    let pixel = local / scale - vec2(0.5);
    let low = vec2<i32>(floor(pixel));
    let weight = fract(pixel);
    return mix(
        mix(load_or_transparent(low, level), load_or_transparent(low + vec2(1, 0), level), weight.x),
        mix(load_or_transparent(low + vec2(0, 1), level), load_or_transparent(low + vec2(1, 1), level), weight.x),
        weight.y,
    );
}

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
    let x_pixels = outside_x
        / max(length(vec2<f32>(params.document_x.x, params.document_x.y)), 0.000001);
    let y_pixels = outside_y
        / max(length(vec2<f32>(params.document_y.x, params.document_y.y)), 0.000001);
    let edge_distance_pixels = max(x_pixels, y_pixels);
    if edge_distance_pixels <= 2.0 {
        discard;
    }

    let local = document - params.tile_origin.xy;
    let footprint = max(length(params.document_x.xy), length(params.document_y.xy));
    var artwork: vec4<f32>;
    if footprint <= 1.00001 {
        if any(local < vec2(0.0)) || any(local >= vec2(128.0)) { discard; }
        artwork = textureLoad(source_texture, vec2<i32>(floor(local)), 0);
    } else {
        let lod = log2(footprint);
        let low = i32(floor(lod));
        let support = exp2(ceil(lod));
        let cell_origin = floor(params.tile_origin.xy / support) * support;
        let cell_size = max(128.0, support);
        if any(document < cell_origin - vec2(support * 0.5))
            || any(document >= cell_origin + vec2(cell_size + support * 0.5)) {
            discard;
        }
        artwork = mix(filtered_level(document, local, low), filtered_level(document, local, low + 1), fract(lod));
    }
    if artwork.a <= 0.0 {
        discard;
    }
    return artwork;
}
