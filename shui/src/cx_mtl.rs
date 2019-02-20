use std::mem;
use std::ffi::CStr;

use cocoa::base::id as cocoa_id;
use cocoa::foundation::{NSAutoreleasePool,NSUInteger};
use cocoa::appkit::{NSWindow, NSView};
use core_graphics::geometry::CGSize;
use objc::runtime::YES;
use objc::{msg_send, sel, sel_impl};
use metal::*;
use winit::os::macos::WindowExt;

use crate::shader::*;
use crate::cxdrawing::*;
use crate::cxshaders::*;
use crate::cxfonts::*;
use crate::cxtextures::*;
use crate::cxturtle::*;

#[derive(Clone)]
pub struct Cx{
    pub title:String,
    pub running:bool,

    pub turtle:CxTurtle,
    pub shaders:CxShaders,
    pub drawing:CxDrawing,
    pub fonts:CxFonts,
    pub textures:CxTextures,

    pub uniforms:Vec<f32>,
    pub buffers:CxBuffers
}

impl Default for Cx{
    fn default()->Self{
        let mut uniforms = Vec::<f32>::new();
        uniforms.resize(CX_UNI_SIZE, 0.0);
        Self{
            turtle:CxTurtle{..Default::default()},
            fonts:CxFonts{..Default::default()},
            drawing:CxDrawing{..Default::default()},
            shaders:CxShaders{..Default::default()},
            textures:CxTextures{..Default::default()},
            title:"Hello World".to_string(),
            running:true,
            uniforms:uniforms,
            buffers:CxBuffers{..Default::default()}
        }
    }
}

const CX_UNI_PROP1:usize = 0;
const CX_UNI_SIZE:usize = 1;

impl Cx{
    pub fn def_shader(sh:&mut Shader){
        Shader::def_df(sh);
        Shader::def_builtins(sh);
        Cx::def_uniforms(sh);
        DrawList::def_uniforms(sh);
    }

    pub fn def_uniforms(_sh: &mut Shader){
        //sh.cx_uniform("prop1", Kind::Float);
    }

    pub fn uniform_prop1(&mut self, v:f32){
        self.uniforms[CX_UNI_PROP1] = v;
    }

    pub fn exec_draw_list(&mut self, id: usize, device:&Device, encoder:&RenderCommandEncoderRef){
        
        // update draw list uniforms
        {
            let draw_list = &mut self.drawing.draw_lists[id];
            draw_list.buffers.uni_dl.update_with_f32_data(device, &draw_list.uniforms);
        }
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        for ci in 0..self.drawing.draw_lists[id].draws_len{
            let sub_list_id = self.drawing.draw_lists[id].draws[ci].sub_list_id;
            if sub_list_id != 0{
                self.exec_draw_list(sub_list_id, device, encoder);
            }
            else{
                let draw_list = &mut self.drawing.draw_lists[id];
                let draw = &mut draw_list.draws[ci];

                let sh = &self.shaders.shaders[draw.shader_id];
                let shc = &self.shaders.compiled_shaders[draw.shader_id];
                
                if draw.update_frame_id == self.drawing.frame_id{
                    // update the instance buffer data
                    draw.buffers.inst_vbuf.update_with_f32_data(device, &draw.instance);
                    draw.buffers.uni_dr.update_with_f32_data(device, &draw.uniforms);
                }

                let instances = (draw.instance.len() / shc.assembled_shader.instance_slots) as u64;
                if let Some(pipeline_state) = &shc.pipeline_state{
                    encoder.set_render_pipeline_state(pipeline_state);
                    if let Some(buf) = &shc.geom_vbuf.buffer{encoder.set_vertex_buffer(0, Some(&buf), 0);}
                    else{println!("Drawing error: geom_vbuf None")}
                    if let Some(buf) = &draw.buffers.inst_vbuf.buffer{encoder.set_vertex_buffer(1, Some(&buf), 0);}
                    else{println!("Drawing error: inst_vbuf None")}
                    if let Some(buf) = &self.buffers.uni_cx.buffer{encoder.set_vertex_buffer(2, Some(&buf), 0);}
                    else{println!("Drawing error: uni_cx None")}
                    if let Some(buf) = &draw_list.buffers.uni_dl.buffer{encoder.set_vertex_buffer(3, Some(&buf), 0);}
                    else{println!("Drawing error: uni_dl None")}
                    if let Some(buf) = &draw.buffers.uni_dr.buffer{encoder.set_vertex_buffer(4, Some(&buf), 0);}
                    else{println!("Drawing error: uni_dr None")}

                    if let Some(buf) = &self.buffers.uni_cx.buffer{encoder.set_fragment_buffer(0, Some(&buf), 0);}
                    else{println!("Drawing error: uni_cx None")}
                    if let Some(buf) = &draw_list.buffers.uni_dl.buffer{encoder.set_fragment_buffer(1, Some(&buf), 0);}
                    else{println!("Drawing error: uni_dl None")}
                    if let Some(buf) = &draw.buffers.uni_dr.buffer{encoder.set_fragment_buffer(2, Some(&buf), 0);}
                    else{println!("Drawing error: uni_dr None")}

                    // lets set our textures
                    for (i, texture_id) in draw.textures.iter().enumerate(){
                        let tex = &mut self.textures.textures[*texture_id];
                        if tex.dirty{
                            tex.upload_to_device(device);
                        }
                        if let Some(mtltex) = &tex.mtltexture{
                            encoder.set_fragment_texture(i as NSUInteger, Some(&mtltex));
                            encoder.set_vertex_texture(i as NSUInteger, Some(&mtltex));
                        }
                    }

                    if let Some(buf) = &shc.geom_ibuf.buffer{
                        encoder.draw_indexed_primitives_instanced(
                            MTLPrimitiveType::Triangle,
                            sh.geometry_indices.len() as u64, // Index Count
                            MTLIndexType::UInt32, // indexType,
                            &buf, // index buffer
                            0, // index buffer offset
                            instances, // instance count
                        )
                   }
                    else{println!("Drawing error: geom_ibuf None")}
                }
            }
        }
    }

