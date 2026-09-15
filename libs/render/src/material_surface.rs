//! Portable material pixels prepared on workers, including explicit mip data.
use makepad_draw::{ImageBuffer, makepad_platform::{Cx,Texture,TextureFormat,TextureUpdated,TextureWrap}};

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialSurface {
    pub fur: Option<makepad_gltf::GlbFurMaterial>,
    pub normal_png: Option<Vec<u8>>,
    pub normal_scale: f32,
    pub occlusion_png: Option<Vec<u8>>,
    pub occlusion_strength: f32,
    pub emissive_png: Option<Vec<u8>>,
    pub emissive: [f32;3],
    /// 0 opaque, 1 mask, 2 blend.
    pub alpha_mode: u8,
    pub alpha_cutoff: f32,
    pub base_alpha: f32,
    pub double_sided: bool,
}
impl Default for MaterialSurface {
    fn default()->Self {Self{fur:None,normal_png:None,normal_scale:1.0,occlusion_png:None,occlusion_strength:1.0,
        emissive_png:None,emissive:[0.0;3],alpha_mode:0,alpha_cutoff:0.5,base_alpha:1.0,double_sided:false}}
}

pub(crate) fn fur_params(fur: Option<makepad_gltf::GlbFurMaterial>) -> makepad_draw::Vec4f {
    fur.filter(|fur| fur.valid()).map_or_else(Default::default, |fur| {
        makepad_draw::vec4(fur.length as f32, fur.density as f32, fur.scale as f32, fur.seed as f32)
    })
}

/// Fixed layer and triangle ceilings apply on every platform, including Quest.
/// The source mesh stays resident once; only draw instances are repeated.
pub(crate) fn fur_shell_count(length: f32, transform: &makepad_draw::Mat4f, distance: f32, triangles: usize, budget: &mut usize) -> usize {
    if length <= 0.0 || triangles == 0 || !distance.is_finite() { return 0; }
    // Account for model scaling, including miniature construction previews.
    // This angular-size heuristic is independent of the headset eye: both
    // eyes use the scene camera and retain the same number of layers.
    let scale = [0, 4, 8].into_iter().map(|i| {
        (transform.v[i].powi(2) + transform.v[i + 1].powi(2) + transform.v[i + 2].powi(2)).sqrt()
    }).fold(0.0f32, f32::max);
    let detail = length * scale * 800.0 / distance.max(0.1);
    if !detail.is_finite() { return 0; }
    let desired = if detail < 1.0 { 0 } else if detail < 3.0 { 2 } else if detail < 6.0 { 4 } else { 6 };
    let shells = desired.min(*budget / triangles);
    *budget -= shells * triangles;
    shells
}

#[derive(Clone,Copy)]
pub enum PixelSemantic { Color, Data, Normal }

