
use makepad_math::Vec4;
use crate::colorhex;

pub trait Vec4Ext{
    fn from_hex_str(hex: &str) -> Result<Vec4, ()> {Self::from_hex_bytes(hex.as_bytes())}
    fn from_hex_bytes(bytes: &[u8]) -> Result<Vec4, ()>;
    fn append_hex_to_string(&self, out:&mut String);
    fn color(value: &str) -> Vec4;
}

impl Vec4Ext for Vec4{
    fn append_hex_to_string(&self, out:&mut String) {
        fn int_to_hex(d: u8) -> char {
            if d >= 10 {
                return (d + 55) as char;
            }
            return (d + 48) as char;
        }
        
        let r = (self.x * 255.0) as u8;
        let g = (self.y * 255.0) as u8;
        let b = (self.z * 255.0) as u8;
        out.push(int_to_hex((r >> 4) & 0xf));
        out.push(int_to_hex((r) & 0xf));
        out.push(int_to_hex((g >> 4) & 0xf));
        out.push(int_to_hex((g) & 0xf));
        out.push(int_to_hex((b >> 4) & 0xf));
        out.push(int_to_hex((b) & 0xf));
    }
    
    fn color(value: &str) -> Vec4 {
        if let Ok(val) = Self::from_hex_str(value) {
            val
        }
        else {
            Vec4 {x: 1.0, y: 0.0, z: 1.0, w: 1.0}
        }
    }
    
    fn from_hex_bytes(bytes: &[u8]) -> Result<Vec4, ()> {
        let color = if bytes.len()>2 && bytes[0] == '#' as u8 {
            colorhex::hex_bytes_to_u32(&bytes[1..])?
        }
        else {
            colorhex::hex_bytes_to_u32(bytes)?
        };
        Ok(Vec4 {
            x: (((color >> 24)&0xff) as f32) / 255.0,
            y: (((color >> 16)&0xff) as f32) / 255.0,
            z: (((color >> 8)&0xff) as f32) / 255.0,
            w: (((color >> 0)&0xff) as f32) / 255.0,
        })
    }
}
