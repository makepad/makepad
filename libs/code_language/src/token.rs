//! Compact token roles and spans. Continuation state is provider-owned; this
//! module never packs Rust block-comment depth into a universal integer.

/// Role of a token for colouring, search extraction and line summaries.
/// Language-specific detail stays in the producing frontend.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenRole {
    Unknown = 0,
    Whitespace = 1,
    Comment = 2,
    Identifier = 3,
    Keyword = 4,
    BranchKeyword = 5,
    LoopKeyword = 6,
    Typename = 7,
    Function = 8,
    Macro = 9,
    Number = 10,
    String = 11,
    Char = 12,
    Punctuator = 13,
    Delimiter = 14,
    Preprocessor = 15,
    Constant = 16,
}

impl TokenRole {
    pub fn is_trivia(self) -> bool {
        matches!(self, TokenRole::Whitespace | TokenRole::Comment)
    }

    pub fn is_identifier_like(self) -> bool {
        matches!(
            self,
            TokenRole::Identifier
                | TokenRole::Typename
                | TokenRole::Function
                | TokenRole::Macro
                | TokenRole::Constant
                | TokenRole::Keyword
                | TokenRole::BranchKeyword
                | TokenRole::LoopKeyword
        )
    }
}

/// A token spanning `[start, end)` bytes of the original UTF-8 (or raw) document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TokenSpan {
    pub start: u32,
    pub end: u32,
    pub role: TokenRole,
}

impl TokenSpan {
    pub fn new(start: u32, end: u32, role: TokenRole) -> Self {
        TokenSpan { start, end, role }
    }

    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// Four-bucket mix used by map line summaries: comment, literal, identifier, syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LineMix(pub [u8; 4]);

impl LineMix {
    pub fn from_counts(comment: u32, literal: u32, ident: u32, syntax: u32) -> Self {
        let total = comment
            .saturating_add(literal)
            .saturating_add(ident)
            .saturating_add(syntax);
        if total == 0 {
            return LineMix([0; 4]);
        }
        let scale = |n: u32| ((n as u64 * 255) / total as u64) as u8;
        LineMix([scale(comment), scale(literal), scale(ident), scale(syntax)])
    }

    pub fn add_role(&mut self, role: TokenRole, bytes: u32) {
        match role {
            TokenRole::Comment => self.0[0] = self.0[0].saturating_add(bytes.min(255) as u8),
            TokenRole::String | TokenRole::Char | TokenRole::Number | TokenRole::Constant => {
                self.0[1] = self.0[1].saturating_add(bytes.min(255) as u8)
            }
            TokenRole::Identifier
            | TokenRole::Typename
            | TokenRole::Function
            | TokenRole::Macro
            | TokenRole::Keyword
            | TokenRole::BranchKeyword
            | TokenRole::LoopKeyword => {
                self.0[2] = self.0[2].saturating_add(bytes.min(255) as u8)
            }
            TokenRole::Whitespace => {}
            _ => self.0[3] = self.0[3].saturating_add(bytes.min(255) as u8),
        }
    }
}

/// One line's compact lexical summary. Continuation is provider-owned and not
/// stored here; consumers keep roles, spans and this mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LineSummary {
    pub mix: LineMix,
    pub token_count: u16,
}
