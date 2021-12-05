use crate::cx_win32::*;
use crate::cx::*;

use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::{TRUE, FALSE};
use winapi::shared::{dxgi, dxgi1_2, dxgitype, dxgiformat, winerror};
use winapi::um::{d3d11, d3dcommon, d3dcompiler};
use winapi::Interface;
use wio::com::ComPtr;
use std::mem;
use std::ptr;
use std::ffi;

use makepad_live_compiler::generate_hlsl;
use makepad_live_compiler::analyse::ShaderCompileOptions;
use makepad_live_compiler::shaderast::ShaderAst;
//use std::ffi::c_void;
//use std::sync::Mutex;

impl Cx {
    
    pub fn render_view(
        &mut self,
        pass_id: usize,
        view_id: usize,
        scroll: Vec2,
        clip: (Vec2, Vec2),
        repaint_id: u64,
        d3d11_cx: &D3d11Cx,
        zbias: &mut f32,
        zbias_step: f32
    ) {
        // tad ugly otherwise the borrow checker locks 'self' and we can't recur
        let draw_calls_len = self.views[view_id].draw_calls_len;
        
        self.views[view_id].uniform_view_transform(&Mat4::identity());
        self.views[view_id].parent_scroll = scroll;
        
        let local_scroll = self.views[view_id].get_local_scroll();
        let clip = self.views[view_id].intersect_clip(clip);
        
        let cxview = &mut self.views[view_id];
        cxview.platform.view_uniforms.update_with_f32_constant_data(d3d11_cx, cxview.view_uniforms.as_slice());
        
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.render_view(
                    pass_id,
                    sub_view_id,
                    Vec2 {x: local_scroll.x + scroll.x, y: local_scroll.y + scroll.y},
                    clip,
                    repaint_id,
                    d3d11_cx,
                    zbias,
                    zbias_step
                );
            }
            else {
                let cxview = &mut self.views[view_id];
                
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader.shader_id];
                let shp = sh.platform.as_ref().unwrap();
                
                if draw_call.instance_dirty {
                    draw_call.instance_dirty = false;
                    if draw_call.instances.len() == 0 {
                        continue;
                    }
                    // update the instance buffer data
                    draw_call.platform.inst_vbuf.update_with_f32_vertex_data(d3d11_cx, &draw_call.instances);
                }
                
                // update the zbias uniform if we have it.
                draw_call.set_zbias(*zbias);
                draw_call.set_local_scroll(scroll, local_scroll);
                draw_call.set_clip(clip);
                *zbias += zbias_step;
                
                draw_call.platform.draw_uniforms.update_with_f32_constant_data(d3d11_cx, draw_call.draw_uniforms.as_slice());
                
                if draw_call.uniforms_dirty {
                    draw_call.uniforms_dirty = false;
                    if draw_call.user_uniforms.len() != 0 {
                        draw_call.platform.user_uniforms.update_with_f32_constant_data(d3d11_cx, &mut draw_call.user_uniforms);
                    }
                }
                
                let instances = (draw_call.instances.len() / sh.mapping.instance_props.total_slots) as usize;
                
                if instances == 0 {
                    continue;
                }
                
                let geometry_id = if let Some(geometry) = draw_call.geometry{
                    geometry.geometry_id
                }
                else if let Some(geometry) = sh.default_geometry{
                    geometry.geometry_id
                }
                else{
                    continue
                };
                
                let geometry = &mut self.geometries[geometry_id];
                
                if geometry.dirty {
                    geometry.platform.geom_ibuf.update_with_u32_index_data(d3d11_cx, &geometry.indices);
                    geometry.platform.geom_vbuf.update_with_f32_vertex_data(d3d11_cx, &geometry.vertices);
                    geometry.dirty = false;
                }
                
                d3d11_cx.set_shaders(&shp.vertex_shader, &shp.pixel_shader);
                
                d3d11_cx.set_primitive_topology();
                
                d3d11_cx.set_input_layout(&shp.input_layout);
                
                d3d11_cx.set_index_buffer(&geometry.platform.geom_ibuf);
                
                d3d11_cx.set_vertex_buffers(
                    &geometry.platform.geom_vbuf,
                    sh.mapping.geometry_props.total_slots,
                    &draw_call.platform.inst_vbuf,
                    sh.mapping.instance_props.total_slots
                );
                
                d3d11_cx.set_constant_buffers(
                    &self.passes[pass_id].platform.pass_uniforms,
                    &cxview.platform.view_uniforms,
                    &draw_call.platform.draw_uniforms,
                    &draw_call.platform.user_uniforms,
                    &shp.live_uniforms,
                    &shp.const_table_uniforms
                );
                
                for (i, texture_id) in draw_call.textures_2d.iter().enumerate() {
                    let cxtexture = &mut self.textures[*texture_id as usize];
                    match cxtexture.desc.format { // we only allocate Image and Mapped textures.
                        TextureFormat::Default | TextureFormat::ImageBGRA => {
                            if cxtexture.update_image {
                                cxtexture.update_image = false;
                                d3d11_cx.update_platform_texture_image_bgra(
                                    &mut cxtexture.platform,
                                    cxtexture.desc.width.unwrap(),
                                    cxtexture.desc.height.unwrap(),
                                    &cxtexture.image_u32,
                                );
                            }
                            d3d11_cx.set_shader_resource(i, &cxtexture.platform.shader_resource);
                        },
                        
                        TextureFormat::RenderBGRA | TextureFormat::RenderBGRAf16 | TextureFormat::RenderBGRAf32 => {
                            d3d11_cx.set_shader_resource(i, &cxtexture.platform.shader_resource);
                        },
                        _ => ()
                    }
                }
                //if self.passes[pass_id].debug{
                //    println!("DRAWING PASS {} {}", geometry.indices.len(), instances);
                //}
                
