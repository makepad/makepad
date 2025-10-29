
use crate::*;
use std::fs;
use makepad_script::*;
use makepad_script::id;
use std::io::Read;

pub fn define_fs_module(vm:&mut ScriptVm){
    let fs = vm.new_module(id!(fs));
        
    vm.add_fn(fs, id!(read_to_string), script_args!(path=NIL), |vm, args|{
        let path =  script_value!(vm, args.path);
        println!("READTOSTRING {}", path);
        if let Some(Some(mut file)) = vm.heap.string_with(path, |_heap,s|{
            println!("GOT PATH {}",s);
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
