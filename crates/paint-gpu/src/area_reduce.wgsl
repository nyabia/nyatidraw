@group(0) @binding(0) var source: texture_2d<f32>;

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(positions[index], 0.0, 1.0);
}

@fragment
fn fragment_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let source_size = vec2<f32>(textureDimensions(source));
    let target_size = max(floor(source_size / 2.0), vec2(1.0));
    let start = floor(position.xy) * source_size / target_size;
    let end = (floor(position.xy) + vec2(1.0)) * source_size / target_size;
    var sum = vec4(0.0);
    // Odd dimensions cover up to three source texels, with fractional edge areas.
    for (var y = i32(floor(start.y)); y < i32(ceil(end.y)); y += 1) {
        for (var x = i32(floor(start.x)); x < i32(ceil(end.x)); x += 1) {
            let low = max(start, vec2<f32>(f32(x), f32(y)));
            let high = min(end, vec2<f32>(f32(x + 1), f32(y + 1)));
            let area = max(high - low, vec2(0.0));
            sum += textureLoad(source, vec2(x, y), 0) * area.x * area.y;
        }
    }
    return sum / ((end.x - start.x) * (end.y - start.y));
}
