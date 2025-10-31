
use crate::*;
use std::fs;
use makepad_script::*;
use makepad_script::id;
use makepad_script::array::*;
use std::io::Read;
use std::io::Write;

pub fn define_fs_module(vm:&mut ScriptVm){
    let fs = vm.new_module(id!(fs));
    
    for sym in [id!(read), id!(read_to_string)]{    
        vm.add_fn(fs, sym, script_args_def!(path=NIL), |vm, args|{
            let path =  script_value!(vm, args.path);
            if let Some(Some(mut file)) = vm.heap.string_with(path, |_heap,s|{
                fs::File::open(s).ok()
            }){
                let thread = &vm.thread;
                vm.heap.new_string_with(|_heap, s|{
                    if file.read_to_string(s).is_err(){
                        thread.trap.err_filesystem();
                    }
                }).into()
            }
            else{
                vm.thread.trap.err_filesystem()
            }
        })
    }
    for sym in [id!(write), id!(write_string)]{    
        vm.add_fn(fs, sym, script_args_def!(path=NIL, data=NIL), |vm, args|{
            let path =  script_value!(vm, args.path);
            let data =  script_value!(vm, args.data);
            if let Some(Some(mut file)) = vm.heap.string_with(path, |_heap,s|{
                fs::File::create(s).ok()
            }){
                let thread = &vm.thread;
                if data.is_string_like(){
                    vm.heap.string_with(data, |_heap,s|{
                        if file.write_all(&s.as_bytes()).is_err(){
                            thread.trap.err_filesystem();
                        }
                    });
                }
                else if let Some(data) = data.as_array(){
                    match vm.heap.array_ref(data){
                        ScriptArrayStorage::U8(data)=>{
                            if file.write_all(&data).is_err(){
                                thread.trap.err_filesystem();
                            }
                        }
                        _=>{
                            vm.thread.trap.err_invalid_arg_type();
                        }
                    }
                    
                }
                return NIL
            }
            else{
                vm.thread.trap.err_filesystem()
            }
        })
    }
}
