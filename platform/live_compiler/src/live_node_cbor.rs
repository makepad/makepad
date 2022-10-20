use {
    std::convert::TryInto,
    crate::{
        makepad_live_tokenizer::LiveId,
        live_node::*,
    }
};

pub trait LiveNodeSliceToCbor {
    fn to_cbor(&self, parent_index: usize) -> Result<Vec<u8>, String>;
}

pub trait LiveNodeVecFromCbor {
    fn from_cbor(&mut self, buf: &[u8]) -> Result<(), LiveNodeFromCborError>;
}

//0x00..0x17	unsigned integer 0x00..0x17 (0..23)
const CBOR_UINT_START: u8 = 0x00;
const CBOR_UINT_END: u8 = 0x17;
const CBOR_U8: u8 = 0x18; //	unsigned integer (one-byte uint8_t follows)
const CBOR_U16: u8 = 0x19; // unsigned integer (two - byte uint16_t follows)
const CBOR_U32: u8 = 0x1a; //	unsigned integer (four-byte uint32_t follows)
const CBOR_U64: u8 = 0x1b; //	unsigned integer (eight-byte uint64_t follows)
const CBOR_NUINT_START: u8 = 0x20; //..0x37	negative integer -1-0x00..-1-0x17 (-1..-24)
const CBOR_NUINT_END: u8 = 0x37; //..0x37	negative integer -1-0x00..-1-0x17 (-1..-24)
const CBOR_NU8: u8 = 0x38; //	negative integer -1-n (one-byte uint8_t for n follows)
const CBOR_NU16: u8 = 0x39; //	negative integer -1-n (two-byte uint16_t for n follows)
const CBOR_NU32: u8 = 0x3a; //	negative integer -1-n (four-byte uint32_t for n follows)
const CBOR_NU64: u8 = 0x3b; //	negative integer -1-n (eight-byte uint64_t for n follows)
const CBOR_BSTR_START: u8 = 0x40; //..0x57	byte string (0x00..0x17 bytes follow)
const CBOR_BSTR_END: u8 = 0x57; //..0x57	byte string (0x00..0x17 bytes follow)
const CBOR_BSTR_8: u8 = 0x58; //	byte string (one-byte uint8_t for n, and then n bytes follow)
const CBOR_BSTR_16: u8 = 0x59; //	byte string (two-byte uint16_t for n, and then n bytes follow)
const CBOR_BSTR_32: u8 = 0x5a; //	byte string (four-byte uint32_t for n, and then n bytes follow)
const CBOR_BSTR_64: u8 = 0x5b; //	byte string (eight-byte uint64_t for n, and then n bytes follow)
const CBOR_BSTR_BRK: u8 = 0x5f; //	byte string, byte strings follow, terminated by "break"
const CBOR_UTF8_START: u8 = 0x60; //..0x77	UTF-8 string (0x00..0x17 bytes follow)
const CBOR_UTF8_END: u8 = 0x77; //..0x77	UTF-8 string (0x00..0x17 bytes follow)
const CBOR_UTF8_8: u8 = 0x78; //	UTF-8 string (one-byte uint8_t for n, and then n bytes follow)
const CBOR_UTF8_16: u8 = 0x79; //	UTF-8 string (two-byte uint16_t for n, and then n bytes follow)
const CBOR_UTF8_32: u8 = 0x7a; //	UTF-8 string (four-byte uint32_t for n, and then n bytes follow)
const CBOR_UTF8_64: u8 = 0x7b; //	UTF-8 string (eight-byte uint64_t for n, and then n bytes follow)
const CBOR_UTF8_BRK: u8 = 0x7f; //	UTF-8 string, UTF-8 strings follow, terminated by "break"
const CBOR_ARRAY_START: u8 = 0x80; //..0x97	array (0x00..0x17 data items follow)
const CBOR_ARRAY_END: u8 = 0x97; //..0x97	array (0x00..0x17 data items follow)
const CBOR_ARRAY_8: u8 = 0x98; //	array (one-byte uint8_t for n, and then n data items follow)
const CBOR_ARRAY_16: u8 = 0x99; //	array (two-byte uint16_t for n, and then n data items follow)
const CBOR_ARRAY_32: u8 = 0x9a; //	array (four-byte uint32_t for n, and then n data items follow)
const CBOR_ARRAY_64: u8 = 0x9b; //	array (eight-byte uint64_t for n, and then n data items follow)
const CBOR_ARRAY_BRK: u8 = 0x9f; //	array, data items follow, terminated by "break"
const CBOR_MAP_START: u8 = 0xa0; //..0xb7	map (0x00..0x17 pairs of data items follow)
const CBOR_MAP_END: u8 = 0xb7; //..0xb7	map (0x00..0x17 pairs of data items follow)
const CBOR_MAP_8: u8 = 0xb8; //	map (one-byte uint8_t for n, and then n pairs of data items follow)
const CBOR_MAP_16: u8 = 0xb9; //	map (two-byte uint16_t for n, and then n pairs of data items follow)
const CBOR_MAP_32: u8 = 0xba; //	map (four-byte uint32_t for n, and then n pairs of data items follow)
const CBOR_MAP_64: u8 = 0xbb; //	map (eight-byte uint64_t for n, and then n pairs of data items follow)
const CBOR_MAP_BRK: u8 = 0xbf; //	map, pairs of data items follow, terminated by "break"
const CBOR_TIME_TEXT: u8 = 0xc0; //	text-based date/time (data item follows; see Section 3.4.1)
const CBOR_TIME_EPOCH: u8 = 0xc1; //	epoch-based date/time (data item follows; see Section 3.4.2)
const CBOR_UBIGNUM: u8 = 0xc2; //	unsigned bignum (data item "byte string" follows)
const CBOR_NBIGNUM: u8 = 0xc3; //	negative bignum (data item "byte string" follows)
const CBOR_DECFRACT: u8 = 0xc4; //	decimal Fraction (data item "array" follows; see Section 3.4.4)
const CBOR_BIGFLOAT: u8 = 0xc5; //	bigfloat (data item "array" follows; see Section 3.4.4)
const CBOR_TAG_START: u8 = 0xc6; //..0xd4	(tag)
const CBOR_TAG_END: u8 = 0xd4; //..0xd4	(tag)
const CBOR_CONV_START: u8 = 0xd5; //..0xd7	expected conversion (data item follows; see Section 3.4.5.2)
const CBOR_CONV_END: u8 = 0xd7; //..0xd7	expected conversion (data item follows; see Section 3.4.5.2)
const CBOR_MTAG_START: u8 = 0xd8; //..0xdb	(more tags; 1/2/4/8 bytes of tag number and then a data item follow)
const CBOR_MTAG_END: u8 = 0xdb; //..0xdb	(more tags; 1/2/4/8 bytes of tag number and then a data item follow)
const CBOR_SIMPLE_START: u8 = 0xe0; //..0xf3	(simple value)
const CBOR_SIMPLE_END: u8 = 0xf3; //..0xf3	(simple value)
const CBOR_FALSE: u8 = 0xf4; //	false
const CBOR_TRUE: u8 = 0xf5; //	true
const CBOR_NULL: u8 = 0xf6; //	null
const CBOR_UNDEIFNED: u8 = 0xf7; //	undefined
const CBOR_SIMPLE_8: u8 = 0xf8; //	(simple value, one byte follows)
const CBOR_FLOAT16: u8 = 0xf9; //	half-precision float (two-byte IEEE 754)
const CBOR_FLOAT32: u8 = 0xfa; //	single-precision float (four-byte IEEE 754)
const CBOR_FLOAT64: u8 = 0xfb; //	double-precision float (eight-byte IEEE 754)
const CBOR_BREAK: u8 = 0xff; //	"break" stop code

/* some Rust keyword abuse here 
key:{move:{}} // clone
key:{dyn:{}} // dyn created
key:{ref:{}} // template
key:{in:[v1,v2]} // vec
key:{as:u32} // color
key {enum:"String"} // enum
*/

impl<T> LiveNodeSliceToCbor for T where T: AsRef<[LiveNode]> {
    fn to_cbor(&self, parent_index: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let nodes = self.as_ref();
        let mut index = parent_index;
        
        struct StackItem {index: usize, count: usize, has_keys: bool}
        
        let mut stack = vec![StackItem {index: 0, count: 0, has_keys: false}];
        
        while index < nodes.len() {
            let node = &nodes[index];
            
            if node.value.is_close() {
                if stack.len() == 0 {
                    return Err("Unmatched closed".into())
                }
                let item = stack.pop().unwrap();
                if item.count > std::u16::MAX as usize {
                    out[item.index] = if item.has_keys {CBOR_MAP_32}else {CBOR_ARRAY_32};
                    let bytes = (item.count as u32).to_be_bytes();
                    out.splice(item.index + 1..item.index + 1, bytes.iter().cloned());
                }
                else if item.count > std::u8::MAX as usize {
                    out[item.index] = if item.has_keys {CBOR_MAP_16}else {CBOR_ARRAY_16};
                    let bytes = (item.count as u16).to_be_bytes();
                    out.splice(item.index + 1..item.index + 1, bytes.iter().cloned());
                }
                else if item.count >= 32 {
                    out[item.index] = if item.has_keys {CBOR_MAP_8}else {CBOR_ARRAY_8};
                    let bytes = (item.count as u8).to_be_bytes();
                    out.splice(item.index + 1..item.index + 1, bytes.iter().cloned());
                }
                else {
                    out[item.index] += item.count as u8
                }
                index += 1;
                continue;
            }
            
            let item = stack.last_mut().unwrap();
            item.count += 1;
            
            if item.has_keys {
                encode_id(node.id, &mut out);
            }
            
            fn encode_u32(v: u32, out: &mut Vec<u8>) {
                if v <= CBOR_UINT_END as u32 {
                    out.push(v as u8)
                }
                else if v <= std::u8::MAX as u32 {
                    out.push(CBOR_U8);
                    out.push(v as u8)
                }
                else if v <= std::u16::MAX as u32 {
                    out.push(CBOR_U16);
                    out.extend_from_slice(&(v as u16).to_be_bytes());
                }
                else if v <= std::u32::MAX as u32 {
                    out.push(CBOR_U32);
                    out.extend_from_slice(&(v as u32).to_be_bytes());
                }
            }
            
            fn encode_u64(v: u64, out: &mut Vec<u8>) {
                if v <= std::u32::MAX as u64 {
                    encode_u32(v as u32, out);
                }
                else {
                    out.push(CBOR_U64);
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
            
            fn encode_i64(v: i64, out: &mut Vec<u8>) {
                if v < 0 {
                    let v = ((-v) - 1) as u64;
                    if v <= (CBOR_NUINT_END - CBOR_NUINT_START) as u64 {
                        out.push(CBOR_NUINT_START + v as u8);
                    }
                    else if v <= std::u8::MAX as u64 {
                        out.push(CBOR_NU8);
                        out.extend_from_slice(&(v as u8).to_be_bytes());
                    }
                    else if v <= std::u16::MAX as u64 {
                        out.push(CBOR_NU16);
                        out.extend_from_slice(&(v as u16).to_be_bytes());
                    }
                    else if v <= std::u32::MAX as u64 {
                        out.push(CBOR_NU32);
                        out.extend_from_slice(&(v as u32).to_be_bytes());
                    }
                    else {
                        out.push(CBOR_NU64);
                        out.extend_from_slice(&v.to_be_bytes());
                    }
                }
                else {
                    encode_u64(v as u64, out);
                }
            }
            
            fn encode_f32(v: f32, out: &mut Vec<u8>) {
                if v.fract() == 0.0 {
                    encode_i64(v as i64, out)
                }
                else {
                    out.push(CBOR_FLOAT32);
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
            
            fn encode_f64(v: f64, out: &mut Vec<u8>) {
                if v.fract() == 0.0 {
                    encode_i64(v as i64, out)
                }
                else {
                    out.push(CBOR_FLOAT64);
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
                if len <= (CBOR_UTF8_END - CBOR_UTF8_START) as usize {
                    out.push(len as u8 + CBOR_UTF8_START);
                    out.extend_from_slice(s.as_bytes());
                }
                else if len <= std::u8::MAX as usize {
                    out.push(CBOR_UTF8_8);
                    out.push(len as u8);
                    out.extend_from_slice(s.as_bytes());
                }
                else if len <= std::u16::MAX as usize {
                    out.push(CBOR_UTF8_16);
                    out.extend_from_slice(&(len as u16).to_be_bytes());
                    out.extend_from_slice(s.as_bytes());
                }
                else if len <= std::u32::MAX as usize {
                    out.push(CBOR_UTF8_32);
                    out.extend_from_slice(&(len as u32).to_be_bytes());
                    out.extend_from_slice(s.as_bytes());
                }
                else {
                    out.push(CBOR_UTF8_64);
                    out.extend_from_slice(&(len as u64).to_be_bytes());
                    out.extend_from_slice(s.as_bytes());
                }
            }
            //log!("SAVING {:?} {}", node.value, out.len());
            match &node.value {
                LiveValue::None => {
                    out.push(CBOR_NULL);
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
                    out.push(if *v {CBOR_TRUE} else {CBOR_FALSE});
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
                    out.push(1 + CBOR_MAP_START);
                    encode_str("as", &mut out);
                    encode_u32(*v, &mut out);
                },
                LiveValue::Vec2(v) => {
                    out.push(1 + CBOR_MAP_START);
                    encode_str("in", &mut out);
                    out.push(2 + CBOR_ARRAY_START);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.x, &mut out);
                },
                LiveValue::Vec3(v) => {
                    out.push(1 + CBOR_MAP_START);
                    encode_str("in", &mut out);
                    out.push(3 + CBOR_ARRAY_START);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.z, &mut out);
                },
                LiveValue::Vec4(v) => {
                    out.push(1 + CBOR_MAP_START);
                    encode_str("in", &mut out);
                    out.push(4 + CBOR_ARRAY_START);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.x, &mut out);
                    encode_f32(v.z, &mut out);
                    encode_f32(v.w, &mut out);
                },
                LiveValue::BareEnum(variant) => {
                    out.push(1 + CBOR_MAP_START);
                    encode_str("if", &mut out);
                    encode_id(*variant, &mut out);
                },
                LiveValue::Array => {
                    stack.push(StackItem {index: out.len(), count: 0, has_keys: false});
                    out.push(CBOR_ARRAY_START);
                },
                LiveValue::TupleEnum(variant) => {
                    out.push(1 + CBOR_MAP_START);
                    encode_str("enum", &mut out);
                    out.push(2 + CBOR_ARRAY_START);
                    encode_id(*variant, &mut out);
                    stack.push(StackItem {index: out.len(), count: 0, has_keys: false});
                    out.push(CBOR_ARRAY_START);
                },
                LiveValue::NamedEnum(variant) => {
                    out.push(1 + CBOR_MAP_START);
                    encode_str("enum", &mut out);
                    out.push(2 + CBOR_ARRAY_START);
                    encode_id(*variant, &mut out);
                    stack.push(StackItem {index: out.len(), count: 0, has_keys: true});
                    out.push(CBOR_MAP_START);
                },
                LiveValue::Object => {
                    stack.push(StackItem {index: out.len(), count: 0, has_keys: true});
                    out.push(CBOR_MAP_START);
                }, // subnodes including this one
                LiveValue::Close => {},
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
pub enum LiveNodeFromCborError {
    OutOfBounds,
    UnexpectedVariant,
    LiveIdCollision,
    ExpectedId,
    NotImpl,
    UnexpectedValue,
    ExpectedBareEnumString,
    StackNotClosed,
    UTF8Error
}

impl LiveNodeVecFromCbor for Vec<LiveNode> {
    
    fn from_cbor(&mut self, data: &[u8]) -> Result<(), LiveNodeFromCborError> {
        // alright lets decode msgpack livenodes
        
        fn assert_len(o: usize, len: usize, data: &[u8]) -> Result<(), LiveNodeFromCborError> {
            if o + len > data.len() {panic!()} //return Err(LiveNodeFromBinaryError::OutOfBounds);}
            Ok(())
        }
        
        fn read_u8(data: &[u8], o: &mut usize) -> Result<u8, LiveNodeFromCborError> {
            assert_len(*o, 1, data) ?;
            let d = data[*o];
            *o += 1;
            Ok(d)
        }
        
        fn read_u16(data: &[u8], o: &mut usize) -> Result<u16, LiveNodeFromCborError> {
            assert_len(*o, 2, data) ?;
            let d = u16::from_be_bytes(data[*o..*o + 2].try_into().unwrap());
            *o += 2;
            Ok(d)
        }
        
        fn read_u32(data: &[u8], o: &mut usize) -> Result<u32, LiveNodeFromCborError> {
            assert_len(*o, 4, data) ?;
            let d = u32::from_be_bytes(data[*o..*o + 4].try_into().unwrap());
            *o += 4;
            Ok(d)
        }
        
        fn read_u64(data: &[u8], o: &mut usize) -> Result<u64, LiveNodeFromCborError> {
            assert_len(*o, 8, data) ?;
            let d = u64::from_be_bytes(data[*o..*o + 8].try_into().unwrap());
            *o += 8;
            Ok(d)
        }
        
        fn read_i8(data: &[u8], o: &mut usize) -> Result<i8, LiveNodeFromCborError> {
            assert_len(*o, 1, data) ?;
            let d = i8::from_be_bytes(data[*o..*o + 1].try_into().unwrap());
            *o += 1;
            Ok(d)
        }
        
        fn read_i16(data: &[u8], o: &mut usize) -> Result<i16, LiveNodeFromCborError> {
            assert_len(*o, 2, data) ?;
            let d = i16::from_be_bytes(data[*o..*o + 2].try_into().unwrap());
            *o += 2;
            Ok(d)
        }
        
        fn read_i32(data: &[u8], o: &mut usize) -> Result<i32, LiveNodeFromCborError> {
            assert_len(*o, 4, data) ?;
            let d = i32::from_be_bytes(data[*o..*o + 4].try_into().unwrap());
            *o += 4;
            Ok(d)
        }
        
        fn read_i64(data: &[u8], o: &mut usize) -> Result<i64, LiveNodeFromCborError> {
            assert_len(*o, 8, data) ?;
            let d = i64::from_be_bytes(data[*o..*o + 8].try_into().unwrap());
            *o += 8;
            Ok(d)
        }
        
        fn read_f32(data: &[u8], o: &mut usize) -> Result<f32, LiveNodeFromCborError> {
            assert_len(*o, 4, data) ?;
            let d = f32::from_be_bytes(data[*o..*o + 4].try_into().unwrap());
            *o += 4;
            Ok(d)
        }
        
        fn read_f64(data: &[u8], o: &mut usize) -> Result<f64, LiveNodeFromCborError> {
            assert_len(*o, 8, data) ?;
            let d = f64::from_be_bytes(data[*o..*o + 8].try_into().unwrap());
            *o += 8;
            Ok(d)
        }
        
        fn decode_str<'a>(data: &'a [u8], o: &mut usize) -> Result<Option<&'a str>,
        LiveNodeFromCborError> {
            assert_len(*o, 1, data) ?;
            let len = if data[*o] >= CBOR_UTF8_START && data[*o] <= CBOR_UTF8_END {
                let r = (data[*o] - CBOR_UTF8_START) as usize;
                *o += 1;
                r
            }
            else {
                match data[*o] {
                    CBOR_UTF8_8 => {
                        *o += 1;
                        read_u8(data, o) ? as usize
                    }
                    CBOR_UTF8_16 => {
                        *o += 1;
                        read_u16(data, o) ? as usize
                    }
                    CBOR_UTF8_32 => {
                        *o += 1;
                        read_u32(data, o) ? as usize
                    }
                    CBOR_UTF8_64 => {
                        *o += 1;
                        read_u64(data, o) ? as usize
                    }
                    _ => return Ok(None)
                }
            };
            assert_len(*o, len, data) ?;
            if let Ok(val) = std::str::from_utf8(&data[*o..*o + len]) {
                *o += len;
                return Ok(Some(val))
            }
            return Err(LiveNodeFromCborError::UTF8Error);
        }
        
        fn decode_u64(data: &[u8], o: &mut usize) -> Result<Option<u64>, LiveNodeFromCborError> {
            assert_len(*o, 1, data) ?;
            let v = if data[*o] <= CBOR_UINT_END {
                let r = Some(data[*o] as u64);
                *o += 1;
                r
            }
            else {
                match data[*o] {
                    CBOR_U8 => {
                        *o += 1;
                        Some(read_u8(data, o) ? as u64)
                    }
                    CBOR_U16 => {
                        *o += 1;
                        Some(read_u16(data, o) ? as u64)
                    }
                    CBOR_U32 => {
                        *o += 1;
                        Some(read_u32(data, o) ? as u64)
                    }
                    CBOR_U64 => {
                        *o += 1;
                        Some(read_u64(data, o) ? as u64)
                    }
                    _ => return Ok(None)
                }
            };
            return Ok(v)
        }
        
        fn decode_i64(data: &[u8], o: &mut usize) -> Result<Option<i64>, LiveNodeFromCborError> {
            assert_len(*o, 1, data) ?;
            let v = if data[*o] >= CBOR_NUINT_START && data[*o] <= CBOR_NUINT_END {
                let r = Some(-((data[*o] - CBOR_NUINT_START + 1) as i64));
                *o += 1;
                r
            }
            else {
                match data[*o] {
                    CBOR_NU8 => {
                        *o += 1;
                        Some(-(read_i8(data, o) ? as i64 + 1))
                    }
                    CBOR_NU16 => {
                        *o += 1;
                        Some(-(read_i16(data, o) ? as i64 + 1))
                    }
                    CBOR_NU32 => {
                        *o += 1;
                        Some(-(read_i32(data, o) ? as i64 + 1))
                    }
                    CBOR_NU64 => {
                        *o += 1;
                        Some(-(read_i64(data, o) ? as i64 + 1))
                    }
                    _ => if let Some(data) = decode_u64(data, o) ? {
                        Some(data as i64)
                    }
                    else {
                        None
                    }
                }
            };
            Ok(v)
        }
        
        fn decode_array_len(data: &[u8], o: &mut usize) -> Result<Option<usize>, LiveNodeFromCborError> {
            assert_len(*o, 1, data) ?;
            let v = if data[*o] >= CBOR_ARRAY_START && data[*o] <= CBOR_ARRAY_END {
                let r = Some((data[*o] - CBOR_ARRAY_START) as usize);
                *o += 1;
                r
            }
            else {
                match data[*o] {
                    CBOR_ARRAY_8 => {
                        *o += 1;
                        Some(read_u8(data, o) ? as usize)
                    }
                    CBOR_ARRAY_16 => {
                        *o += 1;
                        Some(read_u16(data, o) ? as usize)
                    }
                    CBOR_ARRAY_32 => {
                        *o += 1;
                        Some(read_u32(data, o) ? as usize)
                    }
                    CBOR_ARRAY_64 => {
                        *o += 1;
                        Some(read_u64(data, o) ? as usize)
                    }
                    _ => return Ok(None)
                }
            };
            Ok(v)
        }
        
        fn decode_map_len(data: &[u8], o: &mut usize) -> Result<Option<usize>, LiveNodeFromCborError> {
            assert_len(*o, 1, data) ?;
            let v = if data[*o] >= CBOR_MAP_START && data[*o] <= CBOR_MAP_END {
                let r = Some((data[*o] - CBOR_MAP_START) as usize);
                *o += 1;
                r
            }
            else {
                match data[*o] {
                    CBOR_MAP_8 => {
                        *o += 1;
                        Some(read_u8(data, o) ? as usize)
                    }
                    CBOR_MAP_16 => {
                        *o += 1;
                        Some(read_u16(data, o) ? as usize)
                    }
                    CBOR_MAP_32 => {
                        *o += 1;
                        Some(read_u32(data, o) ? as usize)
                    }
                    CBOR_MAP_64 => {
                        *o += 1;
                        Some(read_u32(data, o) ? as usize)
                    }
                    _ => return Ok(None)
                }
            };
            Ok(v)
        }
        
        fn decode_id(data: &[u8], o: &mut usize) -> Result<Option<LiveId>, LiveNodeFromCborError> {
            // we expect a string OR a u64
            if let Some(val) = decode_str(data, o) ? {
                if let Ok(id) = LiveId::from_str(val) {
                    return Ok(Some(id))
                }
                else {
                    return Err(LiveNodeFromCborError::LiveIdCollision)
                }
            }
            else if let Some(v) = decode_u64(data, o) ? {
                return Ok(Some(LiveId(v)))
            }
            Ok(None)
        }
        
        struct StackItem {len: usize, count: usize, has_keys: bool}
        
        let mut stack = vec![StackItem {count: 0, len: 1, has_keys: false}];
        let origin = LiveNodeOrigin::field();
        let mut o = 0;
        while o < data.len() {
            
            if stack.last().unwrap().count == stack.last().unwrap().len {
                self.push(LiveNode {id: LiveId(0), origin, value: LiveValue::Close});
                stack.pop();
            }
            
            // ok lets read
            let stack_item = stack.last_mut().unwrap();
            let id = if stack_item.has_keys {
                let id = decode_id(data, &mut o) ?;
                if id.is_none() {return Err(LiveNodeFromCborError::ExpectedId)}
                id.unwrap()
            }
            else {
                LiveId(0)
            };
            
            stack_item.count += 1;
            
            assert_len(o, 1, data) ?;
            
            if let Some(v) = decode_i64(data, &mut o) ? {
                self.push(LiveNode {id, origin, value: LiveValue::Int64(v)});
            }
            else if let Some(v) = decode_str(data, &mut o) ? {
                let value = if let Some(inline_str) = InlineString::from_str(&v) {
                    LiveValue::InlineString(inline_str)
                }
                else {
                    LiveValue::FittedString(FittedString::from_string(v.to_string()))
                };
                self.push(LiveNode {id, origin, value});
            }
            else if let Some(len) = decode_array_len(data, &mut o) ? {
                stack.push(StackItem {count: 0, len, has_keys: false});
                self.push(LiveNode {id, origin, value: LiveValue::Array});
            }
            else if let Some(len) = decode_map_len(data, &mut o) ? {
                // this COULD be a special type.
                if len == 1 {
                    let mut o1 = o;
                    if let Some(s) = decode_str(data, &mut o1) ? {
                        match s {
                            "in" => { // its a vec
                                return Err(LiveNodeFromCborError::NotImpl)
                            }
                            "as" => { // its a color
                                return Err(LiveNodeFromCborError::NotImpl)
                            }
                            "if" => { // bare enum
                                if let Some(variant) = decode_id(data, &mut o1) ? {
                                    self.push(LiveNode {
                                        id,
                                        origin,
                                        value: LiveValue::BareEnum(variant)
                                    });
                                    o = o1;
                                    continue;
                                }
                                else {
                                    return Err(LiveNodeFromCborError::ExpectedBareEnumString)
                                }
                            }
                            "enum" => { // other enum
                                return Err(LiveNodeFromCborError::NotImpl)
                            }
                            _ => ()
                        }
                    }
                }
                stack.push(StackItem {count: 0, len, has_keys: true});
                self.push(LiveNode {id, origin, value: LiveValue::Object});
            }
            else {
                match data[o] {
                    CBOR_TRUE => {
                        o += 1;
                        self.push(LiveNode {id, origin, value: LiveValue::Bool(true)});
                    }
                    CBOR_FALSE => {
                        o += 1;
                        self.push(LiveNode {id, origin, value: LiveValue::Bool(false)});
                    }
                    CBOR_FLOAT32 => {
                        o += 1;
                        let value = LiveValue::Float32(read_f32(data, &mut o) ?);
                        self.push(LiveNode {id, origin, value});
                    }
                    CBOR_FLOAT64 => {
                        o += 1;
                        let value = LiveValue::Float64(read_f64(data, &mut o) ?);
                        self.push(LiveNode {id, origin, value});
                    }
                    CBOR_NULL => {
                        o += 1;
                        self.push(LiveNode {id, origin, value: LiveValue::None});
                    },
                    _ => {
                        return Err(LiveNodeFromCborError::UnexpectedValue)
                    }
                }
            };
        }
        // lets unwind the stack
        while let Some(item) = stack.pop() {
            if item.count != item.len {
                return Err(LiveNodeFromCborError::StackNotClosed)
            }
        }
        self.push(LiveNode {id: LiveId(0), origin, value: LiveValue::Close});
        Ok(())
    }
}
