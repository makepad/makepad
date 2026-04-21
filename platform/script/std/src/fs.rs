use makepad_script::id;
use makepad_script::*;
use std::fs;
use std::io::Read;
use std::io::Write;

pub fn script_mod(vm: &mut ScriptVm) {
    let fs = vm.new_module(id!(fs));

    for sym in [id_lut!(read), id_lut!(read_to_string)] {
        vm.add_method(fs, sym, script_args_def!(path = NIL), |vm, args| {
            let path = script_value!(vm, args.path);
            if let Some(Some(mut file)) = vm.string_with(path, |_vm, s| fs::File::open(s).ok()) {
                vm.new_string_with(|vm, s| {
                    if file.read_to_string(s).is_err() {
                        script_err_io!(vm.bx.threads.trap(), "file system error");
                    }
                })
                .into()
            } else {
                script_err_io!(vm.trap(), "file system error")
            }
        })
    }
    for sym in [id_lut!(write), id_lut!(write_string)] {
        vm.add_method(
            fs,
            sym,
            script_args_def!(path = NIL, data = NIL),
            |vm, args| {
                let path = script_value!(vm, args.path);
                let data = script_value!(vm, args.data);
                if let Some(Some(mut file)) =
                    vm.string_with(path, |_vm, s| fs::File::create(s).ok())
                {
                    if data.is_string_like() {
                        vm.string_with(data, |vm, s| {
                            if file.write_all(&s.as_bytes()).is_err() {
                                script_err_io!(vm.bx.threads.trap(), "file system error");
                            }
                        });
                    } else if let Some(data) = data.as_array() {
                        match vm.bx.heap.array_storage(data) {
                            ScriptArrayStorage::U8(data) => {
                                if file.write_all(&data).is_err() {
                                    script_err_io!(vm.trap(), "file system error");
                                }
                            }
                            _ => {
                                script_err_type_mismatch!(vm.trap(), "invalid fs arg type");
                            }
                        }
                    }
                    return NIL;
                } else {
                    script_err_io!(vm.trap(), "file system error")
                }
            },
        )
    }
}
