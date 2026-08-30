//! A faithful MusicXML document tier.
//!
//! The generic XML tree is retained in full and typed, borrowing views expose
//! MusicXML concepts without forcing them into an application's score model.

mod document;
mod error;
mod mxl;
mod xml;

pub use document::*;
pub use error::{ConversionError, MusicXmlError, MusicXmlResult, XmlError, XmlErrorKind};
pub use mxl::{
    parse_musicxml_bytes, parse_mxl, parse_mxl_reader, write_mxl, write_mxl_with_rootfile,
    DEFAULT_ROOTFILE_PATH,
};
pub use xml::{
    parse_xml, write_xml, XmlAttribute, XmlDeclaration, XmlDocument, XmlElement, XmlNode,
};

/// Parses an uncompressed MusicXML document.
pub fn parse_musicxml(source: &str) -> MusicXmlResult<MusicXmlDocument> {
    MusicXmlDocument::parse(source)
}

/// Serializes a MusicXML document as UTF-8 XML.
pub fn write_musicxml(document: &MusicXmlDocument) -> MusicXmlResult<String> {
    document.to_xml_string()
}

#[cfg(test)]
mod tests;
