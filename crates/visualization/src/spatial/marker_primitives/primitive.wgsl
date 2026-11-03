struct MarkerTime { tick:vec4<u32> };
@group(0) @binding(0) var<uniform> marker_time:MarkerTime;
@group(1) @binding(0) var material_image:texture_2d<f32>;
@group(1) @binding(1) var material_sampler:sampler;
struct VertexInput {
 @location(0) position: vec3<f32>, @location(1) shade: f32,
 @location(2) c0:vec4<f32>, @location(3) c1:vec4<f32>, @location(4) c2:vec4<f32>, @location(5) c3:vec4<f32>,
 @location(6) color:vec4<f32>, @location(7) eye:vec4<f32>, @location(8) ray:vec4<f32>, @location(9) shape:vec4<u32>,
};
struct VertexOutput {
 @builtin(position) position:vec4<f32>, @location(0) color:vec4<f32>, @location(1) local:vec3<f32>, @location(2) shade:f32,
 @location(3) @interpolate(flat) c0:vec4<f32>, @location(4) @interpolate(flat) c1:vec4<f32>, @location(5) @interpolate(flat) c2:vec4<f32>, @location(6) @interpolate(flat) c3:vec4<f32>,
 @location(7) @interpolate(flat) eye:vec4<f32>, @location(8) @interpolate(flat) ray:vec4<f32>, @location(9) @interpolate(flat) shape:vec4<u32>,
};
@vertex fn vs_main(i:VertexInput)->VertexOutput {
 var o:VertexOutput; o.position=mat4x4<f32>(i.c0,i.c1,i.c2,i.c3)*vec4<f32>(i.position,1.);
 o.color=i.color; o.local=i.position;
 if (i.shape.x&255u)==2u {o.color=mix(i.color,i.eye,i.position.x+0.5);}
 if (i.shape.x&255u)==3u {o.position=i.c0*i.position.x+i.c1*i.position.y+i.c2*i.position.z;o.color=i.color*i.position.x+i.eye*i.position.y+i.ray*i.position.z;}
 if (i.shape.x&255u)==4u {
  let t=i.position.xy+vec2<f32>(0.5);
  o.position=mix(mix(i.c0,i.c1,t.x),mix(i.c3,i.c2,t.x),t.y);
  o.color=mix(i.color,i.eye,t.x);
 }
 if (i.shape.x&255u)==6u {
  o.position=i.c0*i.position.x+i.c1*i.position.y+i.c2*i.position.z;
  let uv=i.eye.xy*i.position.x+i.eye.zw*i.position.y+i.ray.xy*i.position.z;
  o.local=vec3<f32>(uv.x,1.-uv.y,0.);
 }
 o.shade=i.shade; o.c0=i.c0;o.c1=i.c1;o.c2=i.c2;o.c3=i.c3;o.eye=i.eye;o.ray=i.ray;o.shape=i.shape;return o;
}
// Rescale before normalization so finite local-eye/ray values cannot overflow
// the squared length for very small ellipsoids or distant perspective eyes.
fn direction(v:vec3<f32>)->vec3<f32> {
 let largest=max(max(abs(v.x),abs(v.y)),abs(v.z));
 if largest==0. {return vec3<f32>(0.,0.,1.);}
 return normalize(v/largest);
}
struct FragmentOutput { @location(0) color:vec4<f32>, @builtin(frag_depth) depth:f32 };
@fragment fn fs_main(i:VertexOutput,@builtin(front_facing) front:bool)->FragmentOutput {
 if i.shape.w==1u && (marker_time.tick.y>i.shape.z || (marker_time.tick.y==i.shape.z && marker_time.tick.x>=i.shape.y)) {discard;}
 var o:FragmentOutput;o.depth=i.position.z;var shade=i.shade;
 if (i.shape.x&255u)==1u {
  let perspective=i.eye.w==1.;let inside=perspective&&all(abs(i.eye.xyz)<vec3<f32>(0.5));
  if front==inside {discard;}
  var origin=i.local;var ray=direction(i.ray.xyz);
  if perspective {origin=i.eye.xyz;ray=direction(i.local-origin);}
  let center=-dot(origin,ray);let closest=origin+ray*center;
  let d=0.25-dot(closest,closest);if d<0. {discard;}
  let root=sqrt(d);if center+root<0. {discard;}
  var hit=closest-ray*root;if center-root<0. {hit=closest+ray*root;}
let clip=mat4x4<f32>(i.c0,i.c1,i.c2,i.c3)*vec4<f32>(hit,1.);
  if clip.w<=0. {discard;}o.depth=clip.z/clip.w;if o.depth<0.||o.depth>1. {discard;}
  let n=normalize(hit);shade=0.55+0.45*abs(dot(n,normalize(vec3<f32>(0.3,0.4,0.866))));
 }
 var alpha=i.color.a;
 if (i.shape.x&255u)==5u {
  let t=vec2<f32>(i.local.x+0.5,0.5-i.local.y);
  let coverage=textureSampleLevel(material_image,material_sampler,mix(i.eye.xy,i.eye.zw,t),0.).r;
  if coverage<=0. {discard;}
  alpha*=coverage;
 }
 if (i.shape.x&255u)==6u {
  let sampled=textureSampleLevel(material_image,material_sampler,i.local.xy,0.);
  let base_alpha=i.color.a*sampled.a;let t=i.c3.a;
  alpha=base_alpha+(1.-base_alpha)*t;
  if alpha<=0. {discard;}
  let rgb=(i.color.rgb*sampled.rgb*base_alpha*(1.-t)+i.c3.rgb*t)/alpha;
  o.color=vec4<f32>(rgb,alpha);return o;
 }
 o.color=vec4<f32>(i.color.rgb*shade,alpha);return o;
}
