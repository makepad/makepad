
use crate::*;
use std::fs;
use makepad_script::*;
use makepad_script::id;
use std::io::Read;
use std::io::Write;

pub fn define_fs_module(vm:&mut ScriptVm){
    let fs = vm.new_module(id!(fs));
    
    for sym in [id_lut!(read), id_lut!(read_to_string)]{    
        vm.add_method(fs, sym, script_args_def!(path=NIL), |vm, args|{
            let path =  script_value!(vm, args.path);
            if let Some(Some(mut file)) = vm.heap.string_with(path, |_heap,s|{
                fs::File::open(s).ok()
            }){
                let thread = &vm.thread;
                vm.heap.new_string_with(|_heap, s|{
                    if file.read_to_string(s).is_err(){
                        script_err_file_system!(thread.trap.pass(), "file system error");
                    }
                }).into()
            }
            else{
                script_err_file_system!(vm.thread.trap.pass(), "file system error")
            }
        })
    }
    for sym in [id_lut!(write), id_lut!(write_string)]{    
        vm.add_method(fs, sym, script_args_def!(path=NIL, data=NIL), |vm, args|{
            let path =  script_value!(vm, args.path);
            let data =  script_value!(vm, args.data);
            if let Some(Some(mut file)) = vm.heap.string_with(path, |_heap,s|{
                fs::File::create(s).ok()
            }){
                let thread = &vm.thread;
                if data.is_string_like(){
                    vm.heap.string_with(data, |_heap,s|{
                        if file.write_all(&s.as_bytes()).is_err(){
                            script_err_file_system!(thread.trap.pass(), "file system error");
                        }
                    });
                }
                else if let Some(data) = data.as_array(){
                    match vm.heap.array_storage(data){
                        ScriptArrayStorage::U8(data)=>{
                            if file.write_all(&data).is_err(){
                                script_err_file_system!(thread.trap.pass(), "file system error");
                            }
                        }
                        _=>{
                            script_err_invalid_arg_type!(vm.thread.trap.pass(), "invalid fs arg type");
                        }
                    }
                    
                }
                return NIL
            }
            else{
                script_err_file_system!(vm.thread.trap.pass(), "file system error")
            }
        })
    }
}
