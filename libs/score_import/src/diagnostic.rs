use std::collections::BTreeMap;
use std::fmt;

/// A stable address in the input, suitable for presenting conversion feedback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceLocation {
    MusicXml {
        part: Option<String>,
        measure: Option<String>,
        element: String,
        occurrence: usize,
    },
    Midi {
        sequence: usize,
        track: Option<usize>,
        tick: Option<u64>,
        event: Option<usize>,
    },
    Model {
        entity: String,
    },
    Document,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    Ignored,
    Approximated,
    Repaired,
}

/// One non-lossless conversion decision. Successfully imported feature totals
/// live in [`ImportStats`] to avoid producing enormous reports for large scores.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportDiagnostic {
    pub kind: DiagnosticKind,
    pub code: &'static str,
    pub message: String,
    pub source: SourceLocation,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportStats {
    pub imported: BTreeMap<&'static str, usize>,
    pub ignored: usize,
    pub approximated: usize,
    pub repaired: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportReport {
    pub stats: ImportStats,
    pub diagnostics: Vec<ImportDiagnostic>,
    pub inferences: Vec<Inference>,
}

impl ImportReport {
    pub fn imported(&mut self, feature: &'static str) {
        *self.stats.imported.entry(feature).or_default() += 1;
    }

    pub fn ignored(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        source: SourceLocation,
    ) {
        self.stats.ignored += 1;
        self.diagnostics.push(ImportDiagnostic {
            kind: DiagnosticKind::Ignored,
            code,
            message: message.into(),
            source,
        });
    }

    pub fn approximated(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        source: SourceLocation,
    ) {
        self.stats.approximated += 1;
        self.diagnostics.push(ImportDiagnostic {
            kind: DiagnosticKind::Approximated,
            code,
            message: message.into(),
            source,
        });
    }

    pub fn repaired(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        source: SourceLocation,
    ) {
        self.stats.repaired += 1;
        self.diagnostics.push(ImportDiagnostic {
            kind: DiagnosticKind::Repaired,
            code,
            message: message.into(),
            source,
        });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inference {
    pub kind: InferenceKind,
    pub confidence_milli: u16,
    pub detail: String,
    pub source: SourceLocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InferenceKind {
    Quantization,
    VoiceSeparation,
    StaffSplit,
    KeySignature,
    TimeSignature,
    PitchSpelling,
    Tuplet,
}

#[derive(Debug)]
pub enum ImportError {
    MusicXml(makepad_musicxml::MusicXmlError),
    MusicXmlConversion(makepad_musicxml::ConversionError),
    Midi(makepad_midi_file::MidiError),
    Arithmetic(makepad_score::model::RationalError),
    InvalidSource(String),
    Unsupported(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MusicXml(error) => write!(formatter, "MusicXML: {error}"),
            Self::MusicXmlConversion(error) => write!(formatter, "MusicXML conversion: {error}"),
            Self::Midi(error) => write!(formatter, "MIDI: {error}"),
            Self::Arithmetic(error) => write!(formatter, "score arithmetic: {error}"),
            Self::InvalidSource(message) => write!(formatter, "invalid source: {message}"),
            Self::Unsupported(message) => write!(formatter, "unsupported source: {message}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<makepad_musicxml::MusicXmlError> for ImportError {
    fn from(value: makepad_musicxml::MusicXmlError) -> Self {
        Self::MusicXml(value)
    }
}

impl From<makepad_musicxml::ConversionError> for ImportError {
    fn from(value: makepad_musicxml::ConversionError) -> Self {
        Self::MusicXmlConversion(value)
    }
}

impl From<makepad_midi_file::MidiError> for ImportError {
    fn from(value: makepad_midi_file::MidiError) -> Self {
        Self::Midi(value)
    }
}

impl From<makepad_score::model::RationalError> for ImportError {
    fn from(value: makepad_score::model::RationalError) -> Self {
        Self::Arithmetic(value)
    }
}