                d3d11_cx.draw_indexed_instanced(
                    geometry.indices.len(),
                    instances
                );
            }
        }
    }
    
    pub fn setup_pass_render_targets(&mut self, pass_id: usize, inherit_dpi_factor: f32, first_target: Option<&ComPtr<d3d11::ID3D11RenderTargetView >>, d3d11_cx: &D3d11Cx) {
        
        let pass_size = self.passes[pass_id].pass_size;
        
        self.passes[pass_id].set_matrix(Vec2::default(), pass_size);
        
        self.passes[pass_id].paint_dirty = false;
        let dpi_factor = if let Some(override_dpi_factor) = self.passes[pass_id].override_dpi_factor {
            override_dpi_factor
        }
        else {
            inherit_dpi_factor
        };
        self.passes[pass_id].set_dpi_factor(dpi_factor);
        
        //let wg = &d3d11_window.window_geom;
        d3d11_cx.set_viewport(pass_size.x * dpi_factor, pass_size.y * dpi_factor);
        
        // set up the color texture array
        let mut color_textures = Vec::<*mut d3d11::ID3D11RenderTargetView>::new();
        for (index, color_texture) in self.passes[pass_id].color_textures.iter().enumerate() {
            let render_target;
            let is_initial;
            if index == 0 && first_target.is_some() {
                render_target = first_target.unwrap();
                is_initial = true;
            }
            else {
                let cxtexture = &mut self.textures[color_texture.texture_id as usize];
                is_initial = d3d11_cx.update_render_target(cxtexture, dpi_factor, pass_size);
                render_target = cxtexture.platform.render_target_view.as_ref().unwrap();
            }
            color_textures.push(render_target.as_raw() as *mut _);
            // possibly clear it
            match color_texture.clear_color {
                ClearColor::InitWith(color) => {
                    if is_initial {
                        d3d11_cx.clear_render_target_view(&render_target, color); //self.clear_color);
                    }
                },
                ClearColor::ClearWith(color) => {
                    d3d11_cx.clear_render_target_view(&render_target, color); //self.clear_color);
                }
            }
        }
        
        // attach/clear depth buffers, if any
        if let Some(depth_texture_id) = self.passes[pass_id].depth_texture {
            let cxtexture = &mut self.textures[depth_texture_id as usize];
            let is_initial = d3d11_cx.update_depth_stencil(cxtexture, dpi_factor, pass_size);
            match self.passes[pass_id].clear_depth {
                ClearDepth::InitWith(depth_clear) => {
                    if is_initial {
                        d3d11_cx.clear_depth_stencil_view(cxtexture.platform.depth_stencil_view.as_ref().unwrap(), depth_clear as f32);
                    }
                },
                ClearDepth::ClearWith(depth_clear) => {
                    d3d11_cx.clear_depth_stencil_view(cxtexture.platform.depth_stencil_view.as_ref().unwrap(), depth_clear as f32);
                }
            }
            unsafe {d3d11_cx.context.OMSetRenderTargets(
                color_textures.len() as u32,
                color_textures.as_ptr(),
                cxtexture.platform.depth_stencil_view.as_ref().unwrap().as_raw() as *mut _
            )}
        }
        else {
            unsafe {d3d11_cx.context.OMSetRenderTargets(
                color_textures.len() as u32,
                color_textures.as_ptr(),
                ptr::null_mut()
            )}
        }
        
        // create depth, blend and raster states
        if self.passes[pass_id].platform.blend_state.is_none() {
            self.passes[pass_id].platform.blend_state = Some(
                d3d11_cx.create_blend_state().expect("Cannot create blend state")
            )
        }
        
        if self.passes[pass_id].platform.raster_state.is_none() {
            self.passes[pass_id].platform.raster_state = Some(
                d3d11_cx.create_raster_state().expect("Cannot create raster state")
            )
        }
        
        d3d11_cx.set_raster_state(self.passes[pass_id].platform.raster_state.as_ref().unwrap());
        d3d11_cx.set_blend_state(self.passes[pass_id].platform.blend_state.as_ref().unwrap());
        let cxpass = &mut self.passes[pass_id];
        cxpass.platform.pass_uniforms.update_with_f32_constant_data(&d3d11_cx, cxpass.pass_uniforms.as_slice());
    }
    
    pub fn draw_pass_to_window(&mut self, pass_id: usize, vsync: bool, dpi_factor: f32, d3d11_window: &mut D3d11Window, d3d11_cx: &D3d11Cx) {
        // let time1 = Cx::profile_time_ns();
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        
        self.setup_pass_render_targets(pass_id, dpi_factor, d3d11_window.render_target_view.as_ref(), d3d11_cx);
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        self.render_view(
            pass_id,
            view_id,
            Vec2::default(),
            (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
            self.repaint_id,
            &d3d11_cx,
            &mut zbias,
            zbias_step
        );
        d3d11_window.present(vsync);
        //println!("{}", (Cx::profile_time_ns() - time1)as f64 / 1000.0);
    }
    
    pub fn draw_pass_to_texture(&mut self, pass_id: usize, dpi_factor: f32, d3d11_cx: &D3d11Cx) {
        // let time1 = Cx::profile_time_ns();
        let view_id = self.passes[pass_id].main_view_id.unwrap();
        self.setup_pass_render_targets(pass_id, dpi_factor, None, d3d11_cx);
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        self.render_view(
            pass_id,
            view_id,
            Vec2::default(),
            (Vec2 {x: -50000., y: -50000.}, Vec2 {x: 50000., y: 50000.}),
            self.repaint_id,
            &d3d11_cx,
            &mut zbias,
            zbias_step
        );
    }
    
    
    pub fn hlsl_compile_all_shaders(&mut self, d3d11_cx: &D3d11Cx) {
        
        let options = ShaderCompileOptions {
            gather_all: false,
            create_const_table: false,
            no_const_collapse: false
        };
        
        for (live_id, shader) in &self.live_styles.shader_alloc {
            match self.live_styles.collect_and_analyse_shader(*live_id, options) {
                Err(err) => {
                    eprintln!("{}", err);
                    panic!()
                },
                Ok((shader_ast, default_geometry)) => {
                    let shader_id = shader.shader_id;
                    Self::hlsl_compile_shader(
                        shader_id,
                        &mut self.shaders[shader_id],
                        shader_ast,
                        default_geometry,
                        options,
                        d3d11_cx,
                        &self.live_styles
                    );
                }
            }
        };
        self.live_styles.changed_shaders.clear();
    }
    
    
    pub fn hlsl_update_all_shaders(&mut self, d3d11_cx: &D3d11Cx, errors: &mut Vec<LiveBodyError>) {
        
        // recompile shaders, and update values
        
        let options = ShaderCompileOptions {
            gather_all: true,
            create_const_table: true,
            no_const_collapse: false
        };
        
        for (live_id, change) in &self.live_styles.changed_shaders {
            match change {
                LiveChangeType::Recompile => {
                    match self.live_styles.collect_and_analyse_shader(*live_id, options) {
                        Err(err) => {
                            errors.push(err);
                        },
                        Ok((shader_ast, default_geometry)) => {
                            let shader_id = self.live_styles.shader_alloc.get(&live_id).unwrap().shader_id;
                            Self::hlsl_compile_shader(
                                shader_id,
                                &mut self.shaders[shader_id],
                                shader_ast,
                                default_geometry,
                                options,
                                d3d11_cx,
                                &self.live_styles
                            );
                        }
                    }
                }
                LiveChangeType::UpdateValue => {
                    let shader_id = self.live_styles.shader_alloc.get(&live_id).unwrap().shader_id;
                    let sh = &mut self.shaders[shader_id];
                    sh.mapping.update_live_uniforms(&self.live_styles);
                    if let Some(platform) = &mut sh.platform{
                        platform.live_uniforms.update_with_f32_constant_data(d3d11_cx, sh.mapping.live_uniforms_buf.as_slice());
                    }
                }
            }
        }
        self.live_styles.changed_shaders.clear();
        
    }
    
    pub fn hlsl_compile_shader(
        shader_id: usize,
        sh: &mut CxShader,
        shader_ast: ShaderAst,
        default_geometry: Option<Geometry>,
        options: ShaderCompileOptions,
        d3d11_cx: &D3d11Cx,
        live_styles: &LiveStyles
    ) -> ShaderCompileResult {
        
        let hlsl = generate_hlsl::generate_shader(&shader_ast, live_styles, options);
        let debug = shader_ast.debug;
        let mut mapping = CxShaderMapping::from_shader_ast(shader_ast, options, false);
        mapping.update_live_uniforms(live_styles);
        
        if debug {
            println!("--------------- Shader {} --------------- \n{}\n", shader_id, hlsl);
        }
        
        if let Some(sh_platform) = &mut sh.platform {
            if sh_platform.hlsl_shader == hlsl {
                sh.mapping = mapping;
                if let Some(const_table) = &sh.mapping.const_table {
                    if const_table.len()>0 {
                        sh_platform.const_table_uniforms.update_with_f32_constant_data(d3d11_cx, const_table.as_slice());
                    } 
                }
                return ShaderCompileResult::Nop
            }
        }
        
        let vs_blob = d3d11_cx.compile_shader("vs", "mpsc_vertex_main".as_bytes(), hlsl.as_bytes());
        
        fn split_source(src: &str) -> String {
            let mut r = String::new();
            let split = src.split("\n");
            for (line, chunk) in split.enumerate() {
                r.push_str(&(line + 1).to_string());
                r.push_str(":");
                r.push_str(chunk);
                r.push_str("\n");
            }
            return r
        }
        
        if let Err(msg) = vs_blob {
            println!("{}\n{}", msg, split_source(&hlsl));
            panic!("Cannot compile vertexshader {}", msg);
        }
        let vs_blob = vs_blob.unwrap();
        
        let ps_blob = d3d11_cx.compile_shader("ps", "mpsc_fragment_main".as_bytes(), hlsl.as_bytes());
        
        if let Err(msg) = ps_blob {
            println!("{}\n{}", msg, split_source(&hlsl));
            panic!("Cannot compile pixelshader {}", msg);
        }
        let ps_blob = ps_blob.unwrap();
        
        let vs = d3d11_cx.create_vertex_shader(&vs_blob).expect("cannot create vertexshader");
        let ps = d3d11_cx.create_pixel_shader(&ps_blob).expect("cannot create pixelshader");
        
        let mut layout_desc = Vec::new();
        let geom_named = NamedProps::construct(&mapping.geometries);
        let inst_named = NamedProps::construct(&mapping.instances);
        let mut strings = Vec::new();
        
        fn slots_to_dxgi_format(slots: usize) -> u32 {
            
            match slots {
                1 => dxgiformat::DXGI_FORMAT_R32_FLOAT,
                2 => dxgiformat::DXGI_FORMAT_R32G32_FLOAT,
                3 => dxgiformat::DXGI_FORMAT_R32G32B32_FLOAT,
                4 => dxgiformat::DXGI_FORMAT_R32G32B32A32_FLOAT,
                _ => panic!("slots_to_dxgi_format unsupported slotcount {}", slots)
            }
        }
        
        for (index, geom) in geom_named.props.iter().enumerate() {
            strings.push(ffi::CString::new(format!("GEOM{}", generate_hlsl::index_to_char(index))).unwrap()); //std::char::from_u32(index as u32 + 65).unwrap())).unwrap());
            layout_desc.push(d3d11::D3D11_INPUT_ELEMENT_DESC {
                SemanticName: strings.last().unwrap().as_ptr() as *const _,
                SemanticIndex: 0,
                Format: slots_to_dxgi_format(geom.slots),
                InputSlot: 0,
                AlignedByteOffset: (geom.offset * 4) as u32,
                InputSlotClass: d3d11::D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0
            })
        }
        
        let mut index = 0;
        for inst in &inst_named.props {
            if inst.slots == 16 {
                for i in 0..4 {
                    strings.push(ffi::CString::new(format!("INST{}", generate_hlsl::index_to_char(index))).unwrap()); //std::char::from_u32(index as u32 + 65).unwrap())).unwrap());
                    layout_desc.push(d3d11::D3D11_INPUT_ELEMENT_DESC {
                        SemanticName: strings.last().unwrap().as_ptr() as *const _,
                        SemanticIndex: 0,
                        Format: slots_to_dxgi_format(4),
                        InputSlot: 1,
                        AlignedByteOffset: (inst.offset * 4 + i * 16) as u32,
                        InputSlotClass: d3d11::D3D11_INPUT_PER_INSTANCE_DATA,
                        InstanceDataStepRate: 1
                    });
                    index += 1;
                }
            }
            else if inst.slots == 9 {
                for i in 0..3 {
                    strings.push(ffi::CString::new(format!("INST{}", generate_hlsl::index_to_char(index))).unwrap()); //std::char::from_u32(index as u32 + 65).unwrap())).unwrap());
                    layout_desc.push(d3d11::D3D11_INPUT_ELEMENT_DESC {
                        SemanticName: strings.last().unwrap().as_ptr() as *const _,
                        SemanticIndex: 0,
                        Format: slots_to_dxgi_format(3),
                        InputSlot: 1,
                        AlignedByteOffset: (inst.offset * 4 + i * 9) as u32,
                        InputSlotClass: d3d11::D3D11_INPUT_PER_INSTANCE_DATA,
                        InstanceDataStepRate: 1
                    });
                    index += 1;
                }
            }
            else {
                strings.push(ffi::CString::new(format!("INST{}", generate_hlsl::index_to_char(index))).unwrap()); //std::char::from_u32(index as u32 + 65).unwrap())).unwrap());
                layout_desc.push(d3d11::D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: strings.last().unwrap().as_ptr() as *const _,
                    SemanticIndex: 0,
                    Format: slots_to_dxgi_format(inst.slots),
                    InputSlot: 1,
                    AlignedByteOffset: (inst.offset * 4) as u32,
                    InputSlotClass: d3d11::D3D11_INPUT_PER_INSTANCE_DATA,
                    InstanceDataStepRate: 1
                });
                index += 1;
            }
        }
        
        let input_layout = d3d11_cx.create_input_layout(&vs_blob, &layout_desc).expect("cannot create input layout");
        
        sh.default_geometry = default_geometry;
        sh.mapping = mapping;
        sh.platform = Some(CxPlatformShader {
            const_table_uniforms: {
                let mut buf = D3d11Buffer {..Default::default()};
                if let Some(const_table) = &sh.mapping.const_table {
                    if const_table.len()>0 {
                        buf.update_with_f32_constant_data(d3d11_cx, const_table.as_slice());
                    }
                }
                buf
            },
            live_uniforms: {
                let mut buf = D3d11Buffer {..Default::default()};
                if sh.mapping.live_uniforms_buf.len()>0 {
                    buf.update_with_f32_constant_data(d3d11_cx, sh.mapping.live_uniforms_buf.as_slice());
                }
                buf
            },
            hlsl_shader: hlsl,
            vertex_shader: vs,
            pixel_shader: ps,
            vertex_shader_blob: vs_blob,
            pixel_shader_blob: ps_blob,
            input_layout: input_layout,
        });
        
        return ShaderCompileResult::Ok;
    }
}


