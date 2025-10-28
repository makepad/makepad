use crate::tokenizer::*;
use crate::makepad_live_id::live_id::*;
use crate::value::*;
use crate::makepad_live_id::makepad_live_id_macros::*;
use crate::heap::*;

enum State{
    Root,
    ObjectKey(ScriptObject),
    ObjectColon(ScriptObject, ScriptValue),
    ObjectValue(ScriptObject, ScriptValue),
    Array(ScriptArray),
}

#[derive(Default)]
pub struct JsonParser{
    index: u32,
    pub root: ScriptValue,
    state: Vec<State>,
    pub errors: Vec<(u32, String)>
}

impl JsonParser{
    pub fn clear(&mut self){
        self.index = 0;
        self.root = NIL;
        self.state.clear();
        self.state.push(State::Root);
        self.errors.clear();
    }
}


impl JsonParser{
    fn parse_step(&mut self, tok:ScriptToken, heap:&mut ScriptHeap){
         match self.state.pop().unwrap(){
             State::Root=>{
                 match tok{
                     ScriptToken::Identifier{id, ..}=>{
                         match id{
                             id!(true)=>self.root = TRUE.into(),
                             id!(false)=>self.root = FALSE.into(),
                             id!(null)=>self.root = NIL.into(),
                             x=>self.root=x.into()
                         }
                     }
                     ScriptToken::String(v)=>{
                         self.root = v;
                     }
                     ScriptToken::Number(v)=>{
                         self.root = v.into();
                     }
                     ScriptToken::Color(v)=>{
                         self.root = v.into()
                     }
                     ScriptToken::OpenCurly=>{ // object
                         let new_obj = heap.new_object();
                         self.root = new_obj.into();
                         self.state.push(State::ObjectKey(new_obj));
                     }
                     ScriptToken::OpenSquare=>{ // 
                         let new_arr = heap.new_array();
                         self.root = new_arr.into();
                         self.state.push(State::Array(new_arr));
                     }
                     ScriptToken::CloseSquare | ScriptToken::CloseRound | ScriptToken::CloseCurly=>{
                         self.errors.push((self.index,format!("JsonParser: Unexpected ] or }} or ) in json root")));
                     }
                     x=>{
                         self.errors.push((self.index,format!("JsonParser: Unexpected token in json root {:?}", x)));
                     }
                 }
             }
             State::ObjectKey(obj)=>{
                // we require a string or identifier
                match tok{
                    ScriptToken::Identifier{id, ..}=>{
                        self.state.push(State::ObjectColon(obj, id.into()));
                    }
                    ScriptToken::String(v)=>{
                        self.state.push(State::ObjectColon(obj, v.into()));
                    }
                    ScriptToken::Number(v)=>{
                        self.state.push(State::ObjectColon(obj, v.into()));
                    }
                    ScriptToken::Color(v)=>{
                        self.state.push(State::ObjectColon(obj, v.into()));
                    }
                    // nonstandard, allow objects and arrays as keys
                    ScriptToken::OpenCurly=>{ // object
                        let new_obj = heap.new_object();
                        self.state.push(State::ObjectColon(obj, new_obj.into()));
                        self.state.push(State::ObjectKey(new_obj));
                    }
                    ScriptToken::OpenSquare=>{ // 
                        let new_arr = heap.new_array();
                        self.state.push(State::ObjectColon(obj, new_arr.into()));
                        self.state.push(State::Array(new_arr));
                    }
                    ScriptToken::CloseRound | ScriptToken::CloseSquare=>{
                        self.errors.push((self.index,format!("JsonParser: Unexpected ] or ) in object")));
                    }
                    ScriptToken::Operator(id)=>{
                        self.state.push(State::ObjectKey(obj));
                        match id{
                            id!(,)=>{}
                            x=>{
                                self.errors.push((self.index,format!("JsonParser: Unexpected operator in object {}", x)));
                            }
                        }
                    }
                    x=>{
                        self.state.push(State::ObjectKey(obj));
                        self.errors.push((self.index,format!("JsonParser: Unexpected token in object {:?}", x)));
                    }
                }
             }
             State::ObjectColon(obj, key)=>{
                 match tok{
                    ScriptToken::Operator(id)=>{
                        match id{
                           id!(:)=>{
                               self.state.push(State::ObjectValue(obj, key));
                           }
                           x=>{
                               self.state.push(State::ObjectKey(obj));
                               self.errors.push((self.index,format!("JsonParser: Unexpected operator expecting ':' {}", x)));
                           }
                        }
                    }
                    x=>{
                        self.state.push(State::ObjectKey(obj));
                        self.errors.push((self.index,format!("JsonParser: Unexpected token waiting for object ':' {:?}", x)));
                    }
                 }
             }
             State::ObjectValue(obj, key)=>{
                 self.state.push(State::ObjectKey(obj));
                 match tok{
                     ScriptToken::Identifier{id, ..}=>{
                         match id{
                             id!(true)=>heap.vec_push_unchecked(obj, key, TRUE),
                             id!(false)=>heap.vec_push_unchecked(obj, key, FALSE),
                             id!(null)=>heap.vec_push_unchecked(obj, key, NIL),
                             x=>heap.vec_push_unchecked(obj, x.into(), NIL),
                         }
                     }
                     ScriptToken::String(v)=>{
                         heap.vec_push_unchecked(obj, key, v);
                     }
                     ScriptToken::Number(v)=>{
                         heap.vec_push_unchecked(obj, key, v.into());
                     }
                     ScriptToken::Color(v)=>{
                         heap.vec_push_unchecked(obj, key, v.into());
                     }
                     ScriptToken::OpenCurly=>{ // object
                         let new_obj = heap.new_object();
                         heap.vec_push_unchecked(obj, key, new_obj.into());
                         self.state.push(State::ObjectKey(new_obj));
                     }
                     ScriptToken::OpenSquare=>{ // 
                         let new_arr = heap.new_array();
                         heap.vec_push_unchecked(obj, key, new_arr.into());
                         self.state.push(State::Array(new_arr));
                     }
                     ScriptToken::CloseCurly=>{
                         self.errors.push((self.index,format!("JsonParser: Unexpected }} expecting object value")));
                         self.state.pop();
                         // end of array
                     }
                     ScriptToken::CloseRound | ScriptToken::CloseSquare=>{
                         self.errors.push((self.index,format!("JsonParser: Unexpected ] or ) in array")));
                     }
                     ScriptToken::Operator(op)=>{
                         if op != id!(,){
                             self.errors.push((self.index,format!("JsonParser: Unexpected operator expecting ',' {}", op)));
                         }
                     }
                     x=>{
                         self.errors.push((self.index,format!("JsonParser: Unexpected token {:?}", x)));
                     }
                 }
             }
             State::Array(arr)=>{
                // alright we can parse a value or ]
                match tok{
                    ScriptToken::Identifier{id, ..}=>{
                        match id{
                            id!(true)=>heap.array_push_unchecked(arr, TRUE),
                            id!(false)=>heap.array_push_unchecked(arr, FALSE),
                            id!(null)=>heap.array_push_unchecked(arr, NIL),
                            x=>heap.array_push_unchecked(arr, x.into())
                        }
                    }
                    ScriptToken::String(v)=>{
                        heap.array_push_unchecked(arr, v);
                    }
                    ScriptToken::Number(v)=>{
                        heap.array_push_unchecked(arr, v.into());
                    }
                    ScriptToken::Color(v)=>{
                        heap.array_push_unchecked(arr, ScriptValue::from_color(v));
                    }
                    ScriptToken::OpenCurly=>{ // object
                        let new_obj = heap.new_object();
                        self.state.push(State::Array(arr));
                        self.state.push(State::ObjectKey(new_obj));
                    }
                    ScriptToken::OpenSquare=>{ // 
                        let new_arr = heap.new_array();
                        self.state.push(State::Array(arr));
                        self.state.push(State::Array(new_arr));
                    }
                    ScriptToken::CloseSquare=>{
                        // end of array
                    }
                    ScriptToken::CloseRound | ScriptToken::CloseCurly=>{
                        self.errors.push((self.index,format!("JsonParser: Unexpected }} or ) in array")));
                    }
                    ScriptToken::Operator(op)=>{
                        self.state.push(State::Array(arr));
                        if op != id!(,){
                            self.errors.push((self.index,format!("JsonParser: Unexpected operator expecting ',' {}", op)));
                        }
                    }
                    x=>{
                        self.state.push(State::Array(arr));
                        self.errors.push((self.index,format!("JsonParser: Unexpected token {:?}", x)));
                    }
                }
             }
         }
    }
    
    pub fn parse(&mut self, tokens:&[ScriptTokenPos], heap:&mut ScriptHeap){
        while self.index < tokens.len() as u32 && self.state.len()>0{
            let tok = if let Some(tok) = tokens.get(self.index as usize){
                tok.token.clone()
            }
            else{
                ScriptToken::StreamEnd
            };
            self.parse_step(tok, heap);
            self.index += 1;
        }
    }
}

#[derive(Default)]
pub struct JsonParserThread{
    pub tokenizer:ScriptTokenizer,
    pub parser:JsonParser
}

impl JsonParserThread{
    pub fn read_json(&mut self, json:&str, heap:&mut ScriptHeap)->ScriptValue{
        self.tokenizer.clear();
        self.parser.clear();
        self.tokenizer.tokenize(json, heap);
        self.parser.parse(&self.tokenizer.tokens, heap);
        return self.parser.root
    }
}