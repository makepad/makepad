 
use std::{mem};

#[derive(Clone, Debug)]
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WasmParseError;

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, offset:0 }
    }

    fn skip(&mut self, count: usize) -> Result<(),WasmParseError> {
        if count > self.bytes.len() {
            return Err(WasmParseError);
        }
        self.offset += count;
        self.bytes = &self.bytes[count..];
        Ok(())
    }

    fn read(&mut self, bytes: &mut [u8]) -> Result<(),WasmParseError> {
        if bytes.len() > self.bytes.len() {
            return Err(WasmParseError);
        }
        bytes.copy_from_slice(&self.bytes[..bytes.len()]);
        self.bytes = &self.bytes[bytes.len()..];
        self.offset += bytes.len();
        Ok(())
    }
    
    fn read_u8(&mut self) -> Result<u8,WasmParseError> {
        let mut bytes = [0; mem::size_of::<u8>()];
        self.read(&mut bytes)?;
        Ok(u8::from_le_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32,WasmParseError> {
        let mut bytes = [0; mem::size_of::<u32>()];
        self.read(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }
    
    fn read_var_u32(&mut self) -> Result<u32,WasmParseError>{
        let byte = self.read_u8()? as  u32;
        if byte&0x80 == 0{
            return Ok(byte)
        }
        
        let mut result = byte & 0x7F;
        let mut shift = 7;
        loop {
            let byte = self.read_u8()?;
            result |= ((byte & 0x7F) as u32) << shift;
            if shift >= 25 && (byte >> (32 - shift)) != 0 {
                // The continuation bit or unused bits are set.
                return Err(WasmParseError);
            }
            shift += 7;
            if (byte & 0x80) == 0 {
                break;
            }
        }
        Ok(result)
    }

}

struct WasmSection{
    pub type_id: u8,
    pub start: usize,
    pub end: usize,
    pub name: String
}

fn read_wasm_sections(buf:&[u8])->Result<Vec<WasmSection>,WasmParseError>{
    let mut sections = Vec::new();
    let mut reader = Reader::new(&buf);
    if reader.read_u32()? != 0x6d736100{
        println!("Not a wasm file!");
        return Err(WasmParseError);
    }
    if reader.read_u32()? != 0x1{
        println!("Wrong version");
        return Err(WasmParseError);
    }
    loop{
        let offset = reader.offset;
        if let Ok(type_id) = reader.read_u8(){
            let payload_len = reader.read_var_u32()? as usize;
            let start = reader.offset;
            if type_id == 0{
                let name_len = reader.read_var_u32()? as usize;
                if let Ok(name) = std::str::from_utf8(&reader.bytes[0..name_len]){
                    sections.push(WasmSection{
                        start: offset,
                        type_id: type_id,
                        end: offset + payload_len + (start-offset),
                        name: name.to_string()
                    })
                }
                else{
                    return Err(WasmParseError);
                }
                let end = reader.offset;
                reader.skip(payload_len - (end-start))?;
            }
            else{
                sections.push(WasmSection{
                    start: offset,
                    type_id: type_id,
                    end: offset + payload_len + (start-offset),
                    name: "".to_string()
                });
                reader.skip(payload_len)?;
            }
        } 
        else{
            break;
        }
    }
    return Ok(sections);
}

pub fn wasm_strip_debug(buf: &[u8])->Result<Vec<u8>,WasmParseError>{
    let mut strip = Vec::new();
    strip.extend_from_slice(&[0, 97, 115, 109, 1, 0, 0, 0]);
    let sections = read_wasm_sections(&buf)?;
    // lets rewrite it
    for section in &sections{
        if section.type_id != 0{// !section.name.starts_with(".debug"){
            strip.extend_from_slice(&buf[section.start..section.end]);
        }
        
    }
    Ok(strip)
}