pub struct D3d11RenderTarget {
    pub render_target_view: Option<ComPtr<d3d11::ID3D11RenderTargetView >>,
    //pub raster_state: ComPtr<d3d11::ID3D11RasterizerState>,
    //pub blend_state: ComPtr<d3d11::ID3D11BlendState>,
}

/*
impl D3d11RenderTarget {
    pub fn new(d3d11_cx: &D3d11Cx) -> D3d11RenderTarget {
        let raster_state = d3d11_cx.create_raster_state().expect("Cannot create_raster_state");
        let blend_state = d3d11_cx.create_blend_state().expect("Cannot create_blend_state");
        return D3d11RenderTarget {
            raster_state: raster_state,
            blend_state: blend_state,
            render_target_view: None
        }
    }
    
    
    pub fn create_from_textures(&mut self, d3d11_cx: &D3d11Cx, swap_texture: &ComPtr<d3d11::ID3D11Texture2D>) {
        self.render_target_view = Some(d3d11_cx.create_render_target_view(swap_texture).expect("Cannot create_render_target_view"));
    }
    
    pub fn disconnect_render_targets(&mut self) {
        self.render_target_view = None;
    }
}
*/
pub struct D3d11Window {
    pub window_id: usize,
    pub is_in_resize: bool,
    pub window_geom: WindowGeom,
    pub win32_window: Win32Window,
    pub render_target_view: Option<ComPtr<d3d11::ID3D11RenderTargetView >>,
    //pub render_target: D3d11RenderTarget,
    // pub depth_stencil_view: Option<ComPtr<d3d11::ID3D11DepthStencilView>>,
    // pub depth_stencil_buffer: Option<ComPtr<d3d11::ID3D11Texture2D>>,
    pub swap_texture: Option<ComPtr<d3d11::ID3D11Texture2D >>,
    // pub d2d1_hwnd_target: Option<ComPtr<d2d1::ID2D1HwndRenderTarget>>,
    // pub d2d1_bitmap: Option<ComPtr<d2d1::ID2D1Bitmap>>,
    pub alloc_size: Vec2,
    pub first_draw: bool,
    pub swap_chain: ComPtr<dxgi1_2::IDXGISwapChain1>,
}

impl D3d11Window {
    pub fn new(window_id: usize, d3d11_cx: &D3d11Cx, win32_app: &mut Win32App, inner_size: Vec2, position: Option<Vec2>, title: &str) -> D3d11Window {
        let mut win32_window = Win32Window::new(win32_app, window_id);
        
        win32_window.init(title, inner_size, position);
        let window_geom = win32_window.get_window_geom();
        
        let swap_chain = d3d11_cx.create_swap_chain_for_hwnd(&window_geom, &win32_window).expect("Cannot create_swap_chain_for_hwnd");
        
        let swap_texture = D3d11Cx::get_swap_texture(&swap_chain).expect("Cannot get swap texture");
        
        let render_target_view = Some(d3d11_cx.create_render_target_view(&swap_texture).expect("Cannot create_render_target_view"));
        
        //let mut render_target = D3d11RenderTarget::new(d3d11_cx);
        //render_target.create_from_textures(d3d11_cx, &swap_texture);
        
        
        //let depth_stencil_buffer = d3d11_cx.create_depth_stencil_buffer(&window_geom).expect("Cannot create_depth_stencil_buffer");
        //let depth_stencil_view = d3d11_cx.create_depth_stencil_view(&depth_stencil_buffer).expect("Cannot create_depth_stencil_view");
        //let depth_stencil_state = d3d11_cx.create_depth_stencil_state().expect("Cannot create_depth_stencil_state");
        //unsafe {d3d11_cx.context.OMSetDepthStencilState(depth_stencil_state.as_raw() as *mut _, 1)}
        
        D3d11Window {
            first_draw: true,
            is_in_resize: false,
            window_id: window_id,
            alloc_size: window_geom.inner_size,
            window_geom: window_geom,
            win32_window: win32_window,
            swap_texture: Some(swap_texture),
            render_target_view: render_target_view,
            //depth_stencil_buffer: Some(depth_stencil_buffer),
            //render_target_view: Some(render_target_view),
            //depth_stencil_view: Some(depth_stencil_view),
            //d2d1_hwnd_target: None,
            //d2d1_bitmap: None,
            swap_chain: swap_chain,
            //raster_state: raster_state,
            //blend_state: blend_state
        }
    }
    
    pub fn start_resize(&mut self) {
        self.is_in_resize = true;
    }
    
    // switch back to swapchain
    pub fn stop_resize(&mut self) {
        self.is_in_resize = false;
        self.alloc_size = Vec2::default();
    }
    
    //fn alloc_buffers_from_texture(&mut self, d3d11_cx: &D3d11Cx) {
    //    let swap_texture = self.swap_texture.as_ref().unwrap();
    //    let render_target_view = d3d11_cx.create_render_target_view(&swap_texture).expect("Cannot create_render_target_view");
    //let depth_stencil_buffer = d3d11_cx.create_depth_stencil_buffer(&self.window_geom).expect("Cannot create_depth_stencil_buffer");
    //let depth_stencil_view = d3d11_cx.create_depth_stencil_view(&depth_stencil_buffer).expect("Cannot create_depth_stencil_view");
    //    self.alloc_size = self.window_geom.inner_size;
    //    self.render_target_view = Some(render_target_view);
    //    self.depth_stencil_view = Some(depth_stencil_view);
    //    self.depth_stencil_buffer = Some(depth_stencil_buffer);
    // }
    
