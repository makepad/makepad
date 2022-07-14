use {
    makepad_micro_serde::{DeBin, DeBinErr, SerBin},
    std::{borrow::Cow, fmt, ops::{Deref, DerefMut}}
};

#[derive(Clone, Default, DeBin, Eq, Hash, PartialEq, PartialOrd, Ord, SerBin)]
pub struct UnixString {
    bytes: Vec<u8>
}

impl UnixString {
    pub fn new() -> Self {
        Self::from_vec(Vec::new())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::from_vec(Vec::with_capacity(capacity))
    }

    pub fn from_vec(vec: Vec<u8>) -> Self {
        Self {
            bytes: vec
        }
    }

    pub fn into_boxed_unix_str(self) -> Box<UnixStr> {
        let raw = Box::into_raw(self.bytes.into_boxed_slice()) as *mut UnixStr;
        unsafe { Box::from_raw(raw) }
    }

    pub fn into_string(self) -> Result<String, Self> {
        String::from_utf8(self.bytes).map_err(|err| Self::from_vec(err.into_bytes()))
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.bytes
    }

    pub fn as_unix_str(&self) -> &UnixStr {
        UnixStr::from_bytes(&self.bytes)
    }

    pub fn as_mut_unix_str(&mut self) -> &mut UnixStr {
        UnixStr::from_mut_bytes(&mut self.bytes)
    }

    pub fn as_mut_vec(&mut self) -> &mut Vec<u8> {
        &mut self.bytes
    }

    pub fn reserve(&mut self, additional: usize) {
        self.bytes.reserve(additional)
    }

    pub fn reserve_exact(&mut self, additional: usize) {
        self.bytes.reserve_exact(additional)
    }

    pub fn shrink_to_fit(&mut self) {
        self.bytes.shrink_to_fit()
    }

    pub fn push<S: AsRef<UnixStr>>(&mut self, string: S) {
        self.bytes.extend_from_slice(&string.as_ref().bytes)
    }

    pub fn clear(&mut self) {
        self.bytes.clear()
    }
}

impl Deref for UnixString {
    type Target = UnixStr;

    fn deref(&self) -> &Self::Target {
        UnixStr::from_bytes(&self.bytes)
    }
}

impl DerefMut for UnixString {
    fn deref_mut(&mut self) -> &mut Self::Target {
        UnixStr::from_mut_bytes(&mut self.bytes)
    }
}

impl fmt::Debug for UnixString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.to_string_lossy())
    }
}

impl AsRef<UnixStr> for UnixString {
    fn as_ref(&self) -> &UnixStr {
        self.as_unix_str()
    }
}

#[derive(Eq, Hash, PartialEq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct UnixStr {
    bytes: [u8]
}

impl UnixStr {
    pub fn new<S: AsRef<UnixStr> + ?Sized>(string: &S) -> &Self {
        string.as_ref()
    }

    pub fn from_bytes(bytes: &[u8]) -> &Self {
        unsafe { std::mem::transmute(bytes) }
    }

    pub fn from_mut_bytes(bytes: &mut [u8]) -> &mut Self {
        unsafe { std::mem::transmute(bytes) }
    }

    pub fn into_unix_string(self: Box<Self>) -> UnixString {
        let boxed = unsafe { Box::from_raw(Box::into_raw(self) as *mut [u8]) };
        UnixString {
            bytes: boxed.to_vec()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn to_unix_string(&self) -> UnixString {
        UnixString {
            bytes: self.bytes.to_owned()
        }
    }

    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes)
    }
}

impl fmt::Debug for UnixStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.to_string_lossy())
    }
}

impl AsRef<UnixStr> for UnixStr {
    fn as_ref(&self) -> &UnixStr {
        self
    }
}

impl AsRef<UnixStr> for str {
    fn as_ref(&self) -> &UnixStr {
        UnixStr::from_bytes(self.as_bytes())
    }
}

impl AsRef<UnixStr> for String {
    fn as_ref(&self) -> &UnixStr {
        UnixStr::from_bytes(self.as_bytes())
    }
}