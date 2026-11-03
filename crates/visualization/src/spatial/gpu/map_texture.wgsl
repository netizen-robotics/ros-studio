@group(0) @binding(0) var cells: texture_2d<u32>;
@group(0) @binding(1) var<uniform> palette: array<vec4<f32>,256>;
@group(0) @binding(2) var<uniform> opacity: vec4<f32>;
struct VertexOut { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn vs_main(@location(0) position:vec4<f32>,@location(1) uv_data:vec4<f32>) -> VertexOut {
    var out:VertexOut;out.position=position;out.uv=uv_data.xy;return out;
}
@fragment fn fs_main(in:VertexOut)->@location(0) vec4<f32>{
    let dimensions=textureDimensions(cells);
    let pixel=min(vec2<u32>(clamp(in.uv,vec2<f32>(0.0),vec2<f32>(1.0))*vec2<f32>(dimensions)),dimensions-vec2<u32>(1));
    let index=textureLoad(cells,vec2<i32>(pixel),0).r;
    let color=palette[index]*vec4<f32>(1.0,1.0,1.0,opacity.x);
    if color.a==0.0 {discard;}
    // Palette RGB is sRGB; the framebuffer performs the final linear-to-sRGB conversion.
    let low=color.rgb/12.92;
    let high=pow((color.rgb+vec3<f32>(0.055))/1.055,vec3<f32>(2.4));
    return vec4<f32>(select(high,low,color.rgb<=vec3<f32>(0.04045)),color.a);
}
