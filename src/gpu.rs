//! WGPU/Metal compute: the same shader is used for GPU preview and original-sized output.
//! Preview never downloads its pixels. Only explicit export maps a GPU buffer.
use std::{path::PathBuf,sync::mpsc};
use slint::wgpu_30::wgpu;
use wgpu::util::DeviceExt;
use bytemuck::{Pod,Zeroable};
use crate::{image_io::SceneImage,lut::{Cube,InputSpace}};

#[repr(C)]#[derive(Clone,Copy,Pod,Zeroable)]
struct ShaderParams{controls:[f32;4],domain_min:[f32;4],domain_span:[f32;4]}
#[derive(Clone,Copy)]pub struct Controls{pub exposure:f32,pub contrast:f32,pub strength:f32}
impl Default for Controls{fn default()->Self{Self{exposure:0.,contrast:0.,strength:1.}}}

struct CachedInput{id:u64,texture:wgpu::Texture,width:u32,height:u32}
struct CachedLut{path:PathBuf,texture:wgpu::Texture,size:u32}
struct Output{texture:wgpu::Texture,width:u32,height:u32}

pub struct GpuEngine{
    device:wgpu::Device,queue:wgpu::Queue,
    pipeline:wgpu::ComputePipeline,layout:wgpu::BindGroupLayout,
    input:Option<CachedInput>,lut:Option<CachedLut>,output:Option<Output>,
    empty_lut:wgpu::Texture,
    pub lut_upload_count:u64,pub scene_upload_count:u64,pub render_count:u64,
}
impl GpuEngine{
    pub fn new(device:wgpu::Device,queue:wgpu::Queue)->Self{
        let shader=device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("NoirRAW film LUT bindings"),entries:&[
            wgpu::BindGroupLayoutEntry{binding:0,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None},
            wgpu::BindGroupLayoutEntry{binding:1,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D3,multisampled:false},count:None},
            wgpu::BindGroupLayoutEntry{binding:2,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},
            wgpu::BindGroupLayoutEntry{binding:3,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::StorageTexture{access:wgpu::StorageTextureAccess::WriteOnly,format:wgpu::TextureFormat::Rgba8Unorm,view_dimension:wgpu::TextureViewDimension::D2},count:None},
        ]});
        let pipe_layout=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:Some("NoirRAW compute layout"),bind_group_layouts:&[Some(&layout)],immediate_size:0});
        let pipeline=device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor{label:Some("NoirRAW common preview/export pipeline"),layout:Some(&pipe_layout),module:&shader,entry_point:Some("apply"),compilation_options:wgpu::PipelineCompilationOptions::default(),cache:None});
        let data=vec![1.0f32;2*2*2*4];
        let empty_lut=upload_float_texture(&device,&queue,"identity LUT (mode 0)",2,2,2,&data,wgpu::TextureDimension::D3);
        Self{device,queue,pipeline,layout,input:None,lut:None,output:None,empty_lut,lut_upload_count:0,scene_upload_count:0,render_count:0}
    }
    fn uploaded_input(&mut self,image:&SceneImage,id:u64)->wgpu::Texture{
        if let Some(c)=&self.input {if c.id==id&&c.width==image.width&&c.height==image.height{return c.texture.clone()}}
        let tex=upload_float_texture(&self.device,&self.queue,"working scene-linear Rec2020",image.width,image.height,1,bytemuck::cast_slice::<[f32;4],f32>(&image.pixels),wgpu::TextureDimension::D2);
        self.scene_upload_count+=1;
        self.input=Some(CachedInput{id,texture:tex.clone(),width:image.width,height:image.height});tex
    }
    fn uploaded_lut(&mut self,cube:Option<(&PathBuf,&Cube)>)->(wgpu::Texture,u32){
        let Some((path,cube))=cube else {return (self.empty_lut.clone(),0)};
        if let Some(c)=&self.lut {if &c.path==path&&c.size==cube.size{return (c.texture.clone(),cube.size)}}
        let texture=upload_float_texture(&self.device,&self.queue,"selected film and paper LUT",cube.size,cube.size,cube.size,bytemuck::cast_slice::<[f32;4],f32>(&cube.data),wgpu::TextureDimension::D3);
        self.lut_upload_count+=1;self.lut=Some(CachedLut{path:path.clone(),texture:texture.clone(),size:cube.size});(texture,cube.size)
    }
    fn make_output(&self,width:u32,height:u32)->wgpu::Texture{
        self.device.create_texture(&wgpu::TextureDescriptor{label:Some("NoirRAW GPU processed image"),size:wgpu::Extent3d{width,height,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba8Unorm,usage:wgpu::TextureUsages::STORAGE_BINDING|wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_SRC|wgpu::TextureUsages::RENDER_ATTACHMENT,view_formats:&[]})
    }
    fn dispatch(&mut self,src:&wgpu::Texture,dst:&wgpu::Texture,cube:Option<(&PathBuf,&Cube,InputSpace)>,controls:Controls){
        let (tex,_) = self.uploaded_lut(cube.as_ref().map(|(p,c,_)|(*p,*c)));
        let mode = cube.as_ref().map(|(_,_,s)|match s{InputSpace::Srgb=>1.,InputSpace::Rec2020LinearSignedLog=>2.}).unwrap_or(0.);
        let (min,span)=cube.as_ref().map(|(_,c,_)|(c.min,[c.max[0]-c.min[0],c.max[1]-c.min[1],c.max[2]-c.min[2]])).unwrap_or(([0.;3],[1.;3]));
        let params=ShaderParams{controls:[controls.exposure,controls.contrast,controls.strength.clamp(0.,1.),mode],domain_min:[min[0],min[1],min[2],0.],domain_span:[span[0],span[1],span[2],1.]};
        let uniform=self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor{label:Some("edit uniforms"),contents:bytemuck::bytes_of(&params),usage:wgpu::BufferUsages::UNIFORM});
        let bind=self.device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("film pipeline binding"),layout:&self.layout,entries:&[
            wgpu::BindGroupEntry{binding:0,resource:wgpu::BindingResource::TextureView(&src.create_view(&Default::default()))},
            wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::TextureView(&tex.create_view(&Default::default()))},
            wgpu::BindGroupEntry{binding:2,resource:uniform.as_entire_binding()},
            wgpu::BindGroupEntry{binding:3,resource:wgpu::BindingResource::TextureView(&dst.create_view(&Default::default()))},
        ]});
        let mut encoder=self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor{label:Some("film edit")});
        {let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("LUT + exposure + contrast"),timestamp_writes:None});pass.set_pipeline(&self.pipeline);pass.set_bind_group(0,&bind,&[]);pass.dispatch_workgroups(dst.width().div_ceil(8),dst.height().div_ceil(8),1);}
        self.queue.submit(Some(encoder.finish()));self.render_count+=1;
    }
    /// Only call after the source/parameters change. Returns a new texture only
    /// when dimensions change; otherwise the very same GPU output is overwritten.
    pub fn preview(&mut self,img:&SceneImage,id:u64,cube:Option<(&PathBuf,&Cube,InputSpace)>,controls:Controls)->(wgpu::Texture,bool){
        let src=self.uploaded_input(img,id);
        let new_size=self.output.as_ref().map_or(true,|v|v.width!=img.width||v.height!=img.height);
        if new_size {self.output=Some(Output{texture:self.make_output(img.width,img.height),width:img.width,height:img.height});}
        let dst=self.output.as_ref().unwrap().texture.clone();
        self.dispatch(&src,&dst,cube,controls);
        (dst,new_size)
    }
    /// Only explicit export reads pixels back. Full scene is uploaded once per
    /// export; original resolution and the same shader/controls are retained.
    pub fn export(&mut self,img:&SceneImage,cube:Option<(&PathBuf,&Cube,InputSpace)>,controls:Controls)->Result<Vec<u8>,String>{
        let src=upload_float_texture(&self.device,&self.queue,"full-resolution input",img.width,img.height,1,bytemuck::cast_slice::<[f32;4],f32>(&img.pixels),wgpu::TextureDimension::D2);
        let dst=self.make_output(img.width,img.height);
        self.dispatch(&src,&dst,cube,controls);
        let row_bytes=img.width*4;let padded=row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)*wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let len=padded as u64*img.height as u64;
        let readback=self.device.create_buffer(&wgpu::BufferDescriptor{label:Some("export only readback"),size:len,usage:wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::MAP_READ,mapped_at_creation:false});
        let mut enc=self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor{label:Some("export readback")});
        enc.copy_texture_to_buffer(wgpu::TexelCopyTextureInfo{texture:&dst,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},wgpu::TexelCopyBufferInfo{buffer:&readback,layout:wgpu::TexelCopyBufferLayout{offset:0,bytes_per_row:Some(padded),rows_per_image:Some(img.height)}},wgpu::Extent3d{width:img.width,height:img.height,depth_or_array_layers:1});
        self.queue.submit(Some(enc.finish()));
        let (tx,rx)=mpsc::sync_channel(1);readback.slice(..).map_async(wgpu::MapMode::Read,move |result|{let _=tx.send(result);});
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e|format!("GPU readback fence: {e:?}"))?;
        rx.recv().map_err(|e|e.to_string())?.map_err(|e|format!("GPU map: {e:?}"))?;
        let mapped=readback.slice(..).get_mapped_range().map_err(|e|format!("GPU map range: {e}"))?;let mut result=Vec::with_capacity((img.width*img.height*4) as usize);
        for row in 0..img.height as usize{let start=row*padded as usize;result.extend_from_slice(&mapped[start..start+row_bytes as usize]);}
        drop(mapped);readback.unmap();Ok(result)
    }
}
fn upload_float_texture(device:&wgpu::Device,queue:&wgpu::Queue,label:&str,width:u32,height:u32,depth:u32,pixels:&[f32],dimension:wgpu::TextureDimension)->wgpu::Texture{
    device.create_texture_with_data(queue,&wgpu::TextureDescriptor{label:Some(label),size:wgpu::Extent3d{width,height,depth_or_array_layers:depth},mip_level_count:1,sample_count:1,dimension,format:wgpu::TextureFormat::Rgba32Float,usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_DST,view_formats:&[]},wgpu::util::TextureDataOrder::LayerMajor,bytemuck::cast_slice(pixels))
}

