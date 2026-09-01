use std::collections::BTreeMap;
use std::fmt;

pub const D2K_TEMPLATE_TABLE: &str = include_str!("d2k-template-table.txt");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D2kTemplate {
    pub id: u16,
    pub image: String,
    pub w: u16,
    pub h: u16,
    pub classes: Vec<u8>,
    pub frames: Vec<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum D2kTilesError {
    InvalidLine { line: usize, field: &'static str },
    DuplicateId(u16),
}

impl fmt::Display for D2kTilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLine { line, field } => {
                write!(f, "invalid Dune 2000 template {field} on line {line}")
            }
            Self::DuplicateId(id) => write!(f, "duplicate Dune 2000 template id {id}"),
        }
    }
}

impl std::error::Error for D2kTilesError {}

#[derive(Clone, Debug)]
pub struct D2kTemplateTable {
    templates: Vec<D2kTemplate>,
    by_id: BTreeMap<u16, usize>,
}

impl D2kTemplateTable {
    pub fn embedded() -> Result<Self, D2kTilesError> {
        Self::parse(D2K_TEMPLATE_TABLE)
    }

    pub fn parse(source: &str) -> Result<Self, D2kTilesError> {
        let mut templates = Vec::new();
        let mut by_id = BTreeMap::new();
        for (line_index, source_line) in source.lines().enumerate() {
            let line = line_index + 1;
            let columns: Vec<_> = source_line.split_whitespace().collect();
            if columns.is_empty() || columns[0].starts_with('#') {
                continue;
            }
            if columns.len() < 3 {
                return Err(D2kTilesError::InvalidLine {
                    line,
                    field: "row",
                });
            }
            if columns[2].eq_ignore_ascii_case("customtiles") {
                continue;
            }

            let id = parse_u16(columns.get(1), line, "id")?;
            let image = columns[2].to_owned();
            let (w, h, classes, frame_source) = match columns.as_slice() {
                [_, _, _, w, h, classes, frames] => (
                    parse_value(w, line, "width")?,
                    parse_value(h, line, "height")?,
                    classes.as_bytes().to_vec(),
                    *frames,
                ),
                // Six source rows omit both height and class. Retain them so
                // callers can see and report the source-table exception.
                [_, _, _, w, frames] => (
                    parse_value(w, line, "width")?,
                    0,
                    Vec::new(),
                    *frames,
                ),
                _ => {
                    return Err(D2kTilesError::InvalidLine {
                        line,
                        field: "row",
                    })
                }
            };
            let mut frames = Vec::new();
            for frame in frame_source.split(',') {
                frames.push(
                    frame
                        .parse()
                        .map_err(|_| D2kTilesError::InvalidLine {
                            line,
                            field: "frame",
                        })?,
                );
            }
            let index = templates.len();
            if by_id.insert(id, index).is_some() {
                return Err(D2kTilesError::DuplicateId(id));
            }
            templates.push(D2kTemplate {
                id,
                image,
                w,
                h,
                classes,
                frames,
            });
        }
        Ok(Self { templates, by_id })
    }

    pub fn templates(&self) -> &[D2kTemplate] {
        &self.templates
    }

    pub fn by_id(&self, id: u16) -> Option<&D2kTemplate> {
        self.by_id.get(&id).map(|&index| &self.templates[index])
    }
}

fn parse_u16(
    value: Option<&&str>,
    line: usize,
    field: &'static str,
) -> Result<u16, D2kTilesError> {
    value
        .ok_or(D2kTilesError::InvalidLine { line, field })?
        .parse()
        .map_err(|_| D2kTilesError::InvalidLine { line, field })
}

fn parse_value(value: &str, line: usize, field: &'static str) -> Result<u16, D2kTilesError> {
    value
        .parse()
        .map_err(|_| D2kTilesError::InvalidLine { line, field })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_d2k_template_table_parses() {
        let table = D2kTemplateTable::embedded().unwrap();
        assert_eq!(table.templates().len(), 476);
        assert_eq!(table.by_id(0).unwrap().image, "BLOXBASE");
        assert!(table.by_id(500).is_none(), "customtiles rows are ignored");

        let mut dimension_exceptions = Vec::new();
        for template in table.templates() {
            assert!(template.frames.iter().all(|&frame| frame < 800));
            let expected = usize::from(template.w) * usize::from(template.h);
            if template.frames.len() != expected {
                dimension_exceptions.push((template.id, expected, template.frames.len()));
            }
        }
        eprintln!("D2K template dimension exceptions: {dimension_exceptions:?}");
    }
}
