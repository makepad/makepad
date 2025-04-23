use crate::{
    makepad_derive_widget::*,
    image_cache::*,
    makepad_draw::*,
    widget::*
};

use std::path::{Path, PathBuf};

live_design!{
    link widgets;
    use link::shaders::*;
    
    DrawImage= {{DrawImage}} {
        texture image: texture2d
        opacity: 1.0
        image_scale: vec2(1.0, 1.0)
        image_pan: vec2(0.0, 0.0)
                
        fn get_color_scale_pan(self, scale: vec2, pan: vec2) -> vec4 {
            return sample2d(self.image, self.pos * scale + pan).xyzw;
        }
                                
        fn get_color(self) -> vec4 {
            return self.get_color_scale_pan(self.image_scale, self.image_pan)
        }
        
        fn pixel(self) -> vec4 {
            let color = mix(self.get_color(), #3, self.async_load);
            return Pal::premul(vec4(color.xyz, color.w * self.opacity))
        }
        
    }
    
    pub ImageBase = {{Image}} {}
    
    pub Image = <ImageBase> {
        animator: {
            async_load = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {async_load: 0.0}
                    }
                }
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                    }
                    apply: {
                        draw_bg: {async_load: 1.0}
                    }
                }
            }
        }
        
        width: 100
        height: 100
    }
    
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawImage {
    #[deref] draw_super: DrawQuad,
    #[live] pub opacity: f32,
    #[live] image_scale: Vec2,
    #[live] image_pan: Vec2,
    #[live] async_load: f32
}


#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum ImageAnimation {
    Stop,
    Once,
    #[pick] Loop,
    Bounce,
    #[live(0.0)] Frame(f64),
    #[live(0.0)] Factor(f64),
    #[live(60.0)] OnceFps(f64),
    #[live(60.0)] LoopFps(f64),
    #[live(60.0)] BounceFps(f64),
}

#[derive(Live, Widget)]
pub struct Image {
    #[walk] walk: Walk,
    #[animator] animator: Animator,
    #[redraw] #[live] pub draw_bg: DrawImage,
    #[live] min_width: i64,
    #[live] min_height: i64,
    #[live(1.0)] width_scale: f64,
    #[live(ImageAnimation::BounceFps(25.0))] animation: ImageAnimation,
    #[rust] last_time: Option<f64>,
    #[rust] animation_frame: f64,
    #[rust] next_frame: NextFrame,
    #[live] fit: ImageFit,
    #[live] source: LiveDependency,
    #[rust] async_image_path: Option<PathBuf>,
    #[rust] async_image_size: Option<(usize, usize)>,
    #[rust] texture: Option<Texture>,
}

impl ImageCacheImpl for Image {
    fn get_texture(&self, _id:usize) -> &Option<Texture> {
        &self.texture
    }
    
    fn set_texture(&mut self, texture: Option<Texture>, _id:usize) {
        self.texture = texture;
    }
}

impl LiveHook for Image{
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        match apply.from{
            ApplyFrom::NewFromDoc{..}|// newed from DSL,
            ApplyFrom:: UpdateFromDoc{..}|
            ApplyFrom::Over{..}=>{
                self.lazy_create_image_cache(cx);
                let source = self.source.clone();
                if source.as_str().len()>0 {
                    let _ = self.load_image_dep_by_path(cx, source.as_str(), 0);
                }
            }
            _=>()
        }
    }
}