/// Runs without a UI, including on GitHub Actions macOS Intel.
/// Verifies pixel parity, GPU texture residency and that slider updates do NOT rebake LUTs.
pub fn smoke_gpu()->Result<(),String>{
    let instance=wgpu::Instance::default();
    let adapter=pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions{
        power_preference:wgpu::PowerPreference::HighPerformance,
        compatible_surface:None,
        force_fallback_adapter:false,
        apply_limit_buckets:false,
    })).map_err(|e|format!("GPU adapter missing: {e:?}"))?;
    let info=adapter.get_info();
    #[cfg(target_os="macos")]
    if info.backend!=wgpu::Backend::Metal{return Err(format!("Expected Metal, got {:?}",info.backend))}
    let (device,queue)=pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e|format!("GPU device: {e:?}"))?;
    let mut gpu=GpuEngine::new(device,queue);
    let img=SceneImage{width:16,height:12,pixels:vec![[0.18,0.18,0.18,1.];16*12]};
    let cube=Cube{size:2,data:vec![
        [0.,0.,0.,1.],[1.,0.,0.,1.],[0.,1.,0.,1.],[1.,1.,0.,1.],
        [0.,0.,1.,1.],[1.,0.,1.,1.],[0.,1.,1.,1.],[1.,1.,1.,1.]
    ],min:[0.;3],max:[1.;3],title:"identity RGB".into()};
    let p=PathBuf::from("synthetic-identity.cube");
    let first=gpu.export(&img,Some((&p,&cube,InputSpace::Srgb)),Controls::default())?;
    if first.len()!=16*12*4{return Err("Export dimensions incorrect".into())}
    let expected=crate::color::rec2020_to_srgb_encoded([0.18,0.18,0.18]);
    for c in 0..3 {let actual=first[c] as f32/255.0;
        if (actual-expected[c]).abs()>0.015{return Err(format!("Color parity mismatch at {c}: {actual} vs {}",expected[c]))}
    }
    let count=gpu.lut_upload_count;
    let exposure=Controls{exposure:1.0,..Controls::default()};
    let second=gpu.export(&img,Some((&p,&cube,InputSpace::Srgb)),exposure)?;
    if gpu.lut_upload_count!=count{return Err("Slider exposure caused LUT reupload".into())}
    if second[0]<=first[0]{return Err("Exposure change did not affect the exported pixels".into())}
    let linear_cube=Cube{size:2,data:cube.data.clone(),min:[0.;3],max:[1.;3],title:"signed log".into()};
    let linear_path=PathBuf::from("synthetic-linear.cube");
    let linear=gpu.export(&img,Some((&linear_path,&linear_cube,InputSpace::Rec2020LinearSignedLog)),Controls::default())?;
    if linear==first{return Err("Scene-linear signed-log LUT was treated as sRGB".into())}
    println!("GPU_SMOKE_PASS backend={:?} adapter={} renders={} uploads={}",info.backend,info.name,gpu.render_count,gpu.lut_upload_count);
    Ok(())
}
