use core::fmt;

/// A typed failure while reading an SF2 byte stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Truncated { offset: usize, needed: usize },
    InvalidRiff,
    InvalidForm,
    MissingChunk(&'static str),
    DuplicateChunk([u8; 4]),
    InvalidChunkSize { chunk: [u8; 4], size: usize },
    LimitExceeded { what: &'static str, limit: usize },
    InvalidHierarchy(&'static str),
    InvalidGenerator { operator: u16 },
    InvalidSample { sample: usize, reason: &'static str },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { offset, needed } => {
                write!(f, "truncated SF2 at byte {offset} (need {needed} bytes)")
            }
            Self::InvalidRiff => f.write_str("input is not a valid RIFF file"),
            Self::InvalidForm => f.write_str("RIFF form is not SoundFont 'sfbk'"),
            Self::MissingChunk(name) => write!(f, "missing required SF2 chunk {name}"),
            Self::DuplicateChunk(id) => write!(f, "duplicate SF2 chunk {:?}", id),
            Self::InvalidChunkSize { chunk, size } => {
                write!(f, "invalid {:?} chunk size {size}", chunk)
            }
            Self::LimitExceeded { what, limit } => write!(f, "{what} exceeds limit {limit}"),
            Self::InvalidHierarchy(reason) => write!(f, "invalid SF2 hierarchy: {reason}"),
            Self::InvalidGenerator { operator } => {
                write!(f, "invalid SoundFont generator operator {operator}")
            }
            Self::InvalidSample { sample, reason } => {
                write!(f, "invalid SoundFont sample {sample}: {reason}")
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// A typed failure in a supported SFZ opcode or structure.
#[derive(Clone, Debug, PartialEq)]
pub enum SfzError {
    LimitExceeded { what: &'static str, limit: usize },
    InvalidHeader { line: usize, header: String },
    InvalidValue { line: usize, opcode: String, value: String },
    MissingSample { region: usize },
    UnterminatedQuote { line: usize },
    UnterminatedComment,
}

impl fmt::Display for SfzError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded { what, limit } => write!(f, "{what} exceeds limit {limit}"),
            Self::InvalidHeader { line, header } => {
                write!(f, "unsupported SFZ header <{header}> at line {line}")
            }
            Self::InvalidValue { line, opcode, value } => {
                write!(f, "invalid value '{value}' for SFZ opcode {opcode} at line {line}")
            }
            Self::MissingSample { region } => write!(f, "SFZ region {region} has no sample"),
            Self::UnterminatedQuote { line } => write!(f, "unterminated SFZ quote at line {line}"),
            Self::UnterminatedComment => f.write_str("unterminated SFZ block comment"),
        }
    }
}

impl std::error::Error for SfzError {}
