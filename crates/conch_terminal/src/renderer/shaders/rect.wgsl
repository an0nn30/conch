// Rectangle shader for cursor, underlines, selection, strikethrough.
// Each rect is a solid-color quad.

struct Uniforms {
    viewport_size: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec2<f32>,  // pixel position
    @location(1) color: vec4<f32>,     // RGBA color
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    // Convert pixel coords to NDC: (0,0) top-left → (-1,1), (w,h) bottom-right → (1,-1)
    let ndc_x = (input.position.x / uniforms.viewport_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (input.position.y / uniforms.viewport_size.y) * 2.0;
    output.position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
