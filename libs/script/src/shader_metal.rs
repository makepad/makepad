use std::fmt::Write;
use crate::vm::ScriptVm;
use crate::shader::{ShaderOutput, ShaderIoKind};

impl ShaderOutput {
    pub fn metal_create_io_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct Io {{").ok();
        writeln!(out, "    constant IoUniform *u;").ok();
        writeln!(out, "    constant IoInstance *i;").ok();
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Texture => {
                    writeln!(out, "    texture2d<float> {};", io.name).ok();
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    sampler {};", io.name).ok();
                }
                ShaderIoKind::UniformBuffer => {
                    write!(out, "    constant ").ok();
                    self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                    writeln!(out, " *u_{};", io.name).ok();
                }
                _=>()
            }
        }
        
        let mut have_vb = false;
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                if !have_vb{
                    writeln!(out, "    constant IoVertexBuffer *vb;").ok();
                    have_vb = true;
                }
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_instance_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoInstance {{").ok();
        
        // 1. Output Dyn instance fields first (order doesn't matter, just output as encountered)
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        
        // 2. Output Rust instance fields last (already in correct order from pre_collect_rust_instance_io)
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_uniform_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoUniform {{").ok();
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Uniform => {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
                _=>()
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_varying_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoVarying {{").ok();
        for io in &self.io {
            match io.kind {
                ShaderIoKind::Varying => {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
                _=>()
            }
        }
        writeln!(out, "    float4 _position [[position]];").ok();
        writeln!(out, "    uint _iid [[flat]];").ok();
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_vertex_buffer_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoVertexBuffer {{").ok();
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_io_vertex_struct(&self, _vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoV {{").ok();
        writeln!(out, "    IoVarying v;").ok();
        writeln!(out, "    uint vid;").ok();
        writeln!(out, "    uint iid;").ok();
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_vertex_fn(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "vertex IoVarying vertex_main(").ok();
        writeln!(out, "    constant IoVertexBuffer *vb [[buffer(0)]],").ok();
        writeln!(out, "    constant IoInstance *i [[buffer(1)]],").ok();
        writeln!(out, "    constant IoUniform *u [[buffer(2)]],").ok();
        
        let mut buf_idx = 3;
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                write!(out, "    constant ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                writeln!(out, " *u_{} [[buffer({})]],", io.name, buf_idx).ok();
                buf_idx += 1;
            }
        }
        
        let mut tex_idx = 0;
        let mut samp_idx = 0;
        for io in &self.io {
            match io.kind {
                ShaderIoKind::Texture => {
                    writeln!(out, "    texture2d<float> {} [[texture({})]],", io.name, tex_idx).ok();
                    tex_idx += 1;
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    sampler {} [[sampler({})]],", io.name, samp_idx).ok();
                    samp_idx += 1;
                }
                _=>()
            }
        }
        
        writeln!(out, "    uint vid [[vertex_id]],").ok();
        writeln!(out, "    uint iid [[instance_id]]").ok();
        writeln!(out, ") {{").ok();
        
        writeln!(out, "    Io _io;").ok();
        writeln!(out, "    _io.vb = vb;").ok();
        writeln!(out, "    _io.i = i;").ok();
        writeln!(out, "    _io.u = u;").ok();
        
        for io in &self.io {
            match io.kind {
                ShaderIoKind::UniformBuffer => {
                    writeln!(out, "    _io.u_{} = u_{};", io.name, io.name).ok();
                }
                ShaderIoKind::Texture => {
                    writeln!(out, "    _io.{} = {};", io.name, io.name).ok();
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    _io.{} = {};", io.name, io.name).ok();
                }
                _=>()
            }
        }
        
        writeln!(out, "    IoV _iov;").ok();
        writeln!(out, "    _iov.vid = vid;").ok();
        writeln!(out, "    _iov.iid = iid;").ok();
        writeln!(out, "    io_vertex(_io, _iov);").ok();
        writeln!(out, "    return _iov.v;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn metal_create_fragment_main_fn(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "fragment IoFb fragment_main(").ok();
        writeln!(out, "    IoVarying v [[stage_in]],").ok();
        writeln!(out, "    constant IoVertexBuffer *vb [[buffer(0)]],").ok();
        writeln!(out, "    constant IoInstance *i [[buffer(1)]],").ok();
        write!(out, "    constant IoUniform *u [[buffer(2)]]").ok();
        
        let mut buf_idx = 3;
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                writeln!(out, ",").ok();
                write!(out, "    constant ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                write!(out, " *u_{} [[buffer({})]]", io.name, buf_idx).ok();
                buf_idx += 1;
            }
        }
        
        let mut tex_idx = 0;
        let mut samp_idx = 0;
        for io in &self.io {
            match io.kind {
                ShaderIoKind::Texture => {
                    writeln!(out, ",").ok();
                    write!(out, "    texture2d<float> {} [[texture({})]]", io.name, tex_idx).ok();
                    tex_idx += 1;
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, ",").ok();
                    write!(out, "    sampler {} [[sampler({})]]", io.name, samp_idx).ok();
                    samp_idx += 1;
                }
                _=>()
            }
        }
        
        writeln!(out, ") {{").ok();
        
        writeln!(out, "    Io _io;").ok();
        writeln!(out, "    _io.vb = vb;").ok();
        writeln!(out, "    _io.i = i;").ok();
        writeln!(out, "    _io.u = u;").ok();
        
        for io in &self.io {
            match io.kind {
                ShaderIoKind::UniformBuffer => {
                    writeln!(out, "    _io.u_{} = u_{};", io.name, io.name).ok();
                }
                ShaderIoKind::Texture => {
                    writeln!(out, "    _io.{} = {};", io.name, io.name).ok();
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    _io.{} = {};", io.name, io.name).ok();
                }
                _=>()
            }
        }
        
        writeln!(out, "    IoFb _iofb;").ok();
        writeln!(out, "    IoF _iof;").ok();
        writeln!(out, "    _iof.v = &v;").ok();
        writeln!(out, "    _iof.fb = &_iofb;").ok();
        writeln!(out, "    io_fragment(_io, _iof);").ok();
        writeln!(out, "    return _iofb;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn metal_create_io_fragment_struct(&self, _vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoF {{").ok();
        writeln!(out, "    thread IoVarying *v;").ok();
        writeln!(out, "    thread IoFb *fb;").ok();
        writeln!(out, "}};").ok();
    }
    
    pub fn metal_create_io_framebuffer_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoFb {{").ok();
        for io in &self.io {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                writeln!(out, " fb{} [[color({})]];", index, index).ok();
            }
        }
        writeln!(out, "}};").ok();
    }
}
