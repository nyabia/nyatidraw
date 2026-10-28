struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) serial: u32,
};

@vertex
fn vertex_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) serial: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var output: VertexOutput;
    output.position = vec4(positions[vertex], 0.0, 1.0);
    output.serial = serial;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let cell = (u32(input.position.x) - 4u) / 6u;
    if cell == 0u { return vec4(1.0, 0.0, 1.0, 1.0); }
    if cell == 9u { return vec4(0.0, 1.0, 1.0, 1.0); }
    let bit = f32((input.serial >> (8u - cell)) & 1u);
    return vec4(bit, bit, bit, 1.0);
}
