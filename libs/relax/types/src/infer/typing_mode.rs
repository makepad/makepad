// ported from N:rustc_type_ir/src/infer_ctxt.rs @8d1a764
use crate::tcx::DefiningOpaquesId;

/// infer_ctxt.rs:71: owner sets are interned, including the Typeck generator owners.
#[derive(Clone, Copy, Debug, Hash)]
pub enum TypingMode {
    Coherence,
    Typeck { defining_opaque_types_and_generators: DefiningOpaquesId },
    PostTypeckUntilBorrowck { defining_opaque_types: DefiningOpaquesId },
    PostBorrowck { defined_opaque_types: DefiningOpaquesId },
    Reflection,
    PostAnalysis,
    Codegen,
    ErasedNotCoherence,
}

impl TypingMode {
    /// infer_ctxt.rs:309.
    pub fn non_body_analysis() -> Self { Self::Typeck { defining_opaque_types_and_generators: DefiningOpaquesId::default() } }
    /// infer_ctxt.rs:331.
    pub fn borrowck(tcx: &crate::tcx::Tcx, prog: &crate::program::Program, body_def_id: crate::program::DefId) -> Self {
        let defining_opaque_types = tcx.opaque_types_defined_by(prog, body_def_id);
        if defining_opaque_types == DefiningOpaquesId::default() { Self::non_body_analysis() }
        else { Self::PostTypeckUntilBorrowck { defining_opaque_types } }
    }
    pub fn is_coherence(self) -> bool {
        match self {
            Self::Coherence => true,
            Self::Typeck { .. } | Self::PostTypeckUntilBorrowck { .. } | Self::PostBorrowck { .. } |
            Self::Reflection | Self::PostAnalysis | Self::Codegen | Self::ErasedNotCoherence => false,
        }
    }
    pub fn is_erased_not_coherence(self) -> bool {
        match self {
            Self::ErasedNotCoherence => true,
            Self::Coherence | Self::Typeck { .. } | Self::PostTypeckUntilBorrowck { .. } |
            Self::PostBorrowck { .. } | Self::Reflection | Self::PostAnalysis | Self::Codegen => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash)]
pub struct TypingModeEqWrapper(pub TypingMode);
impl PartialEq for TypingModeEqWrapper {
    fn eq(&self, other: &Self) -> bool {
        match (self.0, other.0) {
            (TypingMode::Coherence, TypingMode::Coherence) |
            (TypingMode::Reflection, TypingMode::Reflection) |
            (TypingMode::PostAnalysis, TypingMode::PostAnalysis) |
            (TypingMode::Codegen, TypingMode::Codegen) |
            (TypingMode::ErasedNotCoherence, TypingMode::ErasedNotCoherence) => true,
            (TypingMode::Typeck { defining_opaque_types_and_generators: a }, TypingMode::Typeck { defining_opaque_types_and_generators: b }) => a == b,
            (TypingMode::PostTypeckUntilBorrowck { defining_opaque_types: a }, TypingMode::PostTypeckUntilBorrowck { defining_opaque_types: b }) => a == b,
            (TypingMode::PostBorrowck { defined_opaque_types: a }, TypingMode::PostBorrowck { defined_opaque_types: b }) => a == b,
            (TypingMode::Coherence | TypingMode::Reflection | TypingMode::PostAnalysis | TypingMode::Codegen |
            TypingMode::ErasedNotCoherence | TypingMode::Typeck { .. } | TypingMode::PostTypeckUntilBorrowck { .. } |
            TypingMode::PostBorrowck { .. }, _) => false,
        }
    }
}
impl Eq for TypingModeEqWrapper {}
