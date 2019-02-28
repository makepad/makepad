use std::mem;

use cocoa::base::id as cocoa_id;
use cocoa::foundation::{NSAutoreleasePool,NSUInteger};
use cocoa::appkit::{NSWindow, NSView};
use core_graphics::geometry::CGSize;
use objc::runtime::YES;
use objc::{msg_send, sel, sel_impl};
use metal::*;
use winit::os::macos::WindowExt;
use time::precise_time_ns;

use crate::cx_shared::*;
use crate::cx_winit::*;
use crate::events::*;
use crate::area::*;
use crate::cxshaders::*;

impl Cx{

    pub fn exec_draw_list(&mut self, draw_list_id: usize, device:&Device, encoder:&RenderCommandEncoderRef){
        
        // update draw list uniforms
        {
            let draw_list = &mut self.drawing.draw_lists[draw_list_id];
            draw_list.resources.uni_dl.update_with_f32_data(device, &draw_list.uniforms);
        }
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.drawing.draw_lists[draw_list_id].draw_calls_len;
        for draw_call_id in 0..draw_calls_len{
            let sub_list_id = self.drawing.draw_lists[draw_list_id].draw_calls[draw_call_id].sub_list_id;
            if sub_list_id != 0{
                self.exec_draw_list(sub_list_id, device, encoder);
            }
            else{
                let draw_list = &mut self.drawing.draw_lists[draw_list_id];
                let draw = &mut draw_list.draw_calls[draw_call_id];

                let sh = &self.shaders.shaders[draw.shader_id];
                let shc = &self.shaders.compiled_shaders[draw.shader_id];
                
                if draw.update_frame_id == self.drawing.frame_id{
                    // update the instance buffer data
                    draw.resources.inst_vbuf.update_with_f32_data(device, &draw.instance);
                    draw.resources.uni_dr.update_with_f32_data(device, &draw.uniforms);
                }

                let instances = (draw.instance.len() / shc.instance_slots
                ) as u64;
                if let Some(pipeline_state) = &shc.pipeline_state{
                    encoder.set_render_pipeline_state(pipeline_state);
                    if let Some(buf) = &shc.geom_vbuf.buffer{encoder.set_vertex_buffer(0, Some(&buf), 0);}
                    else{println!("Drawing error: geom_vbuf None")}
                    if let Some(buf) = &draw.resources.inst_vbuf.buffer{encoder.set_vertex_buffer(1, Some(&buf), 0);}
                    else{println!("Drawing error: inst_vbuf None")}
                    if let Some(buf) = &self.resources.uni_cx.buffer{encoder.set_vertex_buffer(2, Some(&buf), 0);}
                    else{println!("Drawing error: uni_cx None")}
                    if let Some(buf) = &draw_list.resources.uni_dl.buffer{encoder.set_vertex_buffer(3, Some(&buf), 0);}
                    else{println!("Drawing error: uni_dl None")}
                    if let Some(buf) = &draw.resources.uni_dr.buffer{encoder.set_vertex_buffer(4, Some(&buf), 0);}
                    else{println!("Drawing error: uni_dr None")}

                    if let Some(buf) = &self.resources.uni_cx.buffer{encoder.set_fragment_buffer(0, Some(&buf), 0);}
                    else{println!("Drawing error: uni_cx None")}
                    if let Some(buf) = &draw_list.resources.uni_dl.buffer{encoder.set_fragment_buffer(1, Some(&buf), 0);}
                    else{println!("Drawing error: uni_dl None")}
                    if let Some(buf) = &draw.resources.uni_dr.buffer{encoder.set_fragment_buffer(2, Some(&buf), 0);}
                    else{println!("Drawing error: uni_dr None")}

                    // lets set our textures
                    for (i, texture_id) in draw.textures.iter().enumerate(){
                        let tex = &mut self.textures.textures[*texture_id as usize];
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
 
    pub fn repaint(&mut self,layer:&CoreAnimationLayer, device:&Device, command_queue:&CommandQueue){
        let pool = unsafe { NSAutoreleasePool::new(cocoa::base::nil) };

        if let Some(drawable) = layer.next_drawable() {
            self.prepare_frame();
            
            let render_pass_descriptor = RenderPassDescriptor::new();

            let color_attachment = render_pass_descriptor.color_attachments().object_at(0).unwrap();
            color_attachment.set_texture(Some(drawable.texture()));
            color_attachment.set_load_action(MTLLoadAction::Clear);
            color_attachment.set_clear_color(MTLClearColor::new(
                self.clear_color.x as f64, self.clear_color.y as f64, self.clear_color.z as f64, self.clear_color.w as f64
            ));
            color_attachment.set_store_action(MTLStoreAction::Store);

            let command_buffer = command_queue.new_command_buffer();

            render_pass_descriptor.color_attachments().object_at(0).unwrap().set_load_action(MTLLoadAction::Clear);

            let parallel_encoder = command_buffer.new_parallel_render_command_encoder(&render_pass_descriptor);
            let encoder = parallel_encoder.render_command_encoder();

            self.resources.uni_cx.update_with_f32_data(&device, &self.uniforms);

            // ok now we should call our render thing
            self.exec_draw_list(0, &device, encoder);

            encoder.end_encoding();
            parallel_encoder.end_encoding();

            command_buffer.present_drawable(&drawable);
            command_buffer.commit();
        
            unsafe { 
                msg_send![pool, release];
                //pool = NSAutoreleasePool::new(cocoa::base::nil);
            }
        }
    }

    fn resize_layer_to_turtle(&mut self, layer:&CoreAnimationLayer){
        // resize drawable
        layer.set_drawable_size(CGSize::new(
            (self.turtle.target_size.x * self.turtle.target_dpi_factor) as f64,
             (self.turtle.target_size.y * self.turtle.target_dpi_factor) as f64));
    }

    pub fn event_loop<F>(&mut self, mut event_handler:F)
    where F: FnMut(&mut Cx, Event),
    { 

        let mut events_loop = winit::EventsLoop::new();
        let glutin_window = winit::WindowBuilder::new()
            .with_dimensions((800, 600).into())
            .with_title(format!("Metal - {}",self.title))
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

        glutin_window.set_position(winit::dpi::LogicalPosition::new(1920.0,400.0));
        
        self.shaders.compile_all_mtl_shaders(&device);

        self.load_binary_deps_from_file();

        let start_time = precise_time_ns();
        
        event_handler(self, Event::Init); 

        while self.running{
            // unfortunate duplication of code between poll and run_forever but i don't know how to put this in a closure
            // without borrowchecker hell
            events_loop.poll_events(|winit_event|{
                let event = self.map_winit_event(winit_event, &glutin_window);
                if let Event::Resized(_) = &event{ // do this here because mac
                    self.resize_layer_to_turtle(&layer);
                    event_handler(self, event); 
                    self.redraw_area = Some(Area::zero());
                    self.redraw_none();
                    event_handler(self, Event::Redraw);
                    self.repaint(&layer, &device, &command_queue);
                }
                else{
                    event_handler(self, event); 
                }
            });
            if self.animations.len() != 0{
                let time_now = precise_time_ns();
                let time = (time_now - start_time) as f64 / 1_000_000_000.0; // keeps the error as low as possible
                event_handler(self, Event::Animate(AnimateEvent{time:time}));
                self.check_ended_animations(time);
                if self.ended_animations.len() > 0{
                    event_handler(self, Event::AnimationEnded(AnimateEvent{time:time}));
                }
            }
            // call redraw event
            if let Some(_) = &self.redraw_dirty{
                self.redraw_area = self.redraw_dirty.clone();
                self.redraw_none();
                self.frame_id += 1;
                event_handler(self, Event::Redraw);
                self.paint_dirty = true;
            }
            // repaint everything if we need to
            if self.paint_dirty{
                self.paint_dirty = false;
                self.repaint(&layer, &device, &command_queue);
            }
            
            // wait for the next event blockingly so it stops eating power
            if self.animations.len() == 0 && self.redraw_dirty.is_none(){
                events_loop.run_forever(|winit_event|{
                    let event = self.map_winit_event(winit_event, &glutin_window);
                    if let Event::Resized(_) = &event{ // do this here because mac
                        self.resize_layer_to_turtle(&layer);
                        event_handler(self, event); 
                        self.redraw_area = Some(Area::zero());
                        self.redraw_none();
                        event_handler(self, Event::Redraw);
                        self.repaint(&layer, &device, &command_queue);
                    }
                    else{
                        event_handler(self, event);
                    }
                    winit::ControlFlow::Break
                })
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct CxResources{
    pub uni_cx:MetalBuffer,
    pub winit:CxWinit
}

#[derive(Clone, Default)]
pub struct DrawListResources{
     pub uni_dl:MetalBuffer
}

#[derive(Default,Clone)]
pub struct DrawCallResources{
    pub uni_dr:MetalBuffer,
    pub inst_vbuf:MetalBuffer
}
