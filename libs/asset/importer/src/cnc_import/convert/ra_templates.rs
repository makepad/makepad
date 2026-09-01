//! Embedded Red Alert template metadata used by the game-neutral RTS front.

use std::collections::BTreeMap;

pub(super) const TEXT: &str = include_str!("../ra-template-table.txt");

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TemplateDef {
    pub theater: String,
    pub id: u16,
    pub stem: String,
    pub width: u8,
    pub height: u8,
    pub classes: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct TemplateTable {
    by_key: BTreeMap<(String, u16), TemplateDef>,
}

impl TemplateTable {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut by_key = BTreeMap::new();
        for (line_no, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 6 {
                return Err(format!(
                    "RA template table line {}: expected 6 fields",
                    line_no + 1
                ));
            }
            let id = fields[1]
                .parse::<u16>()
                .map_err(|_| format!("RA template table line {}: bad id", line_no + 1))?;
            let width = fields[3]
                .parse::<u8>()
                .map_err(|_| format!("RA template table line {}: bad width", line_no + 1))?;
            let height = fields[4]
                .parse::<u8>()
                .map_err(|_| format!("RA template table line {}: bad height", line_no + 1))?;
            let class_count = usize::from(width)
                .checked_mul(usize::from(height))
                .ok_or_else(|| format!("RA template table line {}: dimensions", line_no + 1))?;
            if fields[5].len() != class_count {
                return Err(format!(
                    "RA template table line {}: class count",
                    line_no + 1
                ));
            }
            let def = TemplateDef {
                theater: fields[0].to_ascii_lowercase(),
                id,
                stem: fields[2].to_ascii_lowercase(),
                width,
                height,
                classes: fields[5].to_owned(),
            };
            if by_key
                .insert((def.theater.clone(), def.id), def)
                .is_some()
            {
                return Err(format!(
                    "RA template table line {}: duplicate theater/id",
                    line_no + 1
                ));
            }
        }
        Ok(Self { by_key })
    }

    pub fn get(&self, theater: &str, id: u16) -> Option<&TemplateDef> {
        self.by_key.get(&(theater.to_ascii_lowercase(), id))
    }

    pub fn stems(&self, theater: &str) -> impl Iterator<Item = &str> {
        let theater = theater.to_ascii_lowercase();
        self.by_key
            .values()
            .filter(move |def| def.theater == theater)
            .map(|def| def.stem.as_str())
    }
}