#[derive(Clone)]
pub struct PreparedTexture {pub width:usize,pub height:usize,pub data:Vec<u32>,pub max_level:usize}
fn linear(v:f32)->f32{if v<=0.04045{v/12.92}else{((v+0.055)/1.055).powf(2.4)}}
fn srgb(v:f32)->f32{if v<=0.0031308{v*12.92}else{1.055*v.max(0.0).powf(1.0/2.4)-0.055}}
impl PreparedTexture {
    pub fn prepare(image:ImageBuffer,semantic:PixelSemantic)->Self {
        let (width,height)=(image.width,image.height);let mut data=image.data;
        let(mut w,mut h,mut start,mut max_level)=(width,height,0usize,0usize);
        while w>1||h>1 {
            let(nw,nh)=((w/2).max(1),(h/2).max(1));let offset=data.len();
            for y in 0..nh {for x in 0..nw {
                let mut sum=[0.0f32;4];
                for(dy,dx)in[(0,0),(0,1),(1,0),(1,1)] {
                    let pixel=data[start+(y*2+dy).min(h-1)*w+(x*2+dx).min(w-1)];
                    let mut c=[((pixel>>16)&255)as f32/255.0,((pixel>>8)&255)as f32/255.0,(pixel&255)as f32/255.0,(pixel>>24)as f32/255.0];
                    match semantic {PixelSemantic::Color=>{for i in 0..3{c[i]=linear(c[i])*c[3];}},PixelSemantic::Normal=>{for i in 0..3{c[i]=c[i]*2.0-1.0;}},PixelSemantic::Data=>{}}
                    for i in 0..4{sum[i]+=c[i]*0.25;}
                }
                match semantic {
                    PixelSemantic::Color=>{for i in 0..3{sum[i]=srgb(if sum[3]>1e-8{sum[i]/sum[3]}else{0.0});}},
                    PixelSemantic::Normal=>{let length=(sum[0]*sum[0]+sum[1]*sum[1]+sum[2]*sum[2]).sqrt();for i in 0..3{sum[i]=if length>1e-8{sum[i]/length*0.5+0.5}else{if i==2{1.0}else{0.5}};}},
                    PixelSemantic::Data=>{},
                }
                let byte=|v:f32|(v.clamp(0.0,1.0)*255.0+0.5)as u32;
                data.push(byte(sum[3])<<24|byte(sum[0])<<16|byte(sum[1])<<8|byte(sum[2]));
            }}
            start=offset;w=nw;h=nh;max_level+=1;
        }
        Self{width,height,data,max_level}
    }
    pub fn upload(self,cx:&mut Cx)->Texture {
        Texture::new_with_format(cx,TextureFormat::VecMipBGRAu8_32{width:self.width,height:self.height,data:Some(self.data),max_level:Some(self.max_level),wrap:TextureWrap::Repeat,updated:TextureUpdated::Full})
    }
    pub fn bytes(&self)->usize{self.data.len()*4}
}

#[derive(Clone)]
pub struct PreparedSurface {pub definition:MaterialSurface,pub normal:PreparedTexture,pub occlusion:PreparedTexture,pub emissive:PreparedTexture}
#[derive(Clone)]
pub struct UploadedSurface {pub definition:std::sync::Arc<MaterialSurface>,pub normal:Texture,pub occlusion:Texture,pub emissive:Texture}
impl PreparedSurface {
    pub fn prepare(mut definition:MaterialSurface,remaining:&mut usize)->Result<Self,String>{
        let mut image=|bytes:Option<Vec<u8>>,fallback,semantic|->Result<PreparedTexture,String>{
            let image=if let Some(bytes)=bytes{crate::renderer::decode_generated_png(&bytes,4096,*remaining)?}else{
                let mut image=ImageBuffer::default();image.width=1;image.height=1;image.data=vec![fallback];image};
            let texture=PreparedTexture::prepare(image,semantic);
            *remaining=remaining.checked_sub(texture.bytes()).ok_or("material maps exceed texture byte budget")?;Ok(texture)
        };
        let normal=image(definition.normal_png.take(),0xff80_80ff,PixelSemantic::Normal)?;
        let occlusion=image(definition.occlusion_png.take(),0xffff_ffff,PixelSemantic::Data)?;
        let emissive=image(definition.emissive_png.take(),0xffff_ffff,PixelSemantic::Color)?;
        Ok(Self{definition,normal,occlusion,emissive})
    }
    pub fn bytes(&self)->usize{self.normal.bytes()+self.occlusion.bytes()+self.emissive.bytes()}
    pub fn upload(self,cx:&mut Cx)->UploadedSurface {UploadedSurface{definition:std::sync::Arc::new(self.definition),normal:self.normal.upload(cx),occlusion:self.occlusion.upload(cx),emissive:self.emissive.upload(cx)}}
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn color_mips_filter_linear_premultiplied_pixels(){
        let mut image=ImageBuffer::default();image.width=2;image.height=1;image.data=vec![0xffff_ffff,0xff00_0000];
        let texture=PreparedTexture::prepare(image,PixelSemantic::Color);let r=(texture.data[2]>>16)&255;assert!((187..=189).contains(&r));
        let mut image=ImageBuffer::default();image.width=2;image.height=1;image.data=vec![0xffff_0000,0x0000_00ff];
        let texture=PreparedTexture::prepare(image,PixelSemantic::Color);assert_eq!(texture.data[2]&0x00ff_ffff,0x00ff_0000);assert_eq!(texture.data[2]>>24,128);
    }
}