impl Widget for Image {
    fn handle_event(&mut self, cx:&mut Cx, event:&Event, _scope:&mut Scope){
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        // lets check if we have a post action
        if let Event::Actions(actions) = &event{
            for action in actions{
                if let Some(AsyncImageLoad{image_path, result}) = &action.downcast_ref(){
                    if let Some(result) = result.borrow_mut().take(){
                        // we have a result for the image_cache to load up
                        self.process_async_image_load(cx, image_path, result);
                    }
                    if self.async_image_size.is_some() && self.async_image_path.clone() == Some(image_path.to_path_buf()){ // see if we can load from cache
                        self.load_image_from_cache(cx, image_path, 0);
                        self.async_image_size = None;
                        self.animator_play(cx, id!(async_load.off));
                        self.redraw(cx);
                    }
                }
            }
        }
        if let Some(nf) = self.next_frame.is_event(event) {
            // compute the next frame and patch things up
            if let Some(image_texture) = &self.texture {
                let (texture_width, texture_height) = image_texture.get_format(cx).vec_width_height().unwrap_or((self.min_width as usize, self.min_height as usize));
                if let Some(animation) = image_texture.animation(cx).clone(){
                    let delta = if let Some(last_time) = &self.last_time{
                        nf.time - last_time
                    }
                    else{
                        0.0
                    };
                    self.last_time = Some(nf.time);
                    let num_frames = animation.num_frames as f64;
                    match self.animation{
                        ImageAnimation::Stop=>{
                            
                        }
                        ImageAnimation::Frame(frame)=>{
                            self.animation_frame = frame;                                                   
                        }
                        ImageAnimation::Factor(pos)=>{
                            self.animation_frame = pos * (num_frames - 1.0);         
                        }
                        ImageAnimation::Once=>{
                            self.animation_frame += 1.0;
                            if self.animation_frame >= num_frames{
                                self.animation_frame = num_frames - 1.0;
                            }
                            else{
                                self.next_frame = cx.new_next_frame();
                            }
                        }
                        ImageAnimation::Loop=>{
                            self.animation_frame += 1.0;
                            if self.animation_frame >= num_frames{
                                self.animation_frame = 0.0;
                            }
                            self.next_frame = cx.new_next_frame();
                        }
                        ImageAnimation::Bounce=>{
                            self.animation_frame += 1.0;
                            if self.animation_frame >= num_frames * 2.0{
                                self.animation_frame = 0.0;
                            }
                            self.next_frame = cx.new_next_frame();
                        }
                        ImageAnimation::OnceFps(fps)=>{
                            self.animation_frame += delta * fps;
                            if self.animation_frame >= num_frames{
                                self.animation_frame = num_frames - 1.0;
                            }
                            else{
                                self.next_frame = cx.new_next_frame();
                            }
                        }
                        ImageAnimation::LoopFps(fps)=>{
                            self.animation_frame += delta * fps;
                            if self.animation_frame >= num_frames{
                                self.animation_frame = 0.0;
                            }
                            self.next_frame = cx.new_next_frame();
                        }
                        ImageAnimation::BounceFps(fps)=>{
                            self.animation_frame += delta * fps;
                            if self.animation_frame >= num_frames * 2.0{
                                self.animation_frame = 0.0;
                            }
                            self.next_frame = cx.new_next_frame();
                        }
                    }
                    // alright now lets turn animation_frame into the right image_pan
                    let last_pan = self.draw_bg.image_pan;
                    
                    let frame = if self.animation_frame >= num_frames{
                        num_frames * 2.0 - 1.0 - self.animation_frame
                    }
                    else{
                        self.animation_frame
                    } as usize;
                    
                    let horizontal_frames = texture_width / animation.width;
                    let xpos = ((frame % horizontal_frames) * animation.width) as f32 / texture_width as f32;
                    let ypos = ((frame / horizontal_frames) * animation.height) as f32 / texture_height as f32;
                    self.draw_bg.image_pan = vec2(xpos, ypos);
                    if self.draw_bg.image_pan != last_pan{
                        // patch it into the area
                        self.draw_bg.update_instance_area_value(cx, id!(image_pan))
                    }
                }
            }
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk)
    }
}

impl Image {
    /// Returns the original size of the image in pixels (not its displayed size).
    ///
    /// Returns `None` if the image has not been loaded into a texture yet.
    pub fn size_in_pixels(&self, cx: &mut Cx) -> Option<(usize, usize)> {
        self.texture.as_ref()
            .and_then(|t| t.get_format(cx).vec_width_height())
    }