    pub fn resize_buffers(&mut self, d3d11_cx: &D3d11Cx) {
        if self.alloc_size == self.window_geom.inner_size {
            return
        }
        self.alloc_size = self.window_geom.inner_size;
        self.swap_texture = None;
        self.render_target_view = None;
        
        d3d11_cx.resize_swap_chain(&self.window_geom, &self.swap_chain);
        
        let swap_texture = D3d11Cx::get_swap_texture(&self.swap_chain).expect("Cannot get swap texture");
        
        self.render_target_view = Some(d3d11_cx.create_render_target_view(&swap_texture).expect("Cannot create_render_target_view"));
        
        self.swap_texture = Some(swap_texture);
    }
    
    pub fn present(&mut self, vsync: bool) {
        unsafe {self.swap_chain.Present(if vsync {1}else {0}, 0)};
    }
    
}

#[derive(Clone)]
pub struct D3d11Cx {
    pub device: ComPtr<d3d11::ID3D11Device>,
    pub context: ComPtr<d3d11::ID3D11DeviceContext>,
    pub factory: ComPtr<dxgi1_2::IDXGIFactory2>,
    //    pub d2d1_factory: ComPtr<d2d1::ID2D1Factory>
}

impl D3d11Cx {
    
    pub fn new() -> D3d11Cx {
        let factory = D3d11Cx::create_dxgi_factory1(&dxgi1_2::IDXGIFactory2::uuidof()).expect("cannot create_dxgi_factory1");
        let adapter = D3d11Cx::enum_adapters(&factory).expect("cannot enum_adapters");
        let (device, context) = D3d11Cx::create_d3d11_device(&adapter).expect("cannot create_d3d11_device");
        // let d2d1_factory = D3d11Cx::create_d2d1_factory().expect("cannot create_d2d1_factory");
        D3d11Cx {
            device: device,
            context: context,
            factory: factory,
            //    d2d1_factory: d2d1_factory
        }
    }
    
    pub fn disconnect_rendertargets(&self) {
        unsafe {self.context.OMSetRenderTargets(0, ptr::null(), ptr::null_mut())}
    }
    
    pub fn set_viewport(&self, width: f32, height: f32) {
        let viewport = d3d11::D3D11_VIEWPORT {
            Width: width,
            Height: height,
            MinDepth: 0.,
            MaxDepth: 1.,
            TopLeftX: 0.0,
            TopLeftY: 0.0
        };
        unsafe {self.context.RSSetViewports(1, &viewport)}
    }
    
    /*pub fn set_rendertargets(&self, d3d11_target: &D3d11RenderTarget) {
        unsafe {self.context.OMSetRenderTargets(
            1,
            [d3d11_target.render_target_view.as_ref().unwrap().as_raw() as *mut _].as_ptr(),
            ptr::null_mut() //,//d3d11_window.depth_stencil_view.as_ref().unwrap().as_raw() as *mut _
        )}
    }*/
    
    pub fn clear_render_target_view(&self, render_target_view: &ComPtr<d3d11::ID3D11RenderTargetView>, color: Vec4) {
        let color = [color.x, color.y, color.z, color.w];
        unsafe {self.context.ClearRenderTargetView(render_target_view.as_raw() as *mut _, &color)}
    }
    
    pub fn clear_depth_stencil_view(&self, depth_stencil_view: &ComPtr<d3d11::ID3D11DepthStencilView>, depth: f32) {
        unsafe {self.context.ClearDepthStencilView(
            depth_stencil_view.as_raw() as *mut _,
            d3d11::D3D11_CLEAR_DEPTH | d3d11::D3D11_CLEAR_STENCIL,
            depth,
            0
        )}
    }
    
    //fn clear_depth_stencil_view(&self, d3d11_window: &D3d11Window) { 
    //    unsafe {self.context.ClearDepthStencilView(d3d11_window.depth_stencil_view.as_ref().unwrap().as_raw() as *mut _, d3d11::D3D11_CLEAR_DEPTH, 1.0, 0)}
    //}
    
    pub fn set_raster_state(&self, raster_state: &ComPtr<d3d11::ID3D11RasterizerState>,) {
        unsafe {self.context.RSSetState(raster_state.as_raw() as *mut _)}
    }
    
    pub fn set_blend_state(&self, blend_state: &ComPtr<d3d11::ID3D11BlendState>,) {
        let blend_factor = [0., 0., 0., 0.];
        unsafe {self.context.OMSetBlendState(blend_state.as_raw() as *mut _, &blend_factor, 0xffffffff)}
    }
    
    pub fn set_input_layout(&self, input_layout: &ComPtr<d3d11::ID3D11InputLayout>) {
        unsafe {self.context.IASetInputLayout(input_layout.as_raw() as *mut _)}
    }
    
    pub fn set_index_buffer(&self, index_buffer: &D3d11Buffer) {
        if let Some(buf) = &index_buffer.buffer {
            unsafe {self.context.IASetIndexBuffer(buf.as_raw() as *mut _, dxgiformat::DXGI_FORMAT_R32_UINT, 0)}
        }
    }
    
    pub fn set_shaders(&self, vertex_shader: &ComPtr<d3d11::ID3D11VertexShader>, pixel_shader: &ComPtr<d3d11::ID3D11PixelShader>) {
        unsafe {self.context.VSSetShader(vertex_shader.as_raw() as *mut _, ptr::null(), 0)}
        unsafe {self.context.PSSetShader(pixel_shader.as_raw() as *mut _, ptr::null(), 0)}
    }
    
    pub fn set_shader_resource(&self, index: usize, texture: &Option<ComPtr<d3d11::ID3D11ShaderResourceView >>) {
        if let Some(texture) = texture {
            let raw = [texture.as_raw() as *const std::ffi::c_void];
            unsafe {self.context.PSSetShaderResources(index as u32, 1, raw.as_ptr() as *const *mut _)}
            unsafe {self.context.VSSetShaderResources(index as u32, 1, raw.as_ptr() as *const *mut _)}
        }
    }
    
    //fn set_raster_state(&self, d3d11_window: &D3d11Window) {
    //    unsafe {self.context.RSSetState(d3d11_window.raster_state.as_raw() as *mut _)};
    // }
    
    pub fn set_primitive_topology(&self) {
        unsafe {self.context.IASetPrimitiveTopology(d3dcommon::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST)}
    }
    
    pub fn set_vertex_buffers(&self, geom_buf: &D3d11Buffer, geom_slots: usize, inst_buf: &D3d11Buffer, inst_slots: usize) {
        let geom_buf = geom_buf.buffer.as_ref().unwrap();
        let inst_buf = inst_buf.buffer.as_ref().unwrap();
        
        let strides = [(geom_slots * 4) as u32, (inst_slots * 4) as u32];
        let offsets = [0u32, 0u32];
        let buffers = [geom_buf.as_raw() as *const std::ffi::c_void, inst_buf.as_raw() as *const std::ffi::c_void];
        unsafe {self.context.IASetVertexBuffers(
            0,
            2,
            buffers.as_ptr() as *const *mut _,
            strides.as_ptr() as *const _,
            offsets.as_ptr() as *const _
        )};
    }
    
    pub fn set_constant_buffers(
        &self,
        pass_uni: &D3d11Buffer,
        view_uni: &D3d11Buffer,
        draw_uni: &D3d11Buffer,
        user_uni: &D3d11Buffer,
        live_uni: &D3d11Buffer,
        const_table: &D3d11Buffer
    ) {
        let pass_uni = pass_uni.buffer.as_ref().unwrap();
        let view_uni = view_uni.buffer.as_ref().unwrap();
        let draw_uni = draw_uni.buffer.as_ref().unwrap();
        
        let buffers = [
            pass_uni.as_raw() as *const std::ffi::c_void,
            view_uni.as_raw() as *const std::ffi::c_void,
            draw_uni.as_raw() as *const std::ffi::c_void,
            if let Some(uni) = user_uni.buffer.as_ref() {uni.as_raw() as *const std::ffi::c_void}else {0 as *const std::ffi::c_void},
            if let Some(uni) = live_uni.buffer.as_ref() {uni.as_raw() as *const std::ffi::c_void}else {0 as *const std::ffi::c_void},
            if let Some(const_table) = const_table.buffer.as_ref() {const_table.as_raw() as *const std::ffi::c_void}else {0 as *const std::ffi::c_void},
        ];
        unsafe {self.context.VSSetConstantBuffers(
            0,
            6,
            buffers.as_ptr() as *const *mut _,
        )};
        unsafe {self.context.PSSetConstantBuffers(
            0,
            6,
            buffers.as_ptr() as *const *mut _,
        )};
    }
    
    pub fn draw_indexed_instanced(&self, num_vertices: usize, num_instances: usize) {
        
        
        unsafe {self.context.DrawIndexedInstanced(
            num_vertices as u32,
            num_instances as u32,
            0,
            0,
            0
        )};
    }
    
