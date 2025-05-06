
  /*
    pub fn from_hex_str(hex: &str) -> Result<Vec4, ()> {
        Self::from_hex_bytes(hex.as_bytes())
    }*/
        /*
    pub fn from_hex_bytes(bytes: &[u8]) -> Result<Vec4, ()> {
        let color = if bytes.len()>2 && bytes[0] == '#' as u8 {
            hex_bytes_to_u32(&bytes[1..])?
        }
        else {
            hex_bytes_to_u32(bytes)?
        };
        Ok(Vec4 {
            x: (((color >> 24)&0xff) as f32) / 255.0,
            y: (((color >> 16)&0xff) as f32) / 255.0,
            z: (((color >> 8)&0xff) as f32) / 255.0,
            w: (((color >> 0)&0xff) as f32) / 255.0,
        })
    }*/

pub fn hex_bytes_to_u32(buf: &[u8]) -> Result<u32, ()> {
    fn hex_to_int(c: u8) -> Result<u32, ()> {
        if c >= 48 && c <= 57 {
            return Ok((c - 48) as u32);
        }
        if c >= 65 && c <= 70 {
            return Ok((c - 65 + 10) as u32);
        }
        if c >= 97 && c <= 102 {
            return Ok((c - 97 + 10) as u32);
        }
        return Err(());
    }
    
    match buf.len() {
        1 => {
            // #w
            let val = hex_to_int(buf[0]) ?;
            return Ok((val << 28) | (val << 24) | (val << 20) | (val << 16) | (val << 12) | (val << 8) | 0xff);
        }
        2 => { //#ww
            let val = (hex_to_int(buf[0]) ? << 4) + hex_to_int(buf[1]) ?;
            return Ok((val << 24) | (val << 16) | (val << 8) | 0xff)
        },
        3 => {
            // #rgb
            let r = hex_to_int(buf[0]) ?;
            let g = hex_to_int(buf[1]) ?;
            let b = hex_to_int(buf[2]) ?;
            return Ok((r << 28) | (r << 24) | (g << 20) | (g << 16) | (b << 12) | (b << 8) | 0xff);
        }
        4 => {
            // #rgba
            let r = hex_to_int(buf[0]) ?;
            let g = hex_to_int(buf[1]) ?;
            let b = hex_to_int(buf[2]) ?;
            let a = hex_to_int(buf[3]) ?;
            return Ok((r << 28) | (r << 24) | (g << 20) | (g << 16) | (b << 12) | (b << 8) | (a << 4) | a);
        }
        6 => {
            // #rrggbb
            let r = (hex_to_int(buf[0]) ? << 4) + hex_to_int(buf[1]) ?;
            let g = (hex_to_int(buf[2]) ? << 4) + hex_to_int(buf[3]) ?;
            let b = (hex_to_int(buf[4]) ? << 4) + hex_to_int(buf[5]) ?;
            return Ok((r << 24) | (g << 16) | (b << 8) | 0xff)
        }
        8 => {
            // #rrggbbaa
            let r = (hex_to_int(buf[0]) ? << 4) + hex_to_int(buf[1]) ?;
            let g = (hex_to_int(buf[2]) ? << 4) + hex_to_int(buf[3]) ?;
            let b = (hex_to_int(buf[4]) ? << 4) + hex_to_int(buf[5]) ?;
            let a = (hex_to_int(buf[6]) ? << 4) + hex_to_int(buf[7]) ?;
            return Ok((r << 24) | (g << 16) | (b << 8) | a)
        }
        _ => (),
    }
    return Err(());
} 
