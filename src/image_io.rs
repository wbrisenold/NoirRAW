use std::path::Path;
use crate::{raw,color};
#[derive(Clone)]pub struct SceneImage{pub width:u32,pub height:u32,pub pixels:Vec<[f32;4]>}
impl SceneImage {
    pub fn proxy(&self,long_edge:u32)->Self{
        let w=self.width;let h=self.height;let long=w.max(h);
        if long<=long_edge{return self.clone()}
        let scale=long_edge as f64/long as f64;
        let dstw=(w as f64*scale).round().max(1.) as u32;
        let dsth=(h as f64*scale).round().max(1.) as u32;
        let mut pixels=Vec::with_capacity((dstw*dsth) as usize);
        for y in 0..dsth {for x in 0..dstw {let sx=((x as f64+0.5)/scale).floor() as u32;let sy=((y as f64+0.5)/scale).floor() as u32;
            pixels.push(self.pixels[(sy.min(h-1)*w+sx.min(w-1)) as usize]);}}
        Self{width:dstw,height:dsth,pixels}
    }
}
pub fn load(path:&Path)->Result<SceneImage,String>{
    let ext=path.extension().and_then(|x|x.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(),"jpg"|"jpeg"|"png"|"tiff"|"tif"|"bmp"|"webp"){
        let rgb=image::open(path).map_err(|e|e.to_string())?.to_rgb32f();
        let (width,height)=rgb.dimensions();
        let pixels=rgb.pixels().map(|p|{let linear=p.0.map(color::srgb_decode);let r=color::srgb_linear_to_rec2020(linear);[r[0],r[1],r[2],1.0]}).collect();
        Ok(SceneImage{width,height,pixels})
    }else{raw::open_raw(path)}
}
pub fn save_encoded_srgb(path:&Path,width:u32,height:u32,rgba:&[u8])->Result<(),String>{
    if rgba.len()!=width as usize*height as usize*4{return Err("Export buffer size mismatch".into())}
    let rgb=image::RgbaImage::from_raw(width,height,rgba.to_vec()).ok_or("Bad RGBA buffer")?;
    rgb.save(path).map_err(|e|e.to_string())
}
#[cfg(test)]mod tests{use super::*;#[test]fn proxy_is_bounded(){let a=SceneImage{width:2000,height:1000,pixels:vec![[0.;4];2_000_000]};let p=a.proxy(1080);assert_eq!((p.width,p.height),(1080,540));}}
