// Text shader for rendering glyphs from the atlas texture.
// Supports both alpha-mask glyphs (colored by fg uniform) and color emoji.

struct Uniforms {
    viewport_size: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(0) @binding(1)
var atlas_texture: texture_2d<f32>;

@group(0) @binding(2)
var atlas_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,   // pixel position
    @location(1) uv: vec2<f32>,         // texture coordinates
    @location(2) fg_color: vec4<f32>,   // foreground color
    @location(3) is_color: f32,         // 1.0 for color emoji, 0.0 for mask
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg_color: vec4<f32>,
    @location(2) is_color: f32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let ndc_x = (input.position.x / uniforms.viewport_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (input.position.y / uniforms.viewport_size.y) * 2.0;
    output.position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    output.uv = input.uv;
    output.fg_color = input.fg_color;
    output.is_color = input.is_color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas_texture, atlas_sampler, input.uv);

    if (input.is_color > 0.5) {
        // Color glyph (emoji) — use texture color directly
        return tex;
    } else {
        // Alpha-mask glyph — multiply fg color by texture alpha
        return vec4<f32>(input.fg_color.rgb, input.fg_color.a * tex.a);
    }
}
