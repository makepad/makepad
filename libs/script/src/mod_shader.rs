#[allow(unused)]
use crate::*;
use makepad_live_id::*;
use crate::heap::*;
use crate::native::*;
use crate::value::*;
use crate::shader::*;
use crate::function::*;

pub struct ShaderIoType(pub(crate) u32);

pub const SHADER_IO_INSTANCE: ShaderIoType = ShaderIoType(0);

pub fn define_shader_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let shader = heap.new_module(id!(shader));
    
    native.add_method(heap, shader, id!(instance), script_args!(value=NIL), |vm, args|{
        let value = script_value!(vm, args.value);
        let obj = vm.heap.new_with_proto(value);
        vm.heap.set_shader_io(obj, SHADER_IO_INSTANCE);
        obj.into()
    });
    
    native.add_method(heap, shader, id!(compile_draw), script_args!(io_this=NIL), |vm, args|{
        // lets fetch the code
        let io_this = script_value!(vm, args.io_this);
        
        // ok we're going to take a function, and then call it to generate/typetrace it out
        // for every function we make a 'shadercompiler'
        if let Some(io_this) = io_this.as_object(){
            if let Some(fnobj) = vm.heap.object_method(io_this, id!(pixel).into(), &vm.thread.trap).as_object(){
                if let Some(fnptr) = vm.heap.as_fn(fnobj){
                    if let ScriptFnPtr::Script(fnip) = fnptr{
                        let mut compiler = ShaderFnCompiler::new(fnobj);
                        // compiling the entrypoint pixelshader
                        let mut output = ShaderOutput::default();
                        compiler.shader_scope.define_this_io(io_this);
                        compiler.compile_fn(vm, &mut output, fnip);
                        output.functions.push(ShaderFn{
                            overload: 0,
                            name: id!(pixel),
                            call_sig: "fn pixel()".into(),
                            args:Default::default(),
                            fnobj,
                            out: compiler.out,
                            ret: vm.code.builtins.pod.pod_void
                        });
                        // alright we should have output now
                        let mut out = String::new();
                        output.create_struct_defs(vm, &mut out);
                        println!("Structs:\n{}", out);
                        for fns in output.functions{
                            println!("{}\n{}\n",fns.call_sig, fns.out);
                        }
                        return NIL
                    }
                }
            }
        }
        // trap error
        NIL
    });
}
