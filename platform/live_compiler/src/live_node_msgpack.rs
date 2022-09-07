use {
    std::convert::TryInto,
    crate::{
        makepad_live_tokenizer::LiveId,
        live_node::{LivePropType, LiveNode, LiveValue, FittedString, InlineString},
    }
};

pub trait LiveNodeSliceToMsgPack {
    fn to_msgpack(&self, parent_index: usize) -> Result<Vec<u8>, String>;
}

pub trait LiveNodeVecFromMsgPack {
    fn from_msgpack(&mut self, buf: &[u8]) -> Result<(), LiveNodeFromMsgPackError>;
}

const MSGPACK_FIXMAP: u8 = 0x80;
const MSGPACK_FIXARRAY: u8 = 0x90;
const MSGPACK_FIXSTR: u8 = 0xa0;

const MSGPACK_NIL: u8 = 0xc0;

const MSGPACK_FALSE: u8 = 0xc2;
const MSGPACK_TRUE: u8 = 0xc3;

const MSGPACK_BIN8: u8 = 0xc4;
const MSGPACK_BIN16: u8 = 0xc5;
const MSGPACK_BIN32: u8 = 0xc6;

const MSGPACK_EXT8: u8 = 0xc7;
const MSGPACK_EXT16: u8 = 0xc8;
const MSGPACK_EXT32: u8 = 0xc9;

const MSGPACK_F32: u8 = 0xca;
const MSGPACK_F64: u8 = 0xcb;

const MSGPACK_U8: u8 = 0xcc;
const MSGPACK_U16: u8 = 0xcd;
const MSGPACK_U32: u8 = 0xce;
const MSGPACK_U64: u8 = 0xcf;

const MSGPACK_I8: u8 = 0xd0;
const MSGPACK_I16: u8 = 0xd1;
const MSGPACK_I32: u8 = 0xd2;
const MSGPACK_I64: u8 = 0xd3;

const MSGPACK_FIX_EXT1: u8 = 0xd4;
const MSGPACK_FIX_EXT2: u8 = 0xd5;
const MSGPACK_FIX_EXT4: u8 = 0xd6;
const MSGPACK_FIX_EXT8: u8 = 0xd7;
const MSGPACK_FIX_EXT16: u8 = 0xd8;

const MSGPACK_STR8: u8 = 0xd9;
const MSGPACK_STR16: u8 = 0xda;
const MSGPACK_STR32: u8 = 0xdb;

const MSGPACK_ARRAY16: u8 = 0xdc;
const MSGPACK_ARRAY32: u8 = 0xdd;

const MSGPACK_MAP16: u8 = 0xde;
const MSGPACK_MAP32: u8 = 0xdf;

const MSGPACK_FIXNEGINT: u8 = 0xe0;
const MSGPACK_FIXINT: u8 = 0x80;


/* some Rust keyword abuse here 
key:{move:{}} // clone
key:{dyn:{}} // dyn created
key:{ref:{}} // template
key:{in:[v1,v2]} // vec
key:{as:u32} // color
key {enum:"String"} // enum
*/

