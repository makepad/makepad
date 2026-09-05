//! Data-only catalog query rendering for clients without a local SQL store.

#[derive(Clone, Debug)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

impl QueryOutput {
    pub fn to_text(&self) -> String {
        let mut widths: Vec<usize> = self.columns.iter().map(|c| c.chars().count()).collect();
        for row in &self.rows {
            for (index, value) in row.iter().enumerate() {
                if index < widths.len() {
                    widths[index] = widths[index].max(value.chars().count());
                }
            }
        }
        const ALIGN_CAP: usize = 40;
        let pad = |text: &str, width: usize, last: bool| -> String {
            let width = width.min(ALIGN_CAP);
            let len = text.chars().count();
            if last || len >= width {
                text.to_string()
            } else {
                format!("{text}{:width$}", "", width = width - len)
            }
        };
        let mut out = String::new();
        let column_count = self.columns.len();
        for (index, column) in self.columns.iter().enumerate() {
            if index > 0 {
                out.push_str("  ");
            }
            out.push_str(&pad(column, widths[index], index + 1 == column_count));
        }
        out.push('\n');
        for row in &self.rows {
            for (index, value) in row.iter().enumerate() {
                if index > 0 {
                    out.push_str("  ");
                }
                out.push_str(&pad(value, widths[index], index + 1 == column_count));
            }
            out.push('\n');
        }
        if self.truncated {
            out.push_str(&format!(
                "({} rows shown, MORE EXIST — narrow with WHERE or LIMIT)\n",
                self.rows.len()
            ));
        } else {
            out.push_str(&format!("({} rows)\n", self.rows.len()));
        }
        out
    }
}

pub const SCHEMA_NOTES: &str = "\nNotes:\n\
- search_annotations is the main listing: one row per asset with canon_alias, kind, title, description, prompt, live. Always filter live=1.\n\
- search_labels holds per-asset category/tag labels. Join on asset_id.\n\
- asset_aliases maps alias to asset_id/head_revision.\n";
