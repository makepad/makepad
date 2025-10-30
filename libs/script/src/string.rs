use crate::value::*;
use crate::heap::*;
use crate::array::*;
use crate::native::*;
use crate::makepad_live_id::*;
use crate::methods::*;
use std::rc::Rc;
use crate::*;
use std::borrow::Borrow;


#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct ScriptRcString(pub Rc<String>);

impl ScriptRcString{
    pub fn new(str:String)->Self{
        Self(Rc::new(str))
    }
}

impl Borrow<str> for ScriptRcString { 
    fn borrow(&self) -> &str{
        (*self.0).as_str()
    }
}

impl Borrow<String> for ScriptRcString { 
    fn borrow(&self) -> &String{
        &(*self.0)
    }
}

#[derive(Default)]
pub struct StringTag(u64);

impl StringTag{
    const MARK:u64 = 0x1;
    
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
            
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
            
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
}

#[derive(Default)]
pub struct ScriptStringData{
    pub tag: StringTag,
    pub string: ScriptRcString
}

impl ScriptStringData{
    pub fn add_type_methods(tm: &mut ScriptTypeMethods, h: &mut ScriptHeap, native:&mut ScriptNative){
        tm.add(h, native, &[], ScriptValueType::REDUX_STRING, id!(to_bytes), |vm, args|{
            let this = script_value!(vm, args.this);
            vm.heap.string_to_bytes_array(this).into()
        });
        tm.add(h, native, &[], ScriptValueType::REDUX_STRING, id!(to_chars), |vm, args|{
            let this = script_value!(vm, args.this);
            vm.heap.string_to_chars_array(this).into()
        });
        tm.add(h, native, &[], ScriptValueType::REDUX_STRING, id!(parse_json), |vm, args|{
            let this = script_value!(vm, args.this);
            
            if let Some(r) = vm.heap.string_mut_self_with(this, |heap,s|{
                vm.thread.json_parser.read_json(s, heap)
            }){
                r
            }
            else{
                vm.thread.trap.err_unexpected()
            }
        });
        tm.add(h, native, script_args_def!(pat = NIL), ScriptValueType::REDUX_STRING, id!(split), |vm, args|{
            let this = script_value!(vm, args.this);
            let pat = script_value!(vm, args.pat);
            if let Some(Some(s)) = vm.heap.string_mut_self_with(this,|heap,this|{
                heap.string_mut_self_with(pat,|heap,pat|{
                    let array = heap.new_array();
                    heap.array_mut_mut_self_with(array, |heap, storage|{
                        if let ScriptArrayStorage::ScriptValue(_) = storage{}
                        else{*storage = ScriptArrayStorage::ScriptValue(vec![]);}
                        if let ScriptArrayStorage::ScriptValue(vec) = storage{
                            vec.clear();
                            for s in this.split(pat){
                                vec.push(heap.new_string_from_str(s));
                            }
                        }
                    });
                    array
                })
            }){
                return s.into()
            }
            
            vm.thread.trap.err_unexpected()
        });
    }
}