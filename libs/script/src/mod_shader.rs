#[allow(unused)]
use crate::*;
use makepad_live_id::*;
use crate::heap::*;
use crate::native::*;
use crate::value::*;
use crate::shader::*;
use crate::shader_backend::*;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ShaderIoType(pub(crate) u32);

pub const SHADER_IO_RUST_INSTANCE: ShaderIoType = ShaderIoType(0);
pub const SHADER_IO_DYN_INSTANCE: ShaderIoType = ShaderIoType(1);
pub const SHADER_IO_DYN_UNIFORM: ShaderIoType = ShaderIoType(2);
pub const SHADER_IO_UNIFORM_BUFFER: ShaderIoType = ShaderIoType(3);
pub const SHADER_IO_VERTEX_BUFFER: ShaderIoType = ShaderIoType(4);
pub const SHADER_IO_VARYING: ShaderIoType = ShaderIoType(5);
pub const SHADER_IO_VERTEX_POSITION: ShaderIoType = ShaderIoType(6);
pub const SHADER_IO_TEXTURE_1D: ShaderIoType = ShaderIoType(7);
pub const SHADER_IO_TEXTURE_1D_ARRAY: ShaderIoType = ShaderIoType(9);
pub const SHADER_IO_TEXTURE_2D: ShaderIoType = ShaderIoType(10);
pub const SHADER_IO_TEXTURE_2D_ARRAY: ShaderIoType = ShaderIoType(11);
pub const SHADER_IO_TEXTURE_3D: ShaderIoType = ShaderIoType(12);
pub const SHADER_IO_TEXTURE_3D_ARRAY: ShaderIoType = ShaderIoType(13);
pub const SHADER_IO_TEXTURE_CUBE: ShaderIoType = ShaderIoType(14);
pub const SHADER_IO_TEXTURE_CUBE_ARRAY: ShaderIoType = ShaderIoType(15);
pub const SHADER_IO_TEXTURE_DEPTH: ShaderIoType = ShaderIoType(16);
pub const SHADER_IO_TEXTURE_DEPTH_ARRAY: ShaderIoType = ShaderIoType(17);
pub const SHADER_IO_SAMPLER: ShaderIoType = ShaderIoType(18);
pub const SHADER_IO_BUFFER_R: ShaderIoType = ShaderIoType(19);
pub const SHADER_IO_BUFFER_W: ShaderIoType = ShaderIoType(20);
pub const SHADER_IO_BUFFER_RW: ShaderIoType = ShaderIoType(21);
pub const SHADER_IO_FRAGMENT_OUTPUT_0: ShaderIoType = ShaderIoType(22);
pub const SHADER_IO_FRAGMENT_OUTPUT_MAX: ShaderIoType = ShaderIoType(SHADER_IO_FRAGMENT_OUTPUT_0.0 + 7);

pub fn define_shader_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let shader = heap.new_module(id!(shader));
    
    native.add_method(heap, shader, id!(instance), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_DYN_INSTANCE);
        obj.into()
    });

    native.add_method(heap, shader, id!(uniform), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_DYN_UNIFORM);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(uniform_buffer), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_UNIFORM_BUFFER);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(vertex_buffer), script_args!(value=NIL, buf=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let buffer = script_value!(vm, args.buf);
        let obj = vm.heap.new_with_proto(value);
        set_script_value!(vm, obj.buffer = buffer);
        vm.heap.set_shader_io(obj, SHADER_IO_VERTEX_BUFFER);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(varying), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_VARYING);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(vertex_position), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_VERTEX_POSITION);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_1d), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_1D);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_1d_array), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_1D_ARRAY);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_2d), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_2D);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_2d_array), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_2D_ARRAY);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_3d), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_3D);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_3d_array), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_3D_ARRAY);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_cube), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_CUBE);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_cube_array), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_CUBE_ARRAY);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_depth), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_DEPTH);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(texture_depth_array), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_TEXTURE_DEPTH_ARRAY);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(fragment_output), script_args!(index=NIL, ty=NIL), |vm, args|{
        let index = script_value!(vm, args.index);
        let ty = script_value!(vm, args.ty);
        let obj = vm.heap.new_with_proto(ty);
        let index = index.as_index().min(7) as u32;
        vm.heap.set_shader_io(obj, ShaderIoType(SHADER_IO_FRAGMENT_OUTPUT_0.0 + index));
        obj.into()
    });
        
    native.add_method(heap, shader, id!(compile_draw), script_args!(io_self=NIL), |vm, args|{
        // lets fetch the code
        let io_self = script_value!(vm, args.io_self);
        
        // ok we're going to take a function, and then call it to generate/typetrace it out
        // for every function we make a 'shadercompiler'
        if let Some(io_self) = io_self.as_object(){
            let mut output = ShaderOutput::default();
            output.backend = ShaderBackend::Metal;
                        
            output.pre_collect_rust_instance_io(vm, io_self);
            output.pre_collect_fragment_outputs(vm, io_self);
            
            if let Some(fnobj) = vm.heap.object_method(io_self, id!(vertex).into(), &vm.thread.trap).as_object(){
                output.mode = ShaderMode::Vertex;
                ShaderFnCompiler::compile_shader_def(
                    vm, 
                    &mut output, 
                    id!(vertex), 
                    fnobj, 
                    ShaderType::IoSelf(io_self), 
                    vec![],
                );
            }
            if let Some(fnobj) = vm.heap.object_method(io_self, id!(fragment).into(), &vm.thread.trap).as_object(){
                output.mode = ShaderMode::Fragment;
                ShaderFnCompiler::compile_shader_def(
                    vm, 
                    &mut output, 
                    id!(fragment), 
                    fnobj, 
                    ShaderType::IoSelf(io_self), 
                    vec![],
                );
            }
            
            output.assign_uniform_buffer_indices(vm.heap, 3);
            
            let mut out = String::new();
            output.create_struct_defs(vm, &mut out);
            output.metal_create_instance_struct(vm, &mut out);
            output.metal_create_uniform_struct(vm, &mut out);
            output.metal_create_io_struct(vm, &mut out);
            output.metal_create_varying_struct(vm, &mut out);
            output.metal_create_vertex_buffer_struct(vm, &mut out);
            output.metal_create_sampler_decls(&mut out);
            output.metal_create_io_vertex_struct(vm, &mut out);
            output.metal_create_vertex_fn(vm, &mut out);
            output.metal_create_io_fragment_struct(vm, &mut out);
            output.metal_create_fragment_main_fn(vm, &mut out);
            println!("Structs:\n{}", out);
            for fns in output.functions{
                println!("{}{{\n{}}}\n",fns.call_sig, fns.out);
            }
            return NIL
        }
        // trap error
        NIL
    });
}