    pub fn compile_shader(&self, stage: &str, entry: &[u8], shader: &[u8])
        -> Result<ComPtr<d3dcommon::ID3DBlob>, String> {
        
        let mut blob = ptr::null_mut();
        let mut error = ptr::null_mut();
        let entry = ffi::CString::new(entry).unwrap();
        let stage = format!("{}_5_0\0", stage);
        let hr = unsafe {d3dcompiler::D3DCompile(
            shader.as_ptr() as *const _,
            shader.len(),
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            entry.as_ptr() as *const _,
            stage.as_ptr() as *const i8,
            1,
            0,
            &mut blob as *mut *mut _,
            &mut error as *mut *mut _
        )};
        if !winerror::SUCCEEDED(hr) {
            let error = unsafe {ComPtr::<d3dcommon::ID3DBlob>::from_raw(error)};
            let message = unsafe {
                let pointer = error.GetBufferPointer();
                let size = error.GetBufferSize();
                let slice = std::slice::from_raw_parts(pointer as *const u8, size as usize);
                String::from_utf8_lossy(slice).into_owned()
            };
            
            Err(message)
        } else {
            Ok(unsafe {ComPtr::<d3dcommon::ID3DBlob>::from_raw(blob)})
        }
    }
    
    pub fn create_input_layout(&self, vs: &ComPtr<d3dcommon::ID3DBlob>, layout_desc: &Vec<d3d11::D3D11_INPUT_ELEMENT_DESC>)
        -> Result<ComPtr<d3d11::ID3D11InputLayout>, String> {
        let mut input_layout = ptr::null_mut();
        let hr = unsafe {self.device.CreateInputLayout(
            layout_desc.as_ptr(),
            layout_desc.len() as u32,
            vs.GetBufferPointer(),
            vs.GetBufferSize(),
            &mut input_layout as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(input_layout as *mut _)})
        }
        else {
            Err(format!("create_input_layout failed {}", hr))
        }
    }
    
    pub fn create_pixel_shader(&self, ps: &ComPtr<d3dcommon::ID3DBlob>)
        -> Result<ComPtr<d3d11::ID3D11PixelShader>, String> {
        let mut pixel_shader = ptr::null_mut();
        let hr = unsafe {self.device.CreatePixelShader(
            ps.GetBufferPointer(),
            ps.GetBufferSize(),
            ptr::null_mut(),
            &mut pixel_shader as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(pixel_shader as *mut _)})
        }
        else {
            Err(format!("create_pixel_shader failed {}", hr))
        }
    }
    
    
    pub fn create_vertex_shader(&self, vs: &ComPtr<d3dcommon::ID3DBlob>)
        -> Result<ComPtr<d3d11::ID3D11VertexShader>, String> {
        let mut vertex_shader = ptr::null_mut();
        let hr = unsafe {self.device.CreateVertexShader(
            vs.GetBufferPointer(),
            vs.GetBufferSize(),
            ptr::null_mut(),
            &mut vertex_shader as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(vertex_shader as *mut _)})
        }
        else {
            Err(format!("create_vertex_shader failed {}", hr))
        }
    }
    
    
    pub fn create_raster_state(&self)
        -> Result<ComPtr<d3d11::ID3D11RasterizerState>, winerror::HRESULT> {
        let mut raster_state = ptr::null_mut();
        let raster_desc = d3d11::D3D11_RASTERIZER_DESC {
            AntialiasedLineEnable: FALSE,
            CullMode: d3d11::D3D11_CULL_NONE,
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            DepthClipEnable: TRUE,
            FillMode: d3d11::D3D11_FILL_SOLID,
            FrontCounterClockwise: FALSE,
            MultisampleEnable: FALSE,
            ScissorEnable: FALSE,
            SlopeScaledDepthBias: 0.0,
        };
        let hr = unsafe {self.device.CreateRasterizerState(&raster_desc, &mut raster_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(raster_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn create_blend_state(&self)
        -> Result<ComPtr<d3d11::ID3D11BlendState>, winerror::HRESULT> {
        let mut blend_state = ptr::null_mut();
        let mut blend_desc: d3d11::D3D11_BLEND_DESC = unsafe {mem::zeroed()};
        blend_desc.AlphaToCoverageEnable = FALSE;
        blend_desc.RenderTarget[0] = d3d11::D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: TRUE,
            SrcBlend: d3d11::D3D11_BLEND_ONE,
            SrcBlendAlpha: d3d11::D3D11_BLEND_ONE,
            DestBlend: d3d11::D3D11_BLEND_INV_SRC_ALPHA,
            DestBlendAlpha: d3d11::D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: d3d11::D3D11_BLEND_OP_ADD,
            BlendOpAlpha: d3d11::D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: d3d11::D3D11_COLOR_WRITE_ENABLE_ALL as u8,
        };
        let hr = unsafe {self.device.CreateBlendState(&blend_desc, &mut blend_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(blend_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn create_depth_stencil_view(&self, buffer: &ComPtr<d3d11::ID3D11Texture2D>)
        -> Result<ComPtr<d3d11::ID3D11DepthStencilView>, winerror::HRESULT> {
        let mut depth_stencil_view = ptr::null_mut();
        let mut dsv_desc: d3d11::D3D11_DEPTH_STENCIL_VIEW_DESC = unsafe {mem::zeroed()};
        dsv_desc.Format = dxgiformat::DXGI_FORMAT_D32_FLOAT_S8X24_UINT;
        dsv_desc.ViewDimension = d3d11::D3D11_DSV_DIMENSION_TEXTURE2D;
        *unsafe {dsv_desc.u.Texture2D_mut()} = d3d11::D3D11_TEX2D_DSV {
            MipSlice: 0,
        };
        let hr = unsafe {self.device.CreateDepthStencilView(buffer.as_raw() as *mut _, &dsv_desc, &mut depth_stencil_view as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_view as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn create_depth_stencil_state(&self)
        -> Result<ComPtr<d3d11::ID3D11DepthStencilState>, winerror::HRESULT> {
        let mut depth_stencil_state = ptr::null_mut();
        let ds_desc = d3d11::D3D11_DEPTH_STENCIL_DESC {
            DepthEnable: TRUE,
            DepthWriteMask: d3d11::D3D11_DEPTH_WRITE_MASK_ALL,
            DepthFunc: d3d11::D3D11_COMPARISON_LESS_EQUAL,
            StencilEnable: FALSE,
            StencilReadMask: 0xff,
            StencilWriteMask: 0xff,
            FrontFace: d3d11::D3D11_DEPTH_STENCILOP_DESC {
                StencilFailOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilDepthFailOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilPassOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilFunc: d3d11::D3D11_COMPARISON_ALWAYS,
            },
            BackFace: d3d11::D3D11_DEPTH_STENCILOP_DESC {
                StencilFailOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilDepthFailOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilPassOp: d3d11::D3D11_STENCIL_OP_REPLACE,
                StencilFunc: d3d11::D3D11_COMPARISON_ALWAYS,
            },
        };
        
        let hr = unsafe {self.device.CreateDepthStencilState(&ds_desc, &mut depth_stencil_state as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_state as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn create_render_target_view(&self, texture: &ComPtr<d3d11::ID3D11Texture2D>)
        -> Result<ComPtr<d3d11::ID3D11RenderTargetView>, winerror::HRESULT> {
        //let texture = D3d11Cx::get_swap_texture(swap_chain) ?;
        let mut render_target_view = ptr::null_mut();
        let hr = unsafe {self.device.CreateRenderTargetView(
            texture.as_raw() as *mut _,
            ptr::null(),
            &mut render_target_view as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(render_target_view as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn get_swap_texture(swap_chain: &ComPtr<dxgi1_2::IDXGISwapChain1>)
        -> Result<ComPtr<d3d11::ID3D11Texture2D>, winerror::HRESULT> {
        let mut texture = ptr::null_mut();
        let hr = unsafe {swap_chain.GetBuffer(
            0,
            &d3d11::ID3D11Texture2D::uuidof(),
            &mut texture as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(texture as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn create_swap_chain_for_hwnd(
        &self,
        wg: &WindowGeom,
        win32_window: &Win32Window,
    )
        -> Result<ComPtr<dxgi1_2::IDXGISwapChain1>, winerror::HRESULT> {
        let mut swap_chain1 = ptr::null_mut();
        let sc_desc = dxgi1_2::DXGI_SWAP_CHAIN_DESC1 {
            AlphaMode: dxgi1_2::DXGI_ALPHA_MODE_IGNORE,
            BufferCount: 2,
            Width: (wg.inner_size.x * wg.dpi_factor) as u32,
            Height: (wg.inner_size.y * wg.dpi_factor) as u32,
            Format: dxgiformat::DXGI_FORMAT_B8G8R8A8_UNORM,
            Flags: 0,
            BufferUsage: dxgitype::DXGI_USAGE_RENDER_TARGET_OUTPUT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {Count: 1, Quality: 0,},
            Scaling: dxgi1_2::DXGI_SCALING_NONE,
            Stereo: FALSE,
            SwapEffect: dxgi::DXGI_SWAP_EFFECT_FLIP_DISCARD,
        };
        
        let hr = unsafe {self.factory.CreateSwapChainForHwnd(
            self.device.as_raw() as *mut _,
            win32_window.hwnd.unwrap(),
            &sc_desc,
            ptr::null(),
            ptr::null_mut(),
            &mut swap_chain1 as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(swap_chain1 as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn resize_swap_chain(
        &self,
        wg: &WindowGeom,
        swap_chain: &ComPtr<dxgi1_2::IDXGISwapChain1>,
    ) {
        unsafe {
            let hr = swap_chain.ResizeBuffers(
                2,
                (wg.inner_size.x * wg.dpi_factor) as u32,
                (wg.inner_size.y * wg.dpi_factor) as u32,
                dxgiformat::DXGI_FORMAT_B8G8R8A8_UNORM,
                0
            );
            if !winerror::SUCCEEDED(hr) {
                panic!("Could not resize swapchain");
            }
        }
    }
    
    pub fn create_d3d11_device(adapter: &ComPtr<dxgi::IDXGIAdapter>)
        -> Result<(ComPtr<d3d11::ID3D11Device>, ComPtr<d3d11::ID3D11DeviceContext>), winerror::HRESULT> {
        let mut device = ptr::null_mut();
        let mut device_context = ptr::null_mut();
        let hr = unsafe {d3d11::D3D11CreateDevice(
            adapter.as_raw() as *mut _,
            d3dcommon::D3D_DRIVER_TYPE_UNKNOWN,
            ptr::null_mut(),
            0, //d3d11::D3D11_CREATE_DEVICE_VIDEO_SUPPORT, //d3dcommonLLD3D11_CREATE_DEVICE_DEBUG,
            [d3dcommon::D3D_FEATURE_LEVEL_11_0].as_ptr(),
            1,
            d3d11::D3D11_SDK_VERSION,
            &mut device as *mut *mut _,
            ptr::null_mut(),
            &mut device_context as *mut *mut _,
        )};
        if winerror::SUCCEEDED(hr) {
            Ok((unsafe {ComPtr::from_raw(device as *mut _)}, unsafe {ComPtr::from_raw(device_context as *mut _)}))
        }
        else {
            Err(hr)
        }
    }
    
    pub fn create_dxgi_factory1(guid: &GUID)
        -> Result<ComPtr<dxgi1_2::IDXGIFactory2>, winerror::HRESULT> {
        let mut factory = ptr::null_mut();
        let hr = unsafe {dxgi::CreateDXGIFactory1(
            guid,
            &mut factory as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(factory as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    
    pub fn enum_adapters(factory: &ComPtr<dxgi1_2::IDXGIFactory2>)
        -> Result<ComPtr<dxgi::IDXGIAdapter>, winerror::HRESULT> {
        let mut adapter = ptr::null_mut();
        let hr = unsafe {factory.EnumAdapters(
            0,
            &mut adapter as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(adapter as *mut _)})
        }
        else {
            Err(hr)
        }
    }
    
    pub fn update_render_target(&self, cxtexture: &mut CxTexture, dpi_factor: f32, size: Vec2) -> bool {
        
        let width = if let Some(width) = cxtexture.desc.width {width as usize} else {(size.x * dpi_factor) as usize};
        let height = if let Some(height) = cxtexture.desc.height {height as usize} else {(size.y * dpi_factor) as usize};
        
        if cxtexture.platform.width == width && cxtexture.platform.height == height {
            return false
        }
        
        let format;
        match cxtexture.desc.format {
            TextureFormat::Default | TextureFormat::RenderBGRA => {
                format = dxgiformat::DXGI_FORMAT_R8G8B8A8_UNORM;
            }
            TextureFormat::RenderBGRAf16 => {
                format = dxgiformat::DXGI_FORMAT_R32G32B32A32_FLOAT;
            }
            TextureFormat::RenderBGRAf32 => {
                format = dxgiformat::DXGI_FORMAT_R32G32B32A32_FLOAT;
            },
            _ => {
                panic!("Wrong format for update_render_target");
            }
        }
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {Count: 1, Quality: 0},
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_RENDER_TARGET | d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        
        let mut texture = ptr::null_mut();
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, ptr::null(), &mut texture as *mut *mut _)};
        
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
            cxtexture.platform.width = width;
            cxtexture.platform.height = height;
            
            cxtexture.platform.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
            cxtexture.platform.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
            
            cxtexture.platform.render_target_view = Some(
                self.create_render_target_view(
                    cxtexture.platform.texture.as_ref().unwrap()
                ).expect("Cannot create_render_target_view")
            );
        }
        else {
            panic!("update_render_target failed");
        }
        return true
    }
    
    pub fn update_depth_stencil(&self, cxtexture: &mut CxTexture, dpi_factor: f32, size: Vec2) -> bool {
        
        let width = if let Some(width) = cxtexture.desc.width {width as usize} else {(size.x * dpi_factor) as usize};
        let height = if let Some(height) = cxtexture.desc.height {height as usize} else {(size.y * dpi_factor) as usize};
        
        if cxtexture.platform.width == width && cxtexture.platform.height == height {
            return false
        }
        
        let format;
        match cxtexture.desc.format {
            TextureFormat::Default | TextureFormat::Depth32Stencil8 => {
                format = dxgiformat::DXGI_FORMAT_D32_FLOAT_S8X24_UINT;
            }
            _ => {
                panic!("Wrong format for update_depth_stencil");
            }
        }
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {Count: 1, Quality: 0},
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_DEPTH_STENCIL, // | d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        
        let mut texture = ptr::null_mut();
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, ptr::null(), &mut texture as *mut *mut _)};
        
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
            cxtexture.platform.width = width;
            cxtexture.platform.height = height;
            cxtexture.platform.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            
            //let mut shader_resource = ptr::null_mut();
            //unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
            //cxtexture.platform.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
            
            // lets create the render target too
            if let Ok(dsview) = self.create_depth_stencil_view(cxtexture.platform.texture.as_ref().unwrap()) {
                cxtexture.platform.depth_stencil_view = Some(dsview);
            }
            else {
                panic!("create_depth_stencil_view failed");
            }
        }
        else {
            panic!("update_render_target failed");
        }
        return true
    }
    
    pub fn update_platform_texture_image_bgra(&self, res: &mut CxPlatformTexture, width: usize, height: usize, image_u32: &Vec<u32>) {
        
        if image_u32.len() != width * height {
            println!("update_platform_texture_image_bgra with wrong buffer_u32 size!");
            return;
        }
        
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: image_u32.as_ptr() as *const _,
            SysMemPitch: (width * 4) as u32,
            SysMemSlicePitch: 0
        };
        
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgiformat::DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0
            },
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = ptr::null_mut();
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, &sub_data, &mut texture as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(
                texture as *mut _,
                ptr::null(),
                &mut shader_resource as *mut *mut _
            )};
            res.width = width;
            res.height = height;
            res.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            res.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
        }
        else {
            panic!("update_platform_texture_image_bgra failed");
        }
    }
    
    
}

#[derive(Clone, Default)]
pub struct CxPlatformView {
    pub view_uniforms: D3d11Buffer
}

#[derive(Default, Clone)]
pub struct CxPlatformDrawCall {
    pub draw_uniforms: D3d11Buffer,
    pub user_uniforms: D3d11Buffer,
    pub inst_vbuf: D3d11Buffer
}

#[derive(Default, Clone)]
pub struct D3d11Buffer {
    pub last_size: usize,
    pub buffer: Option<ComPtr<d3d11::ID3D11Buffer >>
}

impl D3d11Buffer {
    pub fn update_with_data(&mut self, d3d11_cx: &D3d11Cx, bind_flags: u32, len_slots: usize, data: *const std::ffi::c_void) {
        let mut buffer = ptr::null_mut();
        
        let buffer_desc = d3d11::D3D11_BUFFER_DESC {
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            ByteWidth: (len_slots * 4) as u32,
            BindFlags: bind_flags,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0
        };
        
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: data,
            SysMemPitch: 0,
            SysMemSlicePitch: 0
        };
        
        let hr = unsafe {d3d11_cx.device.CreateBuffer(&buffer_desc, &sub_data, &mut buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            self.last_size = len_slots;
            self.buffer = Some(unsafe {ComPtr::from_raw(buffer as *mut _)});
        }
        else {
            panic!("Buffer create failed {}", len_slots);
        }
    }
    
    pub fn update_with_u32_index_data(&mut self, d3d11_cx: &D3d11Cx, data: &[u32]) {
        self.update_with_data(d3d11_cx, d3d11::D3D11_BIND_INDEX_BUFFER, data.len(), data.as_ptr() as *const _);
    }
    
    pub fn update_with_f32_vertex_data(&mut self, d3d11_cx: &D3d11Cx, data: &[f32]) {
        self.update_with_data(d3d11_cx, d3d11::D3D11_BIND_VERTEX_BUFFER, data.len(), data.as_ptr() as *const _);
    }
    
    pub fn update_with_f32_constant_data(&mut self, d3d11_cx: &D3d11Cx, data: &[f32]) {
        let mut buffer = ptr::null_mut();
        if (data.len() & 3) != 0 { // we have to align the data at the end
            let mut new_data = data.to_vec();
            let steps = 4 - (data.len() & 3);
            for _ in 0..steps {
                new_data.push(0.0);
            };
            return self.update_with_f32_constant_data(d3d11_cx, &new_data);
        }
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0
        };
        let len_slots = data.len();
        
        let buffer_desc = d3d11::D3D11_BUFFER_DESC {
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            ByteWidth: (len_slots * 4) as u32,
            BindFlags: d3d11::D3D11_BIND_CONSTANT_BUFFER,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0
        };
        
        let hr = unsafe {d3d11_cx.device.CreateBuffer(&buffer_desc, &sub_data, &mut buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            // println!("Buffer create OK! {} {}",  len_bytes, bind_flags);
            self.last_size = len_slots;
            self.buffer = Some(unsafe {ComPtr::from_raw(buffer as *mut _)});
        }
        else {
            panic!("Buffer create failed {}", len_slots);
        }
    }
}



#[derive(Default)]
pub struct CxPlatformTexture {
    width: usize,
    height: usize,
    slots_per_pixel: usize,
    texture: Option<ComPtr<d3d11::ID3D11Texture2D >>,
    shader_resource: Option<ComPtr<d3d11::ID3D11ShaderResourceView >>,
    d3d11_resource: Option<ComPtr<d3d11::ID3D11Resource >>,
    render_target_view: Option<ComPtr<d3d11::ID3D11RenderTargetView >>,
    depth_stencil_view: Option<ComPtr<d3d11::ID3D11DepthStencilView >>
}

#[derive(Default, Clone)]
pub struct CxPlatformPass {
    pass_uniforms: D3d11Buffer,
    blend_state: Option<ComPtr<d3d11::ID3D11BlendState >>,
    raster_state: Option<ComPtr<d3d11::ID3D11RasterizerState >>,
    depth_stencil_state: Option<ComPtr<d3d11::ID3D11DepthStencilState >>
}

#[derive(Default, Clone)]
pub struct CxPlatformGeometry {
    pub geom_vbuf: D3d11Buffer,
    pub geom_ibuf: D3d11Buffer,
}

#[derive(Clone)]
pub struct CxPlatformShader {
    pub hlsl_shader: String,
    pub const_table_uniforms: D3d11Buffer,
    pub live_uniforms: D3d11Buffer,
    pub pixel_shader: ComPtr<d3d11::ID3D11PixelShader>,
    pub vertex_shader: ComPtr<d3d11::ID3D11VertexShader>,
    pub pixel_shader_blob: ComPtr<d3dcommon::ID3DBlob>,
    pub vertex_shader_blob: ComPtr<d3dcommon::ID3DBlob>,
    pub input_layout: ComPtr<d3d11::ID3D11InputLayout>
}

/*pub const MAPPED_TEXTURE_BUFFER_COUNT: usize = 4;

    /*
    pub fn create_depth_stencil_buffer(&self, wg: &WindowGeom)
        -> Result<ComPtr<d3d11::ID3D11Texture2D>, winerror::HRESULT> {
        let mut depth_stencil_buffer = ptr::null_mut();
        let texture2d_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: (wg.inner_size.x * wg.dpi_factor) as u32,
            Height: (wg.inner_size.y * wg.dpi_factor) as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgiformat::DXGI_FORMAT_D24_UNORM_S8_UINT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0
            },
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_DEPTH_STENCIL,
            CPUAccessFlags: 0,
            MiscFlags: 0
        };
        let hr = unsafe {self.device.CreateTexture2D(&texture2d_desc, ptr::null_mut(), &mut depth_stencil_buffer as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            Ok(unsafe {ComPtr::from_raw(depth_stencil_buffer as *mut _)})
        }
        else {
            Err(hr)
        }
    }*/
    

#[derive(Default, Clone)]
pub struct CxPlatformTextureResource {
    map_ptr: Option<*mut c_void>,
    locked: bool,
    user_data: Option<usize>,
}
#[derive(Default)]
pub struct CxPlatformTextureMapped {
    read: usize,
    write: usize,
    repaint_read: Option<usize>,
    repaint_id: u64,
    textures: [CxPlatformTextureResource; MAPPED_TEXTURE_BUFFER_COUNT],
}
pub struct CxThread {
    cx: u64
}
pub fn new_cxthread(&mut self) -> CxThread {
        CxThread {cx: self as *mut _ as u64}
    }
    
    pub fn get_mapped_texture_user_data(&mut self, texture: &Texture) -> Option<usize> {
        if let Some(texture_id) = texture.texture_id {
            let cxtexture = &self.textures[texture_id];
            if let Ok(mapped) = cxtexture.platform.mapped.lock() {
                return mapped.textures[mapped.read % MAPPED_TEXTURE_BUFFER_COUNT].user_data;
            }
        }
        None
    }
    
impl CxThread {
    
    pub fn lock_mapped_texture_f32(&mut self, texture: &Texture, user_data: usize) -> Option<&mut [f32]> {
        if let Some(texture_id) = texture.texture_id {
            let cx = unsafe {&mut *(self.cx as *mut Cx)};
            let cxtexture = &mut cx.textures[texture_id];
            if let Ok(mut mapped) = cxtexture.platform.mapped.lock() {
                let write = mapped.write;
                let texture = &mut mapped.textures[write % MAPPED_TEXTURE_BUFFER_COUNT];
                if let Some(ptr) = texture.map_ptr {
                    texture.user_data = Some(user_data);
                    if texture.locked == true {
                        panic!("lock_mapped_texture_f32 ran twice for the same texture, pair with unlock_mapped_texture!")
                    }
                    texture.locked = true;
                    return Some(unsafe {std::slice::from_raw_parts_mut(
                        ptr as *mut f32,
                        texture.width * texture.height * texture.slots_per_pixel
                    )});
                }
            };
        }
        None
    }
    
    pub fn lock_mapped_texture_u32(&mut self, texture: &Texture, user_data: usize) -> Option<&mut [u32]> {
        if let Some(texture_id) = texture.texture_id {
            let cx = unsafe {&mut *(self.cx as *mut Cx)};
            let cxtexture = &mut cx.textures[texture_id];
            if let Ok(mut mapped) = cxtexture.platform.mapped.lock() {
                let write = mapped.write;
                let texture = &mut mapped.textures[write % MAPPED_TEXTURE_BUFFER_COUNT];
                if let Some(ptr) = texture.map_ptr {
                    texture.user_data = Some(user_data);
                    if texture.locked == true {
                        panic!("lock_mapped_texture_u32 ran twice for the same texture, pair with unlock_mapped_texture!")
                    }
                    texture.locked = true;
                    return Some(unsafe {std::slice::from_raw_parts_mut(
                        ptr as *mut u32,
                        texture.width * texture.height * texture.slots_per_pixel
                    )});
                }
            };
        }
        None
    }
    
    pub fn unlock_mapped_texture(&mut self, texture: &Texture) {
        if let Some(texture_id) = texture.texture_id {
            let cx = unsafe {&mut *(self.cx as *mut Cx)};
            let cxtexture = &mut cx.textures[texture_id];
            if let Ok(mut mapped) = cxtexture.platform.mapped.lock() {
                // lets try to get the texture at mapped.write
                let write = mapped.write;
                let texture = &mut mapped.textures[write % MAPPED_TEXTURE_BUFFER_COUNT];
                if !texture.locked { // no-op
                    return
                }
                texture.locked = false;
                mapped.write += 1; // increase the write pointer
            };
        }
    }
}
/*
    
                            /*
                        TextureFormat::ImageBGRAf32 => {},
                        TextureFormat::ImageRf32 => {
                            if cxtexture.update_image {
                                cxtexture.update_image = false;
                                d3d11_cx.update_platform_texture_image_rf32(
                                    &mut cxtexture.platform.single,
                                    cxtexture.desc.width.unwrap(),
                                    cxtexture.desc.height.unwrap(),
                                    &cxtexture.image_f32,
                                );
                            }
                            d3d11_cx.set_shader_resource(i, &cxtexture.platform.single.shader_resource);
                        },
                        TextureFormat::ImageRGf32 => {
                            if cxtexture.update_image {
                                cxtexture.update_image = false;
                                d3d11_cx.update_platform_texture_image_rgf32(
                                    &mut cxtexture.platform.single,
                                    cxtexture.desc.width.unwrap(),
                                    cxtexture.desc.height.unwrap(),
                                    &cxtexture.image_f32,
                                );
                            }
                            d3d11_cx.set_shader_resource(i, &cxtexture.platform.single.shader_resource);
                        },
                        TextureFormat::MappedRGf32 | TextureFormat::MappedRf32 | TextureFormat::MappedBGRA | TextureFormat::MappedBGRAf32 => {
                            if let Ok(mut mapped) = cxtexture.platform.mapped.lock() {
                                if mapped.textures[0].texture.is_none() {
                                    d3d11_cx.init_mapped_textures(
                                        &mut *mapped,
                                        &cxtexture.desc.format,
                                        cxtexture.desc.width.unwrap(),
                                        cxtexture.desc.height.unwrap()
                                    );
                                }
                                if let Some(index) = d3d11_cx.flip_mapped_textures(&mut mapped, repaint_id) {
                                    d3d11_cx.set_shader_resource(i, &mapped.textures[index].shader_resource);
                                }
                            }
                        },*/
                        
    pub fn init_mapped_textures(&self, mapped: &mut CxPlatformTextureMapped, texture_format: &TextureFormat, width: usize, height: usize) {
        let slots_per_pixel;
        let format;
        match texture_format {
            TextureFormat::MappedRGf32 => {
                slots_per_pixel = 2;
                format = dxgiformat::DXGI_FORMAT_R32G32_FLOAT;
            }
            TextureFormat::MappedRf32 => {
                slots_per_pixel = 1;
                format = dxgiformat::DXGI_FORMAT_R32_FLOAT;
            }
            TextureFormat::MappedBGRA => {
                slots_per_pixel = 1;
                format = dxgiformat::DXGI_FORMAT_R8G8B8A8_UNORM;
            }
            TextureFormat::MappedBGRAf32 => {
                slots_per_pixel = 4;
                format = dxgiformat::DXGI_FORMAT_R32G32B32A32_FLOAT;
            },
            _ => {
                panic!("Wrong format for init_mapped_texures");
            }
        }
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {Count: 1, Quality: 0},
            Usage: d3d11::D3D11_USAGE_DYNAMIC,
            BindFlags: d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: d3d11::D3D11_CPU_ACCESS_WRITE,
            MiscFlags: 0,
        };
        for i in 0..MAPPED_TEXTURE_BUFFER_COUNT {
            let mut texture = ptr::null_mut();
            let hr = unsafe {self.device.CreateTexture2D(&texture_desc, ptr::null(), &mut texture as *mut *mut _)};
            if winerror::SUCCEEDED(hr) {
                let mut shader_resource = ptr::null_mut();
                unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
                mapped.textures[i].slots_per_pixel = slots_per_pixel;
                mapped.textures[i].width = width;
                mapped.textures[i].height = height;
                mapped.textures[i].texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
                let mut shader_resource = ptr::null_mut();
                unsafe {self.device.CreateShaderResourceView(texture as *mut _, ptr::null(), &mut shader_resource as *mut *mut _)};
                
                mapped.textures[i].shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
                let mut d3d11_resource = ptr::null_mut();
                unsafe {mapped.textures[i].texture.as_ref().unwrap().QueryInterface(
                    &d3d11::ID3D11Resource::uuidof(),
                    &mut d3d11_resource as *mut *mut _
                )};
                mapped.textures[i].d3d11_resource = Some(unsafe {ComPtr::from_raw(d3d11_resource as *mut _)});
                // map them by default
                self.map_texture(&mut mapped.textures[i]);
            }
            else {
                panic!("init_mapped_textures failed");
            }
        }
    }
    
    pub fn flip_mapped_textures(&self, mapped: &mut CxPlatformTextureMapped, repaint_id: u64) -> Option<usize> {
        if mapped.repaint_id == repaint_id {
            return mapped.repaint_read
        }
        mapped.repaint_id = repaint_id;
        for i in 0..MAPPED_TEXTURE_BUFFER_COUNT {
            if mapped.write > mapped.read && i == mapped.read % MAPPED_TEXTURE_BUFFER_COUNT {
                if mapped.textures[i].locked {
                    panic!("Texture locked whilst unmapping, should never happen!")
                }
                self.unmap_texture(&mut mapped.textures[i]);
            }
            else if mapped.read == 0 || i != (mapped.read - 1) % MAPPED_TEXTURE_BUFFER_COUNT { // keep a safety for not stalling gpu
                self.map_texture(&mut mapped.textures[i]);
            }
        }
        if mapped.read != mapped.write && mapped.textures[mapped.read % MAPPED_TEXTURE_BUFFER_COUNT].map_ptr.is_none() {
            let read = mapped.read;
            if mapped.read != mapped.write && mapped.write > mapped.read {
                if mapped.write > mapped.read + 1 { // space ahead
                    mapped.read += 1;
                }
            }
            mapped.repaint_read = Some(read % MAPPED_TEXTURE_BUFFER_COUNT);
            return mapped.repaint_read;
        }
        mapped.repaint_read = None;
        return None
    }
    
    pub fn map_texture(&self, texture: &mut CxPlatformTextureResource) {
        if !texture.map_ptr.is_none() {
            return
        }
        unsafe {
            let mut mapped_resource = std::mem::zeroed();
            let d3d11_resource = texture.d3d11_resource.as_ref().unwrap();
            let hr = self.context.Map(d3d11_resource.as_raw(), 0, d3d11::D3D11_MAP_WRITE_DISCARD, 0, &mut mapped_resource);
            if winerror::SUCCEEDED(hr) {
                texture.map_ptr = Some(mapped_resource.pData);
            }
        }
    }
    
    pub fn unmap_texture(&self, texture: &mut CxPlatformTextureResource) {
        if texture.map_ptr.is_none() {
            return
        }
        let d3d11_resource = texture.d3d11_resource.as_ref().unwrap();
        unsafe {
            self.context.Unmap(d3d11_resource.as_raw(), 0);
        }
        texture.map_ptr = None;
    }*/
    
    /*
    pub fn update_platform_texture_image_rf32(&self, res: &mut CxPlatformTextureResource, width: usize, height: usize, image_f32: &Vec<f32>) {
        if image_f32.len() != width * height {
            println!("update_platform_texture_image_rf32 with wrong buffer_u32 size!");
            return;
        }
        
        let mut texture = ptr::null_mut();
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: image_f32.as_ptr() as *const _,
            SysMemPitch: (width * 4) as u32,
            SysMemSlicePitch: 0
        };
        
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgiformat::DXGI_FORMAT_R32_FLOAT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0
            },
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, &sub_data, &mut texture as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(
                texture as *mut _,
                ptr::null(),
                &mut shader_resource as *mut *mut _
            )};
            res.width = width;
            res.height = height;
            res.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            res.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
        }
        else {
            panic!("update_platform_texture_image_rf32 failed");
        }
    }
    
    
    pub fn update_platform_texture_image_rgf32(&self, res: &mut CxPlatformTextureResource, width: usize, height: usize, image_f32: &Vec<f32>) {
        if image_f32.len() != width * height * 2 {
            println!("update_platform_texture_image_rgf32 with wrong buffer_f32 size {} {}!", width * height * 2, image_f32.len());
            return;
        }
        
        let mut texture = ptr::null_mut();
        let sub_data = d3d11::D3D11_SUBRESOURCE_DATA {
            pSysMem: image_f32.as_ptr() as *const _,
            SysMemPitch: (width * 8) as u32,
            SysMemSlicePitch: 0
        };
        
        let texture_desc = d3d11::D3D11_TEXTURE2D_DESC {
            Width: width as u32,
            Height: height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgiformat::DXGI_FORMAT_R32G32_FLOAT,
            SampleDesc: dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0
            },
            Usage: d3d11::D3D11_USAGE_DEFAULT,
            BindFlags: d3d11::D3D11_BIND_SHADER_RESOURCE,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        
        let hr = unsafe {self.device.CreateTexture2D(&texture_desc, &sub_data, &mut texture as *mut *mut _)};
        if winerror::SUCCEEDED(hr) {
            let mut shader_resource = ptr::null_mut();
            unsafe {self.device.CreateShaderResourceView(
                texture as *mut _,
                ptr::null(),
                &mut shader_resource as *mut *mut _
            )};
            res.texture = Some(unsafe {ComPtr::from_raw(texture as *mut _)});
            res.width = width;
            res.height = height;
            res.shader_resource = Some(unsafe {ComPtr::from_raw(shader_resource as *mut _)});
        }
        else {
            panic!("update_platform_texture_image_rgf32 failed");
        }
    }*/

*/