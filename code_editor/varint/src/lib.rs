use std::{
    error::Error,
    fmt,
    io,
    io::{Read, Write},
};

macro_rules! read_varsint {
    {
        $(
            $(#[$attrs:meta])*
            fn $read_varsint:ident($sint:ty, $read_varuint:ident);
        )*
    } => {
        $(
            $(#[$attrs])*
            fn $read_varsint(&mut self) -> Result<$sint, ReadVarintError> {
                Ok(self.$read_varuint()?.rotate_right(1) as $sint)
            }
        )*
    }
}

macro_rules! read_varuint {
    {
        $(
            $(#[$attrs:meta])*
            fn $read_varuint:ident($uint:ty);
        )*
    } => {
        $(
            $(#[$attrs])*
            fn $read_varuint(&mut self) -> Result<$uint, ReadVarintError> {
                let mut value = 0;
                let mut shift = 0;
                loop {
                    let mut buffer = [0u8; 1];
                    self.read_exact(&mut buffer)?;
                    value |= ((buffer[0] & 0x7F) as $uint) << shift;
                    if buffer[0] < 0x80 {
                        break;
                    }
                    shift += 7;
                    if shift > <$uint>::BITS {
                        return Err(ReadVarintError::Overflow);
                    }
                }
                Ok(value)
            }
        )*
    }
}

/// Extends `Read` with methods to read variable-length integers.
pub trait ReadVarint: Read {
    read_varsint! {
        /// Reads a signed 8-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_vari8(i8, read_varu8);

        /// Reads a signed 16-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_vari16(i16, read_varu16);

        /// Reads a signed 32-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_vari32(i32, read_varu32);

        /// Reads a signed 64-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_vari64(i64, read_varu64);

        /// Reads a signed 128-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_vari128(i128, read_varu128);
    }

    read_varuint! {
        /// Reads an unsigned 8-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_varu8(u8);

        /// Reads an unsigned 16-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_varu16(u16);

        /// Reads an unsigned 32-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_varu32(u32);

        /// Reads an unsigned 64-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_varu64(u64);

        /// Reads an unsigned 128-bit variable-length integer from the underlying reader.
        ///
        /// # Errors
        ///
        /// Returns `ReadVarintError::Io` if the underlying reader returned an error, or
        /// `ReadVarintError::Overflow` if the integer is too large to store in the target integer
        /// type.
        fn read_varu128(u128);
    }
}

impl<T: Read> ReadVarint for T {}

/// An error that can be returned when reading a variable-length integer.
#[derive(Debug)]
pub enum ReadVarintError {
    /// The underlying reader returned an error.
    Io(io::Error),
    /// The integer is too large to store in the target integer type.
    Overflow,
}

impl fmt::Display for ReadVarintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => write!(f, "the underlying reader returned an error"),
            Self::Overflow => write!(
                f,
                "the integer is too large to store in the target integer type"
            ),
        }
    }
}

impl Error for ReadVarintError {}

impl From<io::Error> for ReadVarintError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

macro_rules! write_varsint {
    {
        $(
            $(#[$attrs:meta])*
            fn $write_varsint:ident($sint:ty, $write_varuint:ident, $uint:ty);
        )*
    } => {
        $(
            $(#[$attrs])*
            fn $write_varsint(&mut self, value: $sint) -> io::Result<()> {
                self.$write_varuint((value as $uint).rotate_left(1))
            }
        )*
    }
}

macro_rules! write_varuint {
    {
        $(
            $(#[$attrs:meta])*
            fn $write_varuint:ident($uint:ty);
        )*
    } => {
        $(
            $(#[$attrs])*
            fn $write_varuint(&mut self, mut value: $uint) -> io::Result<()> {
                while value >= 0x80 {
                    let buffer = [value as u8 | 0x80];
                    self.write_all(&buffer)?;
                    value >>= 7;
                }
                let buffer = [value as u8];
                self.write_all(&buffer)
            }
        )*
    }
}

/// Extends `Write` with methods to write variable-length integers.
pub trait WriteVarint: Write {
    write_varsint! {
        /// Writes a signed 8-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_vari8(i8, write_varu8, u8);

        /// Writes a signed 16-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_vari16(i16, write_varu16, u16);

        /// Writes a signed 32-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_vari32(i32, write_varu32, u32);

        /// Writes a signed 64-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_vari64(i64, write_varu64, u64);

        /// Writes a signed 128-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_vari128(i128, write_varu128, u128);
    }

    write_varuint! {
        /// Writes an unsigned 8-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_varu8(u8);

        /// Writes an unsigned 16-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_varu16(u16);

        /// Writes an unsigned 32-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_varu32(u32);

        /// Writes an unsigned 64-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_varu64(u64);

        /// Writes an unsigned 128-bit variable-length integer to the underlying writer.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the underlying writer.
        fn write_varu128(u128);
    }
}

impl<T: Write> WriteVarint for T {}

#[cfg(test)]
mod tests {
    use {
        super::*,
        proptest::{prelude::*, test_runner::TestRunner},
    };

    macro_rules! varint {
        ($varint:ident, $int:ty, $write_varint:ident, $read_varint:ident) => {
            #[test]
            fn $varint() {
                let mut runner = TestRunner::default();
                runner
                    .run(&any::<$int>(), |value| {
                        let mut bytes = Vec::new();
                        bytes.$write_varint(value).unwrap();
                        assert_eq!(bytes.as_slice().$read_varint().unwrap(), value);
                        Ok(())
                    })
                    .unwrap();
            }
        };
    }

    varint!(vari8, i8, write_vari8, read_vari8);
    varint!(vari16, i16, write_vari16, read_vari16);
    varint!(vari32, i32, write_vari32, read_vari32);
    varint!(vari64, i64, write_vari64, read_vari64);
    varint!(vari128, i128, write_vari128, read_vari128);
    varint!(varu8, u8, write_varu8, read_varu8);
    varint!(varu16, u16, write_varu16, read_varu16);
    varint!(varu32, u32, write_varu32, read_varu32);
    varint!(varu64, u64, write_varu64, read_varu64);
    varint!(varu128, u128, write_varu128, read_varu128);
}
