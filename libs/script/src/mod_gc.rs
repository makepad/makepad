use crate::heap::*;
use crate::makepad_error_log::*;
use crate::makepad_live_id::live_id::*;
use crate::makepad_live_id_macros::*;
use crate::native::*;
use crate::value::*;
use crate::*;

pub fn define_gc_module(heap: &mut ScriptHeap, native: &mut ScriptNative) {
    let gc = heap.new_module(id!(gc));

    native.add_method(
        heap,
        gc,
        id_lut!(set_static),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            vm.bx.heap.set_static(value);
            value
        },
    );

    native.add_method(heap, gc, id_lut!(run), script_args!(), |vm, _args| {
        vm.gc();
        ScriptValue::NIL
    });

    native.add_method(
        heap,
        gc,
        id_lut!(dump_tag),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            if let Some(obj) = value.as_object() {
                let object = &vm.bx.heap.objects[obj.index as usize];
                let tag = &object.tag;
                let type_index = tag.as_type_index().map(|t| t.0);
                let is_static = tag.is_static();
                let proto = object.proto;
                let proto_obj_index = proto.as_object().map(|p| p.index);
                let proto_type_index = if let Some(p) = proto.as_object() {
                    vm.bx.heap.objects[p.index as usize]
                        .tag
                        .as_type_index()
                        .map(|t| t.0)
                } else {
                    None
                };
                let type_props = if let Some(ty) = tag.as_type_index() {
                    let check = &vm.bx.heap.type_check[ty.0 as usize];
                    check
                        .props
                        .props
                        .keys()
                        .map(|k| format!("{:?}", k))
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    String::new()
                };
                log!(
                    "obj {} type_index={:?} is_static={} proto={:?} proto_type_index={:?} props=[{}]",
                    obj.index,
                    type_index,
                    is_static,
                    proto_obj_index,
                    proto_type_index,
                    type_props
                );
            }
            value
        },
    );
}
