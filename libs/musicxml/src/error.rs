use std::fmt;

pub type MusicXmlResult<T> = Result<T, MusicXmlError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XmlErrorKind {
    UnexpectedEnd,
    UnexpectedToken,
    InvalidName,
    InvalidAttribute,
    DuplicateAttribute,
    InvalidEntity,
    InvalidCharacterReference,
    InvalidComment,
    InvalidCData,
    InvalidDeclaration,
    InvalidDoctype,
    MismatchedClosingTag,
    MultipleRoots,
    MissingRoot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlError {
    pub kind: XmlErrorKind,
    pub message: String,
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

impl XmlError {
    pub(crate) fn at(kind: XmlErrorKind, message: impl Into<String>, source: &str, offset: usize) -> Self {
        let offset = offset.min(source.len());
        let prefix = &source[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = prefix
            .rsplit_once('\n')
            .map_or_else(|| prefix.chars().count() + 1, |(_, tail)| tail.chars().count() + 1);
        Self {
            kind,
            message: message.into(),
            offset,
            line,
            column,
        }
    }
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XML error at {}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for XmlError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversionError {
    UnsupportedStructure(String),
    MissingPartId,
    DuplicatePartId(String),
    UnequalMeasureCounts,
    UnequalMeasureAttributes { measure_index: usize },
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedStructure(message) => f.write_str(message),
            Self::MissingPartId => f.write_str("a part has no id attribute"),
            Self::DuplicatePartId(id) => write!(f, "duplicate part id {id:?}"),
            Self::UnequalMeasureCounts => f.write_str("parts have unequal measure counts"),
            Self::UnequalMeasureAttributes { measure_index } => write!(
                f,
                "partwise measure attributes differ at measure index {measure_index}"
            ),
        }
    }
}

impl std::error::Error for ConversionError {}

#[derive(Debug)]
pub enum MusicXmlError {
    Xml(XmlError),
    InvalidRoot(String),
    InvalidMxl(String),
    UnsupportedEncoding(String),
    ZipRead(String),
    ZipWrite(String),
    Conversion(ConversionError),
}

impl fmt::Display for MusicXmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => error.fmt(f),
            Self::InvalidRoot(name) => write!(
                f,
                "expected score-partwise or score-timewise root, found {name:?}"
            ),
            Self::InvalidMxl(message) => write!(f, "invalid MXL container: {message}"),
            Self::UnsupportedEncoding(encoding) => {
                write!(f, "unsupported XML encoding {encoding:?}")
            }
            Self::ZipRead(message) => write!(f, "ZIP read error: {message}"),
            Self::ZipWrite(message) => write!(f, "ZIP write error: {message}"),
            Self::Conversion(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for MusicXmlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::Conversion(error) => Some(error),
            _ => None,
        }
    }
}

impl From<XmlError> for MusicXmlError {
    fn from(value: XmlError) -> Self {
        Self::Xml(value)
    }
}

impl From<ConversionError> for MusicXmlError {
    fn from(value: ConversionError) -> Self {
        Self::Conversion(value)
    }
}
