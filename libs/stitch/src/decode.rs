use std::{error::Error, fmt, marker::PhantomData, str, sync::Arc};

#[derive(Clone, Debug)]
pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    #[inline]
    pub(crate) fn is_at_end(&self) -> bool {
        self.position == self.bytes.len()
    }

    #[inline]
    pub(crate) fn read_byte(&mut self) -> Result<u8, DecodeError> {
        let byte = self
            .bytes
            .get(self.position)
            .copied()
            .ok_or_else(|| DecodeError::new("unexpected end"))?;
        self.position += 1;
        Ok(byte)
    }

    #[inline]
    pub(crate) fn read_bytes(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        let bytes = self
            .bytes
            .get(self.position..self.position + count)
            .ok_or_else(|| DecodeError::new("unexpected end"))?;
        self.position += count;
        Ok(bytes)
    }

    #[inline]
    pub(crate) fn read_bytes_until_end(&mut self) -> &'a [u8] {
        let bytes = &self.bytes[self.position..];
        self.position = self.bytes.len();
        bytes
    }

    #[inline]
    pub(crate) fn decode_bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len: u32 = self.decode()?;
        Ok(self.read_bytes(len as usize)?)
    }

    #[inline]
    pub(crate) fn decode_string(&mut self) -> Result<&'a str, DecodeError> {
        let len: u32 = self.decode()?;
        Ok(str::from_utf8(self.read_bytes(len as usize)?)
            .map_err(|_| DecodeError::new("malformed string"))?)
    }

    pub(crate) fn decode_iter<T>(&mut self) -> Result<DecodeIter<'_, 'a, T>, DecodeError>
    where
        T: Decode,
    {
        let count: u32 = self.decode()?;
        Ok(DecodeIter {
            decoder: self,
            count: count as usize,
            phantom: PhantomData,
        })
    }

    pub(crate) fn decode_decoder(&mut self) -> Result<Decoder<'a>, DecodeError> {
        let count: u32 = self.decode()?;
        Ok(Decoder::new(self.read_bytes(count as usize)?))
    }

    pub(crate) fn decode<T>(&mut self) -> Result<T, DecodeError>
    where
        T: Decode,
    {
        T::decode(self)
    }
}

#[derive(Debug)]
pub(crate) struct DecodeIter<'a, 'b, T> {
    decoder: &'a mut Decoder<'b>,
    count: usize,
    phantom: PhantomData<T>,
}

impl<'a, 'b, T> Iterator for DecodeIter<'a, 'b, T>
where
    T: Decode,
{
    type Item = Result<T, DecodeError>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        Some(self.decoder.decode())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

pub(crate) trait Decode: Sized {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError>;
}

impl Decode for i32 {
    #[inline]
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        fn decode_i32_tail(decoder: &mut Decoder<'_>, mut val: i32) -> Result<i32, DecodeError> {
            let mut shift = 7;
            loop {
                let byte = decoder.read_byte()?;
                if shift >= 25 {
                    let bits = (byte << 1) as i8 >> (32 - shift);
                    if byte & 0x80 != 0 || bits != 0 && bits != -1 {
                        return Err(DecodeError::new("malformed i32"));
                    }
                }
                val |= ((byte & 0x7F) as i32) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            let shift = 25 - shift.min(25);
            Ok(val << shift >> shift)
        }

        let byte = decoder.read_byte()?;
        let val = (byte & 0x7F) as i32;
        if byte & 0x80 == 0 {
            Ok(val << 25 >> 25)
        } else {
            decode_i32_tail(decoder, val)
        }
    }
}

impl Decode for u32 {
    #[inline]
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        fn decode_u32_tail(decoder: &mut Decoder<'_>, mut val: u32) -> Result<u32, DecodeError> {
            let mut shift = 7;
            loop {
                let byte = decoder.read_byte()?;
                if shift >= 25 && byte >> 32 - shift != 0 {
                    return Err(DecodeError::new("malformed u32"));
                }
                val |= ((byte & 0x7F) as u32) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            Ok(val)
        }

        let byte = decoder.read_byte()?;
        let val = (byte & 0x7F) as u32;
        if byte & 0x80 == 0 {
            Ok(val)
        } else {
            decode_u32_tail(decoder, val)
        }
    }
}

impl Decode for i64 {
    #[inline]
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        fn decode_i64_tail(decoder: &mut Decoder<'_>, mut val: i64) -> Result<i64, DecodeError> {
            let mut shift = 7;
            loop {
                let byte = decoder.read_byte()?;
                if shift >= 57 {
                    let bits = (byte << 1) as i8 >> (64 - shift);
                    if byte & 0x80 != 0 || bits != 0 && bits != -1 {
                        return Err(DecodeError::new("malformed i64"));
                    }
                }
                val |= ((byte & 0x7F) as i64) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            let shift = 57 - shift.min(57);
            Ok(val << shift >> shift)
        }

        let byte = decoder.read_byte()?;
        let val = (byte & 0x7F) as i64;
        if byte & 0x80 == 0 {
            Ok(val << 57 >> 57)
        } else {
            decode_i64_tail(decoder, val)
        }
    }
}

impl Decode for usize {
    #[inline]
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(usize::try_from(decoder.decode::<u32>()?).unwrap())
    }
}

impl Decode for f32 {
    #[inline]
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self::from_le_bytes(
            decoder.read_bytes(4)?.try_into().unwrap(),
        ))
    }
}

impl Decode for f64 {
    #[inline]
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self::from_le_bytes(
            decoder.read_bytes(8)?.try_into().unwrap(),
        ))
    }
}

impl Decode for Arc<[u8]> {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(decoder.decode_bytes()?.into())
    }
}

impl Decode for Arc<str> {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(decoder.decode_string()?.into())
    }
}

/// An error that can occur when decoding a [`Module`](crate::Module).
#[derive(Clone, Debug)]
pub struct DecodeError {
    message: Box<str>,
}

impl DecodeError {
    /// Creates a new [`DecodeError`] with the given message.
    pub fn new(message: impl Into<Box<str>>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl Error for DecodeError {}
