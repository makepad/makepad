
use std::io::{self, Write};

#[derive(Default, Copy, Clone, Debug)]
pub struct StringIndex(pub u32);

// an appendable table of strings that store the stringlength as a u32 in front
pub struct StringTable{
    buffer: Vec<u8>
}

impl Default for StringTable{
    fn default()->Self{
        Self{
            buffer: vec![0,0,0,0] // put an empty string at position 0
        }
    }
}

// Implement the Write trait for MyBuffer.
impl Write for StringTable {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Delegate the write operation to the inner Vec<u8>.
        self.buffer.write(buf)
    }
    
    fn flush(&mut self) -> io::Result<()> {
        // Delegate the flush operation to the inner Vec<u8>.
        self.buffer.flush()
    }
}

impl StringTable{
    pub fn lookup(&self, index:StringIndex)->&str{
        let index = index.0 as usize;
        let len = u32::from_le_bytes(self.buffer[index..index+4].try_into().unwrap());
        unsafe{str::from_utf8_unchecked(&self.buffer[(index + 4)..(len as usize)])}
    }
    
    pub fn set_length(&mut self, index:StringIndex){
        let index = index.0 as usize;
        let len_bytes = (self.buffer.len() as u32).to_le_bytes();
        for i in 0..4{
            self.buffer[index + i] = len_bytes[i]
        }
    }
    
    pub fn pop(&mut self, index:StringIndex){
        let index = index.0 as usize;
        let len = u32::from_le_bytes(self.buffer[index..index+4].try_into().unwrap());
        assert!(self.buffer.len() == index + (len as usize) + 4);
        self.buffer.truncate(index);
    }
    
    pub fn add_empty(&mut self)->StringIndex{
        let index = self.buffer.len();
        let len_bytes = (0 as u32).to_le_bytes();
        self.buffer.extend_from_slice(&len_bytes);
        StringIndex(index as u32)
    }
    
    pub fn add_string(&mut self, s:&str)->StringIndex{
        let index = self.buffer.len();
        let len_bytes = (s.len() as u32).to_le_bytes();
        self.buffer.extend_from_slice(&len_bytes);
        self.buffer.extend_from_slice(&s.as_bytes());
        StringIndex(index as u32)
    }
    
    pub fn add_char(&mut self, c:char)->StringIndex{
        let mut buf = [0u8;4];
        self.add_string(c.encode_utf8(&mut buf))
    }
    
    // can only call this on the last item in the buffer
        
    pub fn append_string(&mut self, index:StringIndex, s:&str){
        let index = index.0 as usize;
        let mut len = u32::from_le_bytes(self.buffer[index..index+4].try_into().unwrap());
        assert!(self.buffer.len() == index + (len as usize) + 4);
        let bytes = s.as_bytes();
        self.buffer.extend_from_slice(&s.as_bytes());
        len += bytes.len() as u32;
        // write back the length
        let len_bytes = len.to_le_bytes();
        for i in 0..4{
            self.buffer[index + i] = len_bytes[i]
        }
    }
        
    pub fn append_char(&mut self, index:StringIndex,  c:char){
        let mut buf = [0u8;4];
        self.append_string(index, c.encode_utf8(&mut buf));
    }
}
