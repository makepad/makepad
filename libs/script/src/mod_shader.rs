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
pub const SHADER_IO_VARYING: ShaderIoType = ShaderIoType(4);
pub const SHADER_IO_VERTEX_POSITION: ShaderIoType = ShaderIoType(5);
pub const SHADER_IO_FRAGMENT_OUTPUT0: ShaderIoType = ShaderIoType(6);
pub const SHADER_IO_FRAGMENT_OUTPUT1: ShaderIoType = ShaderIoType(7);
pub const SHADER_IO_FRAGMENT_OUTPUT2: ShaderIoType = ShaderIoType(8);
pub const SHADER_IO_FRAGMENT_OUTPUT3: ShaderIoType = ShaderIoType(9);
pub const SHADER_IO_FRAGMENT_OUTPUT4: ShaderIoType = ShaderIoType(10);
pub const SHADER_IO_FRAGMENT_OUTPUT5: ShaderIoType = ShaderIoType(11);
pub const SHADER_IO_FRAGMENT_OUTPUT7: ShaderIoType = ShaderIoType(12);

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
    
    native.add_method(heap, shader, id!(fragment_output), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(vm.code.builtins.pod.pod_vec4f.into());
        let index = value.as_index().min(7) as u32;
        vm.heap.set_shader_io(obj, ShaderIoType(SHADER_IO_FRAGMENT_OUTPUT0.0 + index));
        obj.into()
    });
        
        
    native.add_method(heap, shader, id!(compile_draw), script_args!(io_self=NIL), |vm, args|{
        // lets fetch the code
        let io_self = script_value!(vm, args.io_self);
        
        // ok we're going to take a function, and then call it to generate/typetrace it out
        // for every function we make a 'shadercompiler'
        if let Some(io_self) = io_self.as_object(){
            let mut output = ShaderOutput::default();
            if let Some(fnobj) = vm.heap.object_method(io_self, id!(vertex).into(), &vm.thread.trap).as_object(){
                output.backend = ShaderBackend::Metal;
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
            // alright on metal we now need to generate the structs
            
            
            // alright we should have output now
            let mut out = String::new();
            output.create_struct_defs(vm, &mut out);
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