    pub unsafe fn gl_string(raw_string: *const gl::types::GLubyte) -> String {
        if raw_string.is_null() { return "(NULL)".into() }
        String::from_utf8(CStr::from_ptr(raw_string as *const _).to_bytes().to_vec()).ok()
                                    .expect("gl_string: non-UTF8 string")
    }

    pub fn event_loop<F>(&mut self, mut callback:F)
    where F: FnMut(&mut Cx, Ev),
    { 

        let mut events_loop = winit::EventsLoop::new();
        let glutin_window = winit::WindowBuilder::new()
            .with_dimensions((800, 600).into())
            .with_title(self.title.clone())
            .build(&events_loop).unwrap();

        let window: cocoa_id = unsafe { mem::transmute(glutin_window.get_nswindow()) };
        let device = Device::system_default();

        let layer = CoreAnimationLayer::new();
        layer.set_device(&device);
        layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        layer.set_presents_with_transaction(false);

        unsafe {
            let view = window.contentView();
            view.setWantsBestResolutionOpenGLSurface_(YES);
            view.setWantsLayer(YES);
            view.setLayer(mem::transmute(layer.as_ref()));
        }

        let draw_size = glutin_window.get_inner_size().unwrap();
        layer.set_drawable_size(CGSize::new(draw_size.width as f64, draw_size.height as f64));

        let command_queue = device.new_command_queue();

        let mut pool = unsafe { NSAutoreleasePool::new(cocoa::base::nil) };
        
        self.shaders.compile_all_shaders(&device);

        while self.running {

            events_loop.poll_events(|event|{
                match event{
                    winit::Event::WindowEvent{ event, .. } => match event {
                        winit::WindowEvent::CloseRequested => self.running = false,
                        winit::WindowEvent::Resized(logical_size) => {
                            let dpi_factor = glutin_window.get_hidpi_factor();
                            let draw_size = logical_size.to_physical(dpi_factor);
                            layer.set_drawable_size(
                               CGSize::new(draw_size.width as f64, draw_size.height as f64));
                        },
                        _ => ()
                    },
                    _ => ()
                }
            });
            
            callback(self, Ev::Redraw);

            if let Some(drawable) = layer.next_drawable() {
                let render_pass_descriptor = RenderPassDescriptor::new();

                let color_attachment = render_pass_descriptor.color_attachments().object_at(0).unwrap();
                color_attachment.set_texture(Some(drawable.texture()));
                color_attachment.set_load_action(MTLLoadAction::Clear);
                color_attachment.set_clear_color(MTLClearColor::new(0.3, 0.3, 0.3, 1.0));
                color_attachment.set_store_action(MTLStoreAction::Store);

                let command_buffer = command_queue.new_command_buffer();
 
                render_pass_descriptor.color_attachments().object_at(0).unwrap().set_load_action(MTLLoadAction::Clear);

                let parallel_encoder = command_buffer.new_parallel_render_command_encoder(&render_pass_descriptor);
                let encoder = parallel_encoder.render_command_encoder();

                self.buffers.uni_cx.update_with_f32_data(&device, &self.uniforms);

                // ok now we should call our render thing
                self.exec_draw_list(0, &device, encoder);

                encoder.end_encoding();
                parallel_encoder.end_encoding();

                command_buffer.present_drawable(&drawable);
                command_buffer.commit();

                unsafe { 
                    msg_send![pool, release];
                    pool = NSAutoreleasePool::new(cocoa::base::nil);
                }
            }
        }
    }
  
}
