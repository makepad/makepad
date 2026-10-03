//! String: owned UTF-8 text.

pub struct String {
    vec: crate::vec::Vec<u8>,
}

impl String {
    pub fn new() -> String {
        String { vec: crate::vec::Vec::new() }
    }
    pub fn from(s: &str) -> String {
        let mut r = String::new();
        r.push_str(s);
        r
    }
    pub fn len(&self) -> usize {
        self.vec.len()
    }
    pub fn is_empty(&self) -> bool {
        self.vec.len() == 0
    }
    pub fn push_str(&mut self, s: &str) {
        let b = s.as_bytes();
        self.vec.reserve(b.len());
        let mut i = 0;
        while i < b.len() {
            self.vec.push(b[i]);
            i += 1;
        }
    }
    pub fn push(&mut self, c: char) {
        let mut buf = [0u8; 4];
        let n = crate::char_utf8::encode(c as u32, &mut buf);
        let mut i = 0;
        while i < n {
            self.vec.push(buf[i]);
            i += 1;
        }
    }
    pub fn as_str(&self) -> &str {
        unsafe { crate::mem::intrinsics_mem::str_from_raw(self.vec.as_ptr(), self.vec.len()) }
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.vec.as_slice()
    }
    pub fn clear(&mut self) {
        self.vec.clear();
    }
}

impl crate::ops::Deref for String {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl crate::clone::Clone for String {
    fn clone(&self) -> String {
        String::from(self.as_str())
    }
}

impl crate::cmp::PartialEq for String {
    fn eq(&self, o: &String) -> bool {
        self.as_str() == o.as_str()
    }
}

pub trait ToString {
    fn to_string(&self) -> String;
}

impl ToString for str {
    fn to_string(&self) -> String {
        String::from(self)
    }
}