    /// True if a texture has been set on this `Image`.
    pub fn has_texture(&self) -> bool {
        self.texture.is_some()
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, mut walk: Walk) -> DrawStep {
        // alright we get a walk. depending on our aspect ratio
        // we change either nothing, or width or height
        let rect = cx.peek_walk_turtle(walk);
        let dpi = cx.current_dpi_factor();
        
        let (width, height) = if let Some((w,h)) = &self.async_image_size{
            // still loading
            
            (*w as f64,*h as f64)
        }else if let Some(image_texture) = &self.texture {
            self.draw_bg.draw_vars.set_texture(0, image_texture);
            let (width,height) = image_texture.get_format(cx).vec_width_height().unwrap_or((self.min_width as usize, self.min_height as usize));
            if let Some(animation) = image_texture.animation(cx){
                let (w,h) = (animation.width as f64, animation.height as f64);
                self.next_frame = cx.new_next_frame();
                // we have an animation. lets compute the scale and zoom for a certain frame
                let scale_x = w as f32 / width as f32;
                let scale_y = h as f32 / height as f32;
                self.draw_bg.image_scale = vec2(scale_x, scale_y);
                (w,h)
            }
            else{
                self.draw_bg.image_scale = vec2(1.0,1.0);
                self.draw_bg.image_pan = vec2(0.0,0.0);
                (width as f64 * self.width_scale, height as f64)
            }
        }
        else {
            self.draw_bg.draw_vars.empty_texture(0);
            (self.min_width as f64 / dpi, self.min_height as f64 / dpi)
        };
        
        let aspect = width / height;
        match self.fit {
            ImageFit::Size => {
                walk.width = Size::Fixed(width);
                walk.height = Size::Fixed(height);
            }
            ImageFit::Stretch => {
            }
            ImageFit::Horizontal => {
                walk.height = Size::Fixed(rect.size.x / aspect);
            }
            ImageFit::Vertical => {
                walk.width = Size::Fixed(rect.size.y * aspect);
            }
            ImageFit::Smallest => {
                let walk_height = rect.size.x / aspect;
                if walk_height > rect.size.y {
                    walk.width = Size::Fixed(rect.size.y * aspect);
                }
                else {
                    walk.height = Size::Fixed(walk_height);
                }
            }
            ImageFit::Biggest => {
                let walk_height = rect.size.x / aspect;
                if walk_height < rect.size.y {
                    walk.width = Size::Fixed(rect.size.y * aspect);
                }
                else {
                    walk.height = Size::Fixed(walk_height);
                }
            }
        }
        
        
        self.draw_bg.draw_walk(cx, walk);
        
        DrawStep::done()
    }
    
    /// Loads the image at the given `image_path` on disk into this `ImageRef`.
    pub fn load_image_file_by_path_async(&mut self, cx: &mut Cx,  image_path: &Path) -> Result<(), ImageError> {
        if let Ok(result) = self.load_image_file_by_path_async_impl(cx, image_path, 0){
            match result{
                AsyncLoadResult::Loading(w,h)=>{
                    self.async_image_size = Some((w,h));
                    self.async_image_path = Some(image_path.into());
                    self.animator_play(cx, id!(async_load.on));
                    self.redraw(cx);
                }
                AsyncLoadResult::Loaded=>{
                    self.redraw(cx);
                }
            }
            // lets set the w-h
        }
        Ok(())
    }    
}

pub enum AsyncLoad{
    Yes,
    No
}

impl ImageRef {
    /// Loads the image at the given `image_path` resource into this `ImageRef`.
    pub fn load_image_dep_by_path(&self, cx: &mut Cx, image_path: &str) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_image_dep_by_path(cx, image_path, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }
    
    /// Loads the image at the given `image_path` on disk into this `ImageRef`.
    pub fn load_image_file_by_path(&self, cx: &mut Cx,  image_path: &Path) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_image_file_by_path(cx, image_path, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }
    
    /// Loads the image at the given `image_path` on disk into this `ImageRef`.
    pub fn load_image_file_by_path_async(&self, cx: &mut Cx,  image_path: &Path) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            return inner.load_image_file_by_path_async(cx, image_path)
        }
        Ok(())
    }    
    
    
    /// Loads a JPEG into this `ImageRef` by decoding the given encoded JPEG `data`.
    pub fn load_jpg_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_jpg_from_data(cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }
    
    /// Loads a PNG into this `ImageRef` by decoding the given encoded PNG `data`.
    pub fn load_png_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_png_from_data(cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }
    
    pub fn set_texture(&self, cx:&mut Cx, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.texture = texture;
            if cx.in_draw_event(){
                inner.redraw(cx);
            }
        }
    }
    
    pub fn set_uniform(&self, cx: &Cx, uniform: &[LiveId], value: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_bg.set_uniform(cx, uniform, value);
        }
    }

    /// See [`Image::size_in_pixels()`].
    pub fn size_in_pixels(&self, cx: &mut Cx) -> Option<(usize, usize)> {
        if let Some(inner) = self.borrow() {
            inner.size_in_pixels(cx)
        } else {
            None
        }
    }

    /// See [`Image::has_texture()`].
    pub fn has_texture(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.has_texture()
        } else {
            false
        }
    }
}

