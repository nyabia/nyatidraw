struct SelectionCoordinates { dimensions: vec4<u32>, origin: vec4<i32> };
struct ViewportParams { document_x: vec4<f32>, document_y: vec4<f32>, document_size: vec4<f32> };
@group(0) @binding(0) var<uniform> selection: SelectionCoordinates;
@group(0) @binding(1) var<storage, read> bits: array<u32>;
@group(1) @binding(1) var<uniform> params: ViewportParams;

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(points[index], 0.0, 1.0);
}

fn covered(document: vec2<f32>) -> bool {
    if any(document < vec2<f32>(0.0)) || any(document >= vec2<f32>(selection.dimensions.yz)) { return false; }
    let pixel = vec2<u32>(floor(document));
    let index = pixel.y * selection.dimensions.y + pixel.x;
    return (bits[index / 32u] & (1u << (index % 32u))) != 0u;
}

@fragment
fn fragment_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let document = vec2<f32>(dot(params.document_x.xy, position.xy) + params.document_x.z,
        dot(params.document_y.xy, position.xy) + params.document_y.z);
    let dx = vec2<f32>(params.document_x.x, params.document_y.x) * 1.5;
    let dy = vec2<f32>(params.document_x.y, params.document_y.y) * 1.5;
    let inside = covered(document);
    if covered(document + dx) == inside && covered(document - dx) == inside
        && covered(document + dy) == inside && covered(document - dy) == inside { discard; }
    let stripe = (u32(position.x + position.y) / 6u) % 2u;
    return vec4<f32>(vec3<f32>(select(0.02, 1.0, stripe == 0u)), 1.0);
}
