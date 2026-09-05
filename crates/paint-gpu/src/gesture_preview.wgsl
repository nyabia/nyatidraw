@group(0) @binding(0) var<uniform> screen: vec4<f32>;
struct VertexOut { @builtin(position) position: vec4<f32>, @location(0) side: f32 };

@vertex
fn vertex_main(@builtin(vertex_index) vertex: u32, @location(0) start: vec2<f32>, @location(1) end: vec2<f32>) -> VertexOut {
    let corners = array<vec2<f32>, 6>(vec2<f32>(0.0,-1.0), vec2<f32>(1.0,-1.0), vec2<f32>(0.0,1.0),
        vec2<f32>(0.0,1.0), vec2<f32>(1.0,-1.0), vec2<f32>(1.0,1.0));
    let corner = corners[vertex];
    let delta = end - start;
    let normal = vec2<f32>(-delta.y, delta.x) / max(length(delta), 0.0001);
    let point = mix(start, end, corner.x) + normal * corner.y * 2.0;
    var out: VertexOut;
    out.position = vec4<f32>(point.x / screen.x * 2.0 - 1.0, 1.0 - point.y / screen.y * 2.0, 0.0, 1.0);
    out.side = corner.y;
    return out;
}

@fragment
fn fragment_main(in: VertexOut) -> @location(0) vec4<f32> {
    let bright = abs(in.side) < 0.55;
    return vec4<f32>(select(vec3<f32>(0.01), vec3<f32>(0.1, 0.9, 1.0), bright), 1.0);
}
