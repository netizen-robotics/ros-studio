@group(0) @binding(0) var diffuse: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> tint: vec4<f32>;
struct VertexOut { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn vs_main(@location(0) position:vec4<f32>,@location(1) uv_data:vec4<f32>) -> VertexOut {
    var out:VertexOut;out.position=position;out.uv=vec2<f32>(uv_data.x,1.0-uv_data.y);return out;
}
@fragment fn fs_main(in:VertexOut)->@location(0) vec4<f32>{return textureSample(diffuse,image_sampler,in.uv)*tint;}
