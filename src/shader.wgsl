struct Params {
  controls: vec4<f32>,    // exposure EV, contrast, LUT opacity, mode
  domain_min: vec4<f32>,
  domain_span: vec4<f32>,
}
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var cube: texture_3d<f32>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var output: texture_storage_2d<rgba8unorm,write>;

fn encode_srgb(x: f32) -> f32 {
  if x <= 0.0031308 { return x*12.92; }
  return 1.055*pow(max(x,0.0),1.0/2.4)-0.055;
}
fn rec2020_to_srgb(x:vec3<f32>)->vec3<f32>{
  return vec3<f32>(
    1.660491*x.r-0.5876411*x.g-0.0728499*x.b,
    -0.1245505*x.r+1.1328999*x.g-0.0083494*x.b,
    -0.0181508*x.r-0.1005789*x.g+1.1187297*x.b);
}
fn signed_log(x:f32)->f32 {
  if x<0.0 {return 0.2*(1.0-log2(1.0+min(-x,2.0)*8.0)/log2(17.0));}
  return 0.2+0.8*log2(1.0+min(x,64.0)*8.0)/log2(513.0);
}
fn lut_at(coord:vec3<i32>)->vec3<f32>{let max_idx=i32(textureDimensions(cube).x)-1;return textureLoad(cube,clamp(coord,vec3<i32>(0),vec3<i32>(max_idx)),0).rgb;}
// 6-way tetrahedral interpolation on an R-fastest 3D `.cube` grid.
fn tetrahedral(input:vec3<f32>)->vec3<f32>{
  let size=f32(textureDimensions(cube).x);
  let p=clamp(input,vec3<f32>(0.0),vec3<f32>(1.0))*(size-1.0);
  let low=vec3<i32>(floor(p));let f=fract(p);
  var a=vec3<i32>(0);var b=vec3<i32>(0);
  if f.x>=f.y {
    if f.y>=f.z {a=vec3<i32>(1,0,0);b=vec3<i32>(1,1,0);}
    else if f.x>=f.z {a=vec3<i32>(1,0,0);b=vec3<i32>(1,0,1);}
    else {a=vec3<i32>(0,0,1);b=vec3<i32>(1,0,1);}
  } else {
    if f.x>=f.z {a=vec3<i32>(0,1,0);b=vec3<i32>(1,1,0);}
    else if f.y>=f.z {a=vec3<i32>(0,1,0);b=vec3<i32>(0,1,1);}
    else {a=vec3<i32>(0,0,1);b=vec3<i32>(0,1,1);}
  }
  let delta=b-a;
  let w1=select(select(f.z,f.y,a.y==1),f.x,a.x==1);
  let w2=select(select(f.z,f.y,delta.y==1),f.x,delta.x==1);
  let w3=f.x+f.y+f.z-w1-w2;
  return lut_at(low)+w1*(lut_at(low+a)-lut_at(low))+w2*(lut_at(low+b)-lut_at(low+a))+w3*(lut_at(low+vec3<i32>(1))-lut_at(low+b));
}

@compute @workgroup_size(8,8)
fn apply(@builtin(global_invocation_id) gid:vec3<u32>) {
  let wh=textureDimensions(output);
  if gid.x>=wh.x || gid.y>=wh.y {return;}
  var rgb=textureLoad(source,vec2<i32>(gid.xy),0).rgb;
  // Pre-film exposure and contrast. Simple scene-linear MVP control.
  rgb=rgb*exp2(params.controls.x);
  rgb=vec3<f32>(0.18)+ (rgb-vec3<f32>(0.18))*(1.0+params.controls.y);
  let srgb=rec2020_to_srgb(rgb);
  let normal=clamp(vec3<f32>(encode_srgb(srgb.r),encode_srgb(srgb.g),encode_srgb(srgb.b)),vec3<f32>(0.0),vec3<f32>(1.0));
  var result=normal;
  let mode=i32(params.controls.w);
  if mode>0 {
    var coord=normal;
    if mode==2 {coord=vec3<f32>(signed_log(rgb.r),signed_log(rgb.g),signed_log(rgb.b));}
    coord=(coord-params.domain_min.xyz)/params.domain_span.xyz;
    let styled=tetrahedral(coord);
    result=mix(normal,styled,params.controls.z);
  }
  textureStore(output,vec2<i32>(gid.xy),vec4<f32>(clamp(result,vec3<f32>(0.0),vec3<f32>(1.0)),1.0));
}