impl<T> LiveNodeSliceToMsgPack for T where T: AsRef<[LiveNode]> {
    fn to_msgpack(&self, parent_index: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let nodes = self.as_ref();
        let mut index = parent_index;

        struct StackItem {index: usize, count: usize, has_keys: bool}

        let mut stack = vec![StackItem{index:0, count:0, has_keys:false}];
        
        while index < nodes.len() {
            let node = &nodes[index];
            let item = stack.last_mut().unwrap();
            item.count += 1;
            
            if item.has_keys {
                encode_id(node.id, &mut out);
            }
            
            fn encode_u32(v: u32, out: &mut Vec<u8>) {
                if v <= 127 {
                    out.push(v as u8)
                }
                else if v <= std::u8::MAX as u32 {
                    out.push(MSGPACK_U8);
                    out.push(v as u8)
                }
                else if v <= std::u16::MAX as u32 {
                    out.push(MSGPACK_U16);
                    out.extend_from_slice(&(v as u16).to_be_bytes());
                }
                else if v <= std::u32::MAX as u32 {
                    out.push(MSGPACK_U32);
                    out.extend_from_slice(&(v as u32).to_be_bytes());
                }
            }
            
            fn encode_u64(v: u64, out: &mut Vec<u8>) {
                if v <= std::u32::MAX as u64 {
                    encode_u32(v as u32, out);
                }
                else {
                    out.push(MSGPACK_U64);
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
            
            fn encode_i64(v: i64, out: &mut Vec<u8>) {
                if v >= -32 && v < 0 {
                    out.push((-v) as u8 | MSGPACK_FIXNEGINT);
                }
                else if v >= 0 && v <= 127 {
                    out.push(v as u8);
                }
                else if v >= std::i8::MIN as i64 && v <= std::i8::MAX as i64 {
                    out.push(MSGPACK_I8);
                    out.extend_from_slice(&(v as i8).to_be_bytes());
                }
                else if v >= std::i16::MIN as i64 && v <= std::i16::MAX as i64 {
                    out.push(MSGPACK_I16);
                    out.extend_from_slice(&(v as i16).to_be_bytes());
                }
                else if v >= std::i32::MIN as i64 && v <= std::i32::MAX as i64 {
                    out.push(MSGPACK_I32);
                    out.extend_from_slice(&(v as i32).to_be_bytes());
                }
                else {
                    out.push(MSGPACK_I64);
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
            
            fn encode_f32(v: f32, out: &mut Vec<u8>) {
                if v.fract() == 0.0 {
                    encode_i64(v as i64, out)
                }
                else {
                    out.push(MSGPACK_F32);
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
            
            fn encode_f64(v: f64, out: &mut Vec<u8>) {
                if v.fract() == 0.0 {
                    encode_i64(v as i64, out)
                }
                else {
                    out.push(MSGPACK_F64);
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
            fn encode_id(id: LiveId, out: &mut Vec<u8>) {
                if id.0 & 0x8000_0000_0000_0000 == 0 {
                    encode_u64(id.0, out);
                }
                else {
                    id.as_string( | v | {
                        if let Some(v) = v {
                            encode_str(&v, out);
                        }
                        else {
                            encode_u64(id.0, out);
                        }
                    });
                }
            }
            
            let prop_type = node.origin.prop_type();
            if prop_type != LivePropType::Field && prop_type != LivePropType::Nameless {
                return Err("Non field types not implemented".into())
            }
            
            fn encode_str(s: &str, out: &mut Vec<u8>) {
                let len = s.len();
                if len <= 31 {
                    out.push(len as u8 | MSGPACK_FIXSTR);
                    out.extend_from_slice(s.as_bytes());
                }
                else if len < std::u8::MAX as usize {
                    out.push(MSGPACK_STR8);
                    out.push(len as u8);
                    out.extend_from_slice(s.as_bytes());
                }
                else if len < std::u16::MAX as usize {
                    out.push(MSGPACK_STR16);
                    out.extend_from_slice(&(len as u16).to_be_bytes());
                    out.extend_from_slice(s.as_bytes());
                }
                else {
                    out.push(MSGPACK_STR32);
                    out.extend_from_slice(&(len as u32).to_be_bytes());
                    out.extend_from_slice(s.as_bytes());
                }
            }
            
            match &node.value {
                LiveValue::None => {
                    out.push(MSGPACK_NIL);
                },
                LiveValue::Str(s) => {
                    encode_str(s, &mut out);
                },
                LiveValue::InlineString(s) => {
                    encode_str(s.as_str(), &mut out);
                },
                LiveValue::FittedString(s) => {
                    encode_str(s.as_str(), &mut out);
                },
                LiveValue::Bool(v) => {
                    out.push(if *v {MSGPACK_TRUE} else {MSGPACK_FALSE});
                }
                LiveValue::Int64(v) => {
                    encode_i64(*v, &mut out);
                }
                LiveValue::Float32(v) => {
                    encode_f32(*v, &mut out);
                },
                LiveValue::Float64(v) => {
                    encode_f64(*v, &mut out);
                },
                LiveValue::Color(v) => {
                    out.push(1 | MSGPACK_FIXMAP);
                    encode_str("as", &mut out);
                    encode_u32(*v, &mut out);
                },
                LiveValue::Vec2(v) => {
                    out.push(1 | MSGPACK_FIXMAP);
                    encode_str("in", &mut out);
                    out.push(2 | MSGPACK_FIXARRAY);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.x, &mut out);
                },
                LiveValue::Vec3(v) => {
                    out.push(1 | MSGPACK_FIXMAP);
                    encode_str("in", &mut out);
                    out.push(3 | MSGPACK_FIXARRAY);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.z, &mut out);
                },
                LiveValue::Vec4(v) => {
                    out.push(1 | MSGPACK_FIXMAP);
                    encode_str("in", &mut out);
                    out.push(4 | MSGPACK_FIXARRAY);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.z, &mut out);
                    encode_f32(v.w, &mut out);
                },
                LiveValue::BareEnum {variant, ..} => {
                    out.push(1 | MSGPACK_FIXMAP);
                    encode_str("if", &mut out);
                    encode_id(*variant, &mut out);
                },
                LiveValue::Array => {
                    stack.push(StackItem {index: out.len(), count: 0, has_keys: false});
                    out.push(MSGPACK_FIXARRAY);
                },
                LiveValue::TupleEnum {variant, ..} => {
                    out.push(1 | MSGPACK_FIXMAP);
                    encode_str("enum", &mut out);
                    out.push(2 | MSGPACK_FIXARRAY);
                    encode_id(*variant, &mut out);
                    stack.push(StackItem {index: out.len(), count: 0, has_keys: false});
                    out.push(MSGPACK_FIXARRAY);
                },
                LiveValue::NamedEnum {variant, ..} => {
                    out.push(1 | MSGPACK_FIXMAP);
                    encode_str("enum", &mut out);
                    out.push(2 | MSGPACK_FIXARRAY);
                    encode_id(*variant, &mut out);
                    stack.push(StackItem {index: out.len(), count: 0, has_keys: true});
                    out.push(MSGPACK_FIXMAP);
                },
                LiveValue::Object => {
                    out.push(MSGPACK_FIXMAP);
                    stack.push(StackItem {index: out.len(), count: 0, has_keys: true});
                }, // subnodes including this one
                LiveValue::Close => {
                    if stack.len() == 0 {
                        return Err("Unmatched closed".into())
                    }
                    let item = stack.pop().unwrap();
                    if item.count > std::u16::MAX as usize {
                        out[item.index] = if item.has_keys {MSGPACK_MAP32}else {MSGPACK_ARRAY32};
                        let bytes = (item.count as u32).to_be_bytes();
                        out.splice(item.index + 1..item.index + 1, bytes.iter().cloned());
                    }
                    else if item.count >= 16 {
                        out[item.index] = if item.has_keys {MSGPACK_MAP16}else {MSGPACK_ARRAY16};
                        let bytes = (item.count as u16).to_be_bytes();
                        out.splice(item.index + 1..item.index + 1, bytes.iter().cloned());
                    }
                    else {
                        out[item.index] |= item.count as u8
                    }
                },
                // TODO ITEMS
                LiveValue::Id(_) => {
                    return Err("Cannot serialise LiveValue::Id".into())
                },
                LiveValue::Clone(_) => {
                    return Err("Cannot serialise LiveValue::Clone".into())
                }, // subnodes including this one
                LiveValue::ExprBinOp(_) => {
                    return Err("Cannot serialise LiveValue::ExprBinOp".into())
                },
                LiveValue::ExprUnOp(_) => {
                    return Err("Cannot serialise LiveValue::ExprUnOp".into())
                },
                LiveValue::ExprMember(_) => {
                    return Err("Cannot serialise LiveValue::ExprMember".into())
                },
                LiveValue::Expr {..} => {
                    return Err("Cannot serialise LiveValue::Expr".into())
                },
                LiveValue::ExprCall {..} => {
                    return Err("Cannot serialise LiveValue::ExprCall".into())
                },
                LiveValue::DocumentString {..} => {
                    return Err("Cannot serialise LiveValue::DocumentString".into())
                },
                LiveValue::Dependency {..} => {
                    return Err("Cannot serialise LiveValue::Dependency".into())
                },
                LiveValue::Class {..} => {
                    return Err("Cannot serialise LiveValue::Class".into())
                }, // subnodes including this one
                LiveValue::DSL {..} => {
                    return Err("Cannot serialise LiveValue::DSL".into())
                },
                LiveValue::Import(..) => {
                    return Err("Cannot serialise LiveValue::Import".into())
                }
                LiveValue::Registry(..) => {
                    return Err("Cannot serialise LiveValue::Registry".into())
                }
            }
            index += 1;
        }
        if stack.len() > 1 {
            return Err("Uneven stack, not enough closes".into())
        }
        Ok(out)
    }
}

// todo: pack these in somehow
/*
const BIN_EXPR_BIN_OP: u8 = 27;
const BIN_EXPR_UN_OP: u8 = 28;
const BIN_EXPR_MEMBER: u8 = 29;
const BIN_EXPR: u8 = 30;
const BIN_EXPR_CALL: u8 = 31;
*/

// compressed number values

#[derive(Debug)]
pub enum LiveNodeFromMsgPackError {
    OutOfBounds,
    UnexpectedVariant,
    LiveIdCollision,
    ExpectedId,
    UTF8Error
}

impl LiveNodeVecFromMsgPack for Vec<LiveNode> {
    
    fn from_msgpack(&mut self, data: &[u8]) -> Result<(), LiveNodeFromMsgPackError> {
        // alright lets decode msgpack livenodes

        fn assert_len(o: usize, len: usize, data: &[u8]) -> Result<(), LiveNodeFromMsgPackError> {
            if o + len > data.len() {panic!()} //return Err(LiveNodeFromBinaryError::OutOfBounds);}
            Ok(())
        }
        
        fn read_u8(data: &[u8], o: &mut usize) -> Result<u8, LiveNodeFromMsgPackError> {
            assert_len(*o, 1, data) ?;
            let d = data[*o];
            *o += 1;
            Ok(d)
        }
        
        fn read_u16(data: &[u8], o: &mut usize) -> Result<u16, LiveNodeFromMsgPackError> {
            assert_len(*o, 2, data) ?;
            let d = u16::from_be_bytes(data[*o..*o + 2].try_into().unwrap());
            *o += 2;
            Ok(d)
        }
        
        fn read_u32(data: &[u8], o: &mut usize) -> Result<u32, LiveNodeFromMsgPackError> {
            assert_len(*o, 4, data) ?;
            let d = u32::from_be_bytes(data[*o..*o + 4].try_into().unwrap());
            *o += 4;
            Ok(d)
        }
        
        fn read_u64(data: &[u8], o: &mut usize) -> Result<u64, LiveNodeFromMsgPackError> {
            assert_len(*o, 8, data) ?;
            let d = u64::from_be_bytes(data[*o..*o + 8].try_into().unwrap());
            *o += 8;
            Ok(d)
        }

        fn read_i8(data: &[u8], o: &mut usize) -> Result<i8, LiveNodeFromMsgPackError> {
            assert_len(*o, 1, data) ?;
            let d = i8::from_be_bytes(data[*o..*o + 1].try_into().unwrap());
            *o += 1;
            Ok(d)
        }
        
        fn read_i16(data: &[u8], o: &mut usize) -> Result<i16, LiveNodeFromMsgPackError> {
            assert_len(*o, 2, data) ?;
            let d = i16::from_be_bytes(data[*o..*o + 2].try_into().unwrap());
            *o += 2;
            Ok(d)
        }
        
        fn read_i32(data: &[u8], o: &mut usize) -> Result<i32, LiveNodeFromMsgPackError> {
            assert_len(*o, 4, data) ?;
            let d = i32::from_be_bytes(data[*o..*o + 4].try_into().unwrap());
            *o += 4;
            Ok(d)
        }
        
        fn read_i64(data: &[u8], o: &mut usize) -> Result<i64, LiveNodeFromMsgPackError> {
            assert_len(*o, 8, data) ?;
            let d = i64::from_be_bytes(data[*o..*o + 8].try_into().unwrap());
            *o += 8;
            Ok(d)
        }

                
        fn decode_str<'a>(data: &'a [u8], o: &mut usize) -> Result<Option<&'a str>, LiveNodeFromMsgPackError> {
            assert_len(*o, 1, data) ?;
            let len = if data[*o] & MSGPACK_FIXSTR == MSGPACK_FIXSTR {
                (data[*o] & 0xf) as usize
            }
            else {
                match data[*o] {
                    MSGPACK_STR8 => {
                        *o += 1;
                        read_u8(data, o) ? as usize
                    }
                    MSGPACK_STR16 => {
                        *o += 1;
                        read_u16(data, o) ? as usize
                    }
                    MSGPACK_STR32 => {
                        *o += 1;
                        read_u32(data, o) ? as usize
                        
                    }
                    _ => return Ok(None)
                }
            };
            assert_len(*o, len, data) ?;
            if let Ok(val) = std::str::from_utf8(&data[*o..*o + len]) {
                return Ok(Some(val))
            }
            return Err(LiveNodeFromMsgPackError::UTF8Error);
        }

        fn decode_u64(data: &[u8], o: &mut usize) -> Result<Option<u64>, LiveNodeFromMsgPackError> {
            assert_len(*o, 1, data) ?;
            let v = if data[*o] & MSGPACK_FIXINT == 0 {
                Some(data[*o] as u64)
            }
            else {
                match data[*o] {
                    MSGPACK_U8 => {
                        *o += 1;
                        Some(read_u8(data, o) ? as u64)
                    }
                    MSGPACK_U16 => {
                        *o += 1;
                        Some(read_u16(data, o) ? as u64)
                    }
                    MSGPACK_U32 => {
                        *o += 1;
                        Some(read_u32(data, o) ? as u64)
                    }
                    MSGPACK_U64 => {
                        *o += 1;
                        Some(read_u64(data, o) ? as u64)
                    }
                    _ => return Ok(None)
                }
            };
            return Ok(v)
        }

        fn decode_i64(data: &[u8], o: &mut usize) -> Result<Option<i64>, LiveNodeFromMsgPackError> {
            assert_len(*o, 1, data) ?;
            let v = if data[*o] & MSGPACK_FIXINT == 0 {
                Some(data[*o] as i64)
            }
            else if data[*o] & MSGPACK_FIXNEGINT == MSGPACK_FIXNEGINT {
                Some(- ((data[*o] & 0xdf) as i64))
            }
            else{
                match data[*o] {
                    MSGPACK_I8 => {
                        *o += 1;
                        Some(read_i8(data, o) ? as i64)
                    }
                    MSGPACK_I16 => {
                        *o += 1;
                        Some(read_i16(data, o) ? as i64)
                    }
                    MSGPACK_I32 => {
                        *o += 1;
                        Some(read_i32(data, o) ? as i64)
                    }
                    MSGPACK_I64 => {
                        *o += 1;
                        Some(read_i64(data, o) ? as i64)
                    }
                    _ => return Ok(None)
                }
            };
            Ok(v)
        }
        
        fn decode_id(data: &[u8], o: &mut usize) -> Result<Option<LiveId>, LiveNodeFromMsgPackError> {
            // we expect a string OR a u64
            if let Some(val) = decode_str(data, o) ? {
                if let Ok(id) = LiveId::from_str(val){
                    return Ok(Some(id))
                }
                else{
                    return Err(LiveNodeFromMsgPackError::LiveIdCollision)
                }
            }
            else if let Some(v) = decode_u64(data, o) ? {
                return Ok(Some(LiveId(v)))
            }
            Ok(None)
        }
        
        struct StackItem {index: usize, count: usize, has_keys: bool}

        let stack = vec![StackItem{index:0, count:0, has_keys:false}];

        let mut o = 0;
        while o < data.len(){
            
            // ok lets read
            let _id = if stack.last().unwrap().has_keys{
                let id = decode_id(data, &mut o)?;
                if id.is_none(){return Err(LiveNodeFromMsgPackError::ExpectedId)}
                id.unwrap()
            }
            else{
                LiveId(0)
            };
            
            assert_len(o, 1, data)?;
            
            let _value = if let Some(v) = decode_i64(data, &mut o)?{
                LiveValue::Int64(v)
            }
            else if let Some(v) = decode_str(data, &mut o)?{
                if let Some(inline_str) = InlineString::from_str(&v) {
                    LiveValue::InlineString(inline_str)
                }
                else {
                    LiveValue::FittedString(FittedString::from_string(v.to_string()))
                }
            }
            else {
                match data[o]{
                    MSGPACK_NIL=>{
                        LiveValue::None
                    },
                    _=>{
                        LiveValue::None
                    }
                }
                
            };
            break;
            /*
            if let Some(v) = decode_i64(data, &mut o)?{
                self.push(LiveNode{
                    origin:LiveNodeOrigin::default()),
                    value:LiveValue::Int64(v),
                });
            }
            assert_len(o, 1, data);
            */
        }
        
        Ok(())
            
        
        
        /*
         let mut strbuf = String::new();
        
        fn assert_len(o: usize, len: usize, data: &[u8]) -> Result<(), LiveNodeFromMsgPackError> {
            if o + len > data.len() {panic!()}//return Err(LiveNodeFromBinaryError::OutOfBounds);}
            Ok(())
        }
        
        fn decode_id(data: &[u8], o: &mut usize, strbuf: &mut String) -> Result<LiveId, LiveNodeFromMsgPackError> {
            assert_len(*o, 1, data) ?;
            if data[*o] & 128 != 0 {
                assert_len(*o, 8, data) ?;
                let id = LiveId(u64::from_be_bytes(data[*o..*o + 8].try_into().unwrap()));
                *o += 8;
                return Ok(id);
            }
            if data[*o] == 64 {
                *o += 1;
                return Ok(LiveId(0))
            }
            if data[*o] == 65 {
                *o += 1;
                assert_len(*o, 1, data) ?;
                let id = LiveId(data[*o] as u64);
                *o += 1;
                return Ok(id);
            }
            if data[*o] == 66 {
                *o += 1;
                assert_len(*o, 2, data) ?;
                let id = LiveId(u16::from_be_bytes(data[*o..*o + 2].try_into().unwrap()) as u64);
                *o += 2;
                return Ok(id);
            }
            if data[*o] == 68 {
                *o += 1;
                assert_len(*o, 4, data) ?;
                let id = LiveId(u32::from_be_bytes(data[*o..*o + 4].try_into().unwrap()) as u64);
                *o += 4;
                return Ok(id);
            }
            if data[*o] == 72 {
                *o += 1;
                assert_len(*o, 8, data) ?;
                let id = LiveId(u64::from_be_bytes(data[*o..*o + 8].try_into().unwrap()));
                *o += 8;
                return Ok(id);
            }
            strbuf.clear();
            loop {
                assert_len(*o, 1, data) ?;
                let d = data[*o];
                let c = d & 63;
                if c<10 {strbuf.push(('0' as u8 + c) as char)}
                else if c >= 10 && c<36 {strbuf.push(('a' as u8 + (c - 10)) as char)}
                else if c >= 36 && c<63 {strbuf.push(('A' as u8 + (c - 36)) as char)}
                else {strbuf.push('_')}
                *o += 1;
                if d & 64 == 0 {break}
            }
            return Ok(LiveId::from_str_unchecked(strbuf));
        }
        
        let mut o = 0;
        while o < data.len() {
            let id = decode_id(data, &mut o, &mut strbuf) ?;
            assert_len(o, 1, data)?;
            
            let prop_type = data[o] >> 5;
            let variant_id = data[o] & 0x1f;
            o += 1;
            
            let value = match variant_id {
                BIN_NONE => {LiveValue::None},
                BIN_STRING_8 | BIN_STRING_16 | BIN_STRING_32 => {
                    let len = if variant_id == BIN_STRING_8 {
                        assert_len(o, 1, data) ?;
                        let len = data[o] as usize;
                        o += 1;
                        len
                    }
                    else if variant_id == BIN_STRING_16 {
                        assert_len(o, 2, data) ?;
                        let len = u16::from_be_bytes(data[o..o + 2].try_into().unwrap()) as usize;
                        o += 2;
                        len
                    }
                    else{
                        assert_len(o, 4, data) ?;
                        let len = u32::from_be_bytes(data[o..o + 2].try_into().unwrap()) as usize;
                        o += 4;
                        len
                    };
                    if let Ok(val) = std::str::from_utf8(&data[o..o + len]) {
                        o += len;
                        if let Some(inline_str) = InlineString::from_str(val) {
                            LiveValue::InlineString(inline_str)
                        }
                        else {
                            LiveValue::FittedString(FittedString::from_string(val.to_string()))
                        }
                    }
                    else {
                        return Err(LiveNodeFromBinaryError::UTF8Error);
                    }
                },
                BIN_TRUE => {LiveValue::Bool(true)},
                BIN_FALSE => {LiveValue::Bool(false)},
                BIN_INT0 => {
                    LiveValue::Int64(0)
                },
                BIN_INT8 => {
                    assert_len(o, 1, data)?;
                    let b = i8::from_be_bytes(data[o..o + 1].try_into().unwrap()) as i64;
                    o += 1;
                    LiveValue::Int64(b)
                },
                BIN_INT16 => {
                    assert_len(o, 2, data)?;
                    let v = i16::from_be_bytes(data[o..o + 2].try_into().unwrap()) as i64;
                    o += 2;
                    LiveValue::Int64(v)
                },
                BIN_INT32 => {
                    assert_len(o, 4, data)?;
                    let v = i32::from_be_bytes(data[o..o + 4].try_into().unwrap()) as i64;
                    o += 4;
                    LiveValue::Int64(v)
                },
                BIN_INT64 => {
                    assert_len(o, 8, data)?;
                    let v = i64::from_be_bytes(data[o..o + 8].try_into().unwrap());
                    o += 8;
                    LiveValue::Int64(v)
                },
                BIN_FLOAT32 => {
                    assert_len(o, 4, data)?;
                    let v = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    LiveValue::Float32(v)
                },
                BIN_FLOAT64 => {
                    assert_len(o, 8, data)?;
                    let v = f64::from_be_bytes(data[o..o + 8].try_into().unwrap());
                    o += 8;
                    LiveValue::Float64(v)
                },
                BIN_FLOAT64_8 => {
                    assert_len(o, 1, data)?;
                    let v = (i8::from_be_bytes(data[o..o + 1].try_into().unwrap()) as f64) / 40.0;
                    o += 1;
                    LiveValue::Float64(v)
                },
                BIN_COLOR => {
                    assert_len(o, 4, data)?;
                    let u = u32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    LiveValue::Color(u)
                },
                BIN_VEC2 => {
                    assert_len(o, 8, data)?;
                    let x = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    let y = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    LiveValue::Vec2(Vec2 {x, y})
                },
                BIN_VEC3 => {
                    assert_len(o, 12, data)?;
                    let x = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    let y = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    let z = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    LiveValue::Vec3(Vec3 {x, y, z})
                },
                BIN_VEC4 => {
                    assert_len(o, 16, data)?;
                    let x = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    let y = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    let z = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    let w = f32::from_be_bytes(data[o..o + 4].try_into().unwrap());
                    o += 4;
                    LiveValue::Vec4(Vec4 {x, y, z, w})
                },
                BIN_ID => {
                    LiveValue::Id(decode_id(data, &mut o, &mut strbuf) ?)
                },
                BIN_BARE_ENUM => {
                    let base = decode_id(data, &mut o, &mut strbuf) ?;
                    let variant = decode_id(data, &mut o, &mut strbuf) ?;
                    LiveValue::BareEnum {base, variant}
                },
                BIN_ARRAY => {
                    LiveValue::Array
                },
                BIN_TUPLE_ENUM => {
                    let base = decode_id(data, &mut o, &mut strbuf) ?;
                    let variant = decode_id(data, &mut o, &mut strbuf) ?;
                    LiveValue::TupleEnum {base, variant}
                },
                BIN_NAMED_ENUM => {
                    let base = decode_id(data, &mut o, &mut strbuf) ?;
                    let variant = decode_id(data, &mut o, &mut strbuf) ?;
                    LiveValue::NamedEnum {base, variant}
                },
                BIN_OBJECT => {
                    LiveValue::Object
                },
                BIN_CLONE => {
                    LiveValue::Clone(decode_id(data, &mut o, &mut strbuf) ?)
                },
                BIN_CLOSE => {
                    LiveValue::Close
                },
                _ => {
                    return Err(LiveNodeFromBinaryError::UnexpectedVariant);
                }
            };
            self.push(LiveNode {
                origin: LiveNodeOrigin::empty()
                    .with_prop_type(LivePropType::from_usize(prop_type as usize)),
                id,
                value
            });
        }
        Ok(())
        */
    }
}
