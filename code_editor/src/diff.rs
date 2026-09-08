//! Worker-prepared, immutable diff rows. Endpoint line numbers are zero-based;
//! a final newline terminates a source line and does not create another one.
use crate::{
    decoration::{Decoration, DecorationSet, DecorationType},
    document::{CodeDocument, DocumentLayout, PreparedDocument},
    text::{Position, Text},
};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffRowKind {
    Equal,
    Added,
    Removed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiffRowSpec {
    pub kind: DiffRowKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub hunk: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GutterMode {
    #[default]
    Plain,
    Diff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffEndpoint {
    Old,
    New,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineTerminator {
    None,
    Lf,
    CrLf,
}

impl LineTerminator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffSourceLine {
    pub line: u32,
    /// Endpoint UTF-8 byte range, excluding its terminator.
    pub bytes: Range<usize>,
    pub terminator: LineTerminator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffRow {
    /// None only for the empty document's non-source display sentinel.
    pub kind: Option<DiffRowKind>,
    pub old: Option<DiffSourceLine>,
    pub new: Option<DiffSourceLine>,
    pub hunk: Option<u32>,
    /// Separate from row decorations, so overlap replacement cannot erase it.
    pub gutter_mark: Option<DecorationType>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunk {
    pub id: u32,
    pub rows: Range<usize>,
    pub old_lines: Range<usize>,
    pub new_lines: Range<usize>,
    pub added: usize,
    pub removed: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffMetadata {
    pub rows: Vec<DiffRow>,
    /// Contiguous changed runs; an input hunk ID may label multiple fragments.
    pub hunks: Vec<DiffHunk>,
    pub old_to_row: Vec<usize>,
    pub new_to_row: Vec<usize>,
    pub old_final_newline: bool,
    pub new_final_newline: bool,
}

impl DiffMetadata {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Reconstruct an endpoint using this metadata and its associated display text.
    pub fn reconstruct(&self, display: &Text, endpoint: DiffEndpoint) -> String {
        let map = match endpoint {
            DiffEndpoint::Old => &self.old_to_row,
            DiffEndpoint::New => &self.new_to_row,
        };
        let mut result = String::new();
        for &row in map {
            let source = match endpoint {
                DiffEndpoint::Old => self.rows[row].old.as_ref(),
                DiffEndpoint::New => self.rows[row].new.as_ref(),
            }
            .unwrap();
            result.push_str(&display.as_lines()[row]);
            result.push_str(source.terminator.as_str());
        }
        result
    }

    pub fn gutter_digits(&self) -> usize {
        self.old_to_row
            .len()
            .max(self.new_to_row.len())
            .max(1)
            .ilog10() as usize
            + 1
    }
}

/// Validated allocations; prepare a `PreparedView` from `document()` on the worker,
/// then move both outputs to the UI. Neither attachment performs row-wise work.
#[derive(Clone, Debug)]
pub struct PreparedDiffDocument {
    pub(crate) document: PreparedDocument,
    pub(crate) metadata: DiffMetadata,
    pub(crate) decorations: DecorationSet,
}

impl PreparedDiffDocument {
    pub fn document(&self) -> &PreparedDocument {
        &self.document
    }
    pub fn as_text(&self) -> &Text {
        self.document.as_text()
    }
    pub fn metadata(&self) -> &DiffMetadata {
        &self.metadata
    }
    pub fn decorations(&self) -> &DecorationSet {
        &self.decorations
    }
    pub fn row_count(&self) -> usize {
        self.metadata.row_count()
    }
    pub fn reconstruct(&self, endpoint: DiffEndpoint) -> String {
        self.metadata.reconstruct(self.as_text(), endpoint)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareDiffError {
    /// Missing, repeated, out-of-order, out-of-bounds, or wrong-side line number.
    InvalidRow {
        row: usize,
    },
    UnequalContent {
        row: usize,
    },
    IncompleteEndpoints,
}

fn source_lines(text: &str) -> Vec<DiffSourceLine> {
    let mut offset = 0;
    text.split_inclusive('\n')
        .enumerate()
        .map(|(line, text)| {
            let terminator = if text.ends_with("\r\n") {
                LineTerminator::CrLf
            } else if text.ends_with('\n') {
                LineTerminator::Lf
            } else {
                LineTerminator::None
            };
            let bytes = offset..offset + text.len() - terminator.as_str().len();
            offset += text.len();
            DiffSourceLine {
                line: line as u32,
                bytes,
                terminator,
            }
        })
        .collect()
}

pub(crate) fn prepare(
    specs: &[DiffRowSpec],
    old_text: &str,
    new_text: &str,
) -> Result<PreparedDiffDocument, PrepareDiffError> {
    let old = source_lines(old_text);
    let new = source_lines(new_text);
    let mut rows = Vec::with_capacity(specs.len());
    let (mut old_cursor, mut new_cursor) = (0, 0);
    for (index, spec) in specs.iter().enumerate() {
        let valid = match spec.kind {
            DiffRowKind::Equal => spec.old_line.is_some() && spec.new_line.is_some(),
            DiffRowKind::Added => spec.old_line.is_none() && spec.new_line.is_some(),
            DiffRowKind::Removed => spec.old_line.is_some() && spec.new_line.is_none(),
        };
        if !valid
            || spec
                .old_line
                .is_some_and(|line| line as usize != old_cursor || old_cursor >= old.len())
            || spec
                .new_line
                .is_some_and(|line| line as usize != new_cursor || new_cursor >= new.len())
        {
            return Err(PrepareDiffError::InvalidRow { row: index });
        }
        let old_source = spec.old_line.map(|_| old[old_cursor].clone());
        let new_source = spec.new_line.map(|_| new[new_cursor].clone());
        old_cursor += usize::from(old_source.is_some());
        new_cursor += usize::from(new_source.is_some());
        if let (Some(old), Some(new)) = (&old_source, &new_source) {
            if old_text[old.bytes.clone()] != new_text[new.bytes.clone()] {
                return Err(PrepareDiffError::UnequalContent { row: index });
            }
            if old.terminator != new.terminator {
                rows.push(DiffRow {
                    kind: Some(DiffRowKind::Removed),
                    old: old_source,
                    new: None,
                    hunk: Some(spec.hunk),
                    gutter_mark: None,
                });
                rows.push(DiffRow {
                    kind: Some(DiffRowKind::Added),
                    old: None,
                    new: new_source,
                    hunk: Some(spec.hunk),
                    gutter_mark: None,
                });
                continue;
            }
        }
        rows.push(DiffRow {
            kind: Some(spec.kind),
            old: old_source,
            new: new_source,
            hunk: Some(spec.hunk),
            gutter_mark: None,
        });
    }
    if old_cursor != old.len() || new_cursor != new.len() {
        return Err(PrepareDiffError::IncompleteEndpoints);
    }
    // Tokenize the endpoints independently, never the combined display stream.
    let old_document = CodeDocument::prepare(Text::from_display_lines(
        old.iter()
            .map(|line| old_text[line.bytes.clone()].to_owned())
            .collect(),
    ));
    let new_document = CodeDocument::prepare(Text::from_display_lines(
        new.iter()
            .map(|line| new_text[line.bytes.clone()].to_owned())
            .collect(),
    ));
    if rows.is_empty() {
        rows.push(DiffRow {
            kind: None,
            old: None,
            new: None,
            hunk: None,
            gutter_mark: None,
        });
    }
    let mut metadata = DiffMetadata {
        rows,
        hunks: Vec::new(),
        old_to_row: vec![0; old.len()],
        new_to_row: vec![0; new.len()],
        old_final_newline: old_text.ends_with('\n'),
        new_final_newline: new_text.ends_with('\n'),
    };
    let count = metadata.row_count();
    let mut lines = Vec::with_capacity(count);
    let mut layout = DocumentLayout {
        indent_state: Vec::with_capacity(count),
        tokens: Vec::with_capacity(count),
        inline_inlays: vec![Vec::new(); count],
        block_inlays: Vec::new(),
    };
    for (index, row) in metadata.rows.iter().enumerate() {
        if let Some(source) = &row.old {
            metadata.old_to_row[source.line as usize] = index;
        }
        if let Some(source) = &row.new {
            metadata.new_to_row[source.line as usize] = index;
        }
        let endpoint = row
            .new
            .as_ref()
            .map(|source| (&new_document, source))
            .or_else(|| row.old.as_ref().map(|source| (&old_document, source)));
        if let Some((document, source)) = endpoint {
            let line = source.line as usize;
            lines.push(document.as_text().as_lines()[line].clone());
            layout.tokens.push(document.layout().tokens[line].clone());
            layout
                .indent_state
                .push(document.layout().indent_state[line]);
        } else {
            lines.push(String::new());
            layout.tokens.push(Vec::new());
            layout
                .indent_state
                .push(Some(crate::document::IndentState::Empty(0)));
        }
    }
    let mut decorations = Vec::new();
    let mut index = 0;
    let (mut old_line, mut new_line) = (0, 0);
    while index < count {
        let row = &metadata.rows[index];
        if matches!(row.kind, None | Some(DiffRowKind::Equal)) {
            old_line += usize::from(row.old.is_some());
            new_line += usize::from(row.new.is_some());
            index += 1;
            continue;
        }
        let start = index;
        let id = row.hunk.unwrap();
        let (old_start, new_start) = (old_line, new_line);
        while index < count
            && metadata.rows[index].hunk == Some(id)
            && matches!(
                metadata.rows[index].kind,
                Some(DiffRowKind::Added | DiffRowKind::Removed)
            )
        {
            old_line += usize::from(metadata.rows[index].old.is_some());
            new_line += usize::from(metadata.rows[index].new.is_some());
            index += 1;
        }
        let removed = old_line - old_start;
        let added = new_line - new_start;
        metadata.hunks.push(DiffHunk {
            id,
            rows: start..index,
            old_lines: old_start..old_line,
            new_lines: new_start..new_line,
            added,
            removed,
        });
        for row in &mut metadata.rows[start..index] {
            row.gutter_mark = Some(if added > 0 && removed > 0 {
                DecorationType::DiffChangedGutter
            } else if added > 0 {
                DecorationType::DiffAdded
            } else {
                DecorationType::DiffRemoved
            });
        }
    }
    index = 0;
    while index < count {
        let kind = metadata.rows[index].kind;
        let start = index;
        index += 1;
        while index < count && metadata.rows[index].kind == kind {
            index += 1;
        }
        let ty = match kind {
            Some(DiffRowKind::Added) => DecorationType::DiffAdded,
            Some(DiffRowKind::Removed) => DecorationType::DiffRemoved,
            _ => continue,
        };
        decorations.push(Decoration::new(
            decorations.len(),
            Position {
                line_index: start,
                byte_index: 0,
            },
            Position {
                line_index: index,
                byte_index: 0,
            },
            ty,
        ));
    }
    let mut set = DecorationSet::new();
    set.replace_prepared(decorations)
        .expect("ordered disjoint diff runs");
    Ok(PreparedDiffDocument {
        document: PreparedDocument::from_diff_rows(Text::from_display_lines(lines), layout),
        metadata,
        decorations: set,
    })
}
