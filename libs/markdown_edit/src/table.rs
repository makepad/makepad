//! GFM table commands over source lines, with parser events defining the table's extent.

use crate::plan::{content_start, line_at, Containers, Leaf, Plan};
use pulldown_cmark::{Alignment, Event, Tag, TagEnd};
use std::ops::Range;

/// A table cell: its place and its content's source range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Zero for the header, then the body rows.
    pub row: usize,
    pub column: usize,
    /// Header plus body rows.
    pub rows: usize,
    /// The header's cell count.
    pub columns: usize,
    pub content: Range<usize>,
}

/// Whole cells of one table: what a selection from one of its cells to
/// another covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellBlock {
    /// A place in the table (its header's start), which finds it again.
    pub table: usize,
    /// Zero for the header, then the body rows.
    pub rows: Range<usize>,
    pub columns: Range<usize>,
    /// The table's header plus body rows, and its columns.
    pub table_rows: usize,
    pub table_columns: usize,
}

impl CellBlock {
    /// Whether the block holds every column of its rows.
    pub fn whole_rows(&self) -> bool {
        self.columns.start == 0 && self.columns.end >= self.table_columns
    }
}

/// An optional source edit, then a selection in the resulting text.
/// An empty selection is a caret; a plan's caret is the selection's end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub plan: Option<Plan>,
    pub select: Range<usize>,
}

type Edits = Vec<(Range<usize>, String)>;

#[derive(Clone, Copy)]
struct Style {
    leading: bool,
    trailing: bool,
}

struct Row {
    line: Range<usize>,
    source: Range<usize>,
    raw: Vec<Range<usize>>,
    pipes: Vec<usize>,
    style: Style,
}

impl Row {
    fn read(text: &str, line: Range<usize>, start: usize) -> Self {
        let source = start..line.end;
        let mut pipes = Vec::new();
        let mut escaped = false;
        // Backticks do not protect pipes in GFM tables; backslashes do.
        for (offset, byte) in text[source.clone()].bytes().enumerate() {
            if byte == b'|' && !escaped {
                pipes.push(start + offset);
            }
            escaped = byte == b'\\' && !escaped;
        }
        let leading = pipes.first().is_some_and(|&pipe| text[start..pipe].trim_matches(' ').is_empty());
        let trailing = pipes.last().is_some_and(|&pipe| text[pipe + 1..line.end].trim_matches(' ').is_empty());
        let mut raw = Vec::new();
        let mut from = start;
        for (index, &pipe) in pipes.iter().enumerate() {
            if index != 0 || !leading {
                raw.push(from..pipe);
            }
            from = pipe + 1;
        }
        if !trailing || raw.is_empty() {
            raw.push(from..line.end);
        }
        Self { line, source, raw, pipes, style: Style { leading, trailing } }
    }

    fn content(&self, text: &str, column: usize) -> Range<usize> {
        let Some(raw) = self.raw.get(column) else { return self.line.end..self.line.end };
        let value = &text[raw.clone()];
        let trimmed = value.trim_matches(' ');
        if trimmed.is_empty() {
            let at = raw.start + usize::from(value.starts_with(' '));
            at..at
        } else {
            let start = raw.start + value.len() - value.trim_start_matches(' ').len();
            start..start + trimmed.len()
        }
    }

    fn column_at(&self, at: usize, columns: usize) -> Option<usize> {
        let start = if self.style.leading { self.pipes[0] } else { self.source.start };
        if at < start || at > self.line.end {
            return None;
        }
        if at == self.line.end {
            return Some(columns - 1);
        }
        let passed = self.pipes.partition_point(|&pipe| pipe <= at);
        Some(passed.saturating_sub(usize::from(self.style.leading)).min(columns - 1))
    }

    fn values<'a>(&self, text: &'a str, columns: usize) -> Vec<&'a str> {
        (0..columns).map(|column| &text[self.content(text, column)]).collect()
    }

    fn write(&self, text: &str, columns: usize, values: &[&str], style: Style) -> Written {
        let extra = self.raw.len() > columns;
        let mut written = write_row(values, Style { leading: style.leading, trailing: style.trailing && !extra });
        if extra {
            // Keep every byte after the last visible cell, including the
            // separator and the hidden cells' own spacing and outer pipe.
            written.text.push(' ');
            written.text.push_str(&text[self.raw[columns - 1].end..self.line.end]);
        }
        written
    }
}

struct Written {
    text: String,
    cells: Vec<Range<usize>>,
}

fn write_row(values: &[&str], style: Style) -> Written {
    let mut text = String::new();
    let mut cells = Vec::new();
    if style.leading {
        text.push_str("| ");
    }
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            text.push_str(" | ");
        }
        let start = text.len();
        text.push_str(value);
        cells.push(start..text.len());
    }
    if values.is_empty() {
        // Deleting the only written cell of a short row leaves an empty
        // row. Its first cell still has a typing position between the pipes.
        cells.push(text.len()..text.len());
    }
    if style.trailing {
        text.push_str(" |");
    }
    Written { text, cells }
}

struct Table {
    rows: Vec<Row>,
    delimiter: Row,
    columns: usize,
    aligns: Vec<Alignment>,
    containers: Containers,
}

impl Table {
    fn at(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<Self> {
        debug_assert!(text.is_char_boundary(at));
        let (index, aligns, start) = events.iter().enumerate().find_map(|(index, (event, range))| {
            let Event::Start(Tag::Table(aligns)) = event else { return None };
            (line_at(text, range.start).start <= at && at < range.end)
                .then(|| (index, aligns.clone(), range.start))
        })?;
        let columns = aligns.len();
        if columns == 0 {
            return None;
        }
        let containers = Containers::at(text, events, start);
        let mut rows = Vec::new();
        for (event, range) in &events[index + 1..] {
            match event {
                Event::Start(Tag::TableHead | Tag::TableRow) => {
                    let line = line_at(text, range.start);
                    let start = content_start(text, line.clone(), &containers.stack);
                    rows.push(Row::read(text, line, start));
                }
                Event::End(TagEnd::Table) => break,
                _ => {}
            }
        }
        let header = rows.first()?;
        let line = line_at(text, after_line(text, &header.line));
        let start = content_start(text, line.clone(), &containers.stack);
        let delimiter = Row::read(text, line, start);
        Some(Self { rows, delimiter, columns, aligns, containers })
    }

    fn locate(&self, at: usize) -> Option<(usize, usize)> {
        self.rows.iter().enumerate().find_map(|(index, row)| {
            row.column_at(at, self.columns).map(|column| (index, column))
        })
    }

    fn style(&self) -> Style {
        self.rows[0].style
    }

    fn end(&self, text: &str) -> usize {
        let last = self.rows.last().filter(|_| self.rows.len() > 1).unwrap_or(&self.delimiter);
        after_line(text, &last.line)
    }
}

fn after_line(text: &str, line: &Range<usize>) -> usize {
    (line.end + 1).min(text.len())
}

enum Destination {
    Existing(Range<usize>),
    /// An edit's old start, and a range relative to its replacement.
    Written(usize, Range<usize>),
}

#[derive(Clone, Copy)]
enum Placement {
    Select,
    Start,
    End,
}

fn finish(text: &str, mut edits: Edits, destination: Destination, placement: Placement) -> Step {
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut plan = Plan { edits, caret: 0 };
    let mut select = match destination {
        Destination::Existing(range) => plan.map(range.start)..plan.map(range.end),
        Destination::Written(at, range) => {
            let replacement = &plan.edits.iter().find(|(source, _)| source.start == at)
                .expect("destination belongs to an edit").1;
            let start = plan.map(at) - replacement.len();
            start + range.start..start + range.end
        }
    };
    match placement {
        Placement::Select => {}
        Placement::Start => select.end = select.start,
        Placement::End => select.start = select.end,
    }
    plan.caret = select.end;
    Step { plan: (!plan.edits.is_empty()).then(|| plan.checked(text)), select }
}

fn go_to(
    text: &str, table: &Table, row: usize, column: usize, mut edits: Edits, placement: Placement,
) -> Step {
    let row = &table.rows[row];
    let destination = if row.raw.len() < table.columns {
        let written = row.write(text, table.columns, &row.values(text, table.columns), table.style());
        let destination = Destination::Written(row.source.start, written.cells[column].clone());
        edits.push((row.source.clone(), written.text));
        destination
    } else {
        Destination::Existing(row.content(text, column))
    };
    finish(text, edits, destination, placement)
}

fn add_row(text: &str, table: &Table, row: usize, column: usize, placement: Placement) -> Step {
    let at = table.rows.get(row).map_or_else(|| table.end(text), |row| row.line.start);
    let prefix = table.containers.continuation();
    let written = write_row(&vec![""; table.columns], table.style());
    let cell = &written.cells[column];
    let destination = Destination::Written(at, prefix.len() + cell.start..prefix.len() + cell.end);
    let inserted = prefix + &written.text + "\n";
    finish(text, vec![(at..at, inserted)], destination, placement)
}

/// The cell holding `at`, including its surrounding spaces. Pipes belong to
/// the cell after them; a line end belongs to the last column. Delimiters do not hold cells.
pub fn cell_at(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<Cell> {
    let table = Table::at(text, events, at)?;
    let (row, column) = table.locate(at)?;
    Some(Cell {
        row, column, rows: table.rows.len(), columns: table.columns,
        content: table.rows[row].content(text, column),
    })
}

// Tight lists omit Paragraph events. Stop their initial paragraph at the
// first block child instead of using the whole item's range (which can
// include a nested list, code block or table).
fn paragraph(text: &str, events: &[(Event<'_>, Range<usize>)], containers: &Containers) -> Option<Range<usize>> {
    if containers.leaf != Leaf::Paragraph {
        return None;
    }
    let range = &containers.leaf_range;
    let item = events.iter().position(|(event, source)| matches!(event, Event::Start(Tag::Item)) && source == range);
    let end = if let Some(index) = item {
        let mut depth = 0usize;
        let mut end = range.start;
        for (event, source) in &events[index + 1..] {
            match event {
                Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }) => depth += 1,
                Event::Start(_) => break,
                Event::End(_) if depth == 0 => break,
                Event::End(_) => depth -= 1,
                _ => {}
            }
            end = end.max(source.end);
        }
        end
    } else {
        range.end
    };
    let last = text[..end].char_indices().next_back().map_or(range.start, |(at, _)| at);
    Some(line_at(text, range.start).start..after_line(text, &line_at(text, last)))
}

/// Inserts a table after the caret's leaf, or in place of its blank line.
/// The table stays in the same containers and selects its first header cell.
/// Returns `None` in tables or code, or for zero columns.
pub fn insert_table(
    text: &str, events: &[(Event<'_>, Range<usize>)], at: usize, columns: usize, rows: usize,
) -> Option<Step> {
    let containers = Containers::at(text, events, at);
    if columns == 0 || matches!(containers.leaf, Leaf::Table | Leaf::Code { .. }) {
        return None;
    }
    let start = containers.content_start(text);
    let blank = text[start..containers.line.end].trim_matches([' ', '\t']).is_empty();
    let prefix = containers.continuation();
    let (range, mut inserted) = if blank {
        let separate = start == text.len() && start > 0
            && !text[line_at(text, start - 1)].trim_matches([' ', '\t']).is_empty();
        (start..after_line(text, &containers.line), if separate { "\n".to_string() } else { String::new() })
    } else {
        let end = paragraph(text, events, &containers).map_or_else(|| {
            let last = text[..containers.leaf_range.end].char_indices().next_back().map_or(at, |(at, _)| at);
            after_line(text, &line_at(text, last))
        }, |range| range.end);
        (end..end, containers.blank_continuation() + "\n" + &prefix)
    };
    let style = Style { leading: true, trailing: true };
    let labels: Vec<_> = (1..=columns).map(|column| format!("Column {column}")).collect();
    let header = write_row(&labels.iter().map(String::as_str).collect::<Vec<_>>(), style);
    let first = &header.cells[0];
    let select = inserted.len() + first.start..inserted.len() + first.end;
    inserted.push_str(&header.text);
    inserted.push('\n');
    let delimiter = write_row(&vec!["---"; columns], style);
    inserted.push_str(&prefix);
    inserted.push_str(&delimiter.text);
    inserted.push('\n');
    let empty = write_row(&vec![""; columns], style);
    for _ in 0..rows {
        inserted.push_str(&prefix);
        inserted.push_str(&empty.text);
        inserted.push('\n');
    }
    // A following paragraph must not become another body row. Reuse a
    // written blank line when there is one, otherwise separate it here.
    if range.end < text.len() {
        let line = line_at(text, range.end);
        let start = content_start(text, line.clone(), &containers.stack);
        if !text[start..line.end].trim_matches([' ', '\t']).is_empty() {
            inserted.push_str(&containers.blank_continuation());
            inserted.push('\n');
        }
    }
    let destination = Destination::Written(range.start, select);
    Some(finish(text, vec![(range, inserted)], destination, Placement::Select))
}

/// Tab or Shift+Tab selects the next or previous cell in row order.
/// Tab at the last cell appends an empty body row; Shift+Tab at the first is inert.
pub fn table_step(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize, back: bool) -> Option<Step> {
    let table = Table::at(text, events, at)?;
    let (row, column) = table.locate(at)?;
    let (row, column) = if back {
        if column > 0 { (row, column - 1) } else { (row.checked_sub(1)?, table.columns - 1) }
    } else if column + 1 < table.columns {
        (row, column + 1)
    } else {
        (row + 1, 0)
    };
    Some(if row == table.rows.len() {
        add_row(text, &table, row, column, Placement::Select)
    } else {
        go_to(text, &table, row, column, Vec::new(), Placement::Select)
    })
}

/// Enter moves to the end of the cell below, appending a body row when needed.
pub fn cell_below(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<Step> {
    let table = Table::at(text, events, at)?;
    let (row, column) = table.locate(at)?;
    Some(if row + 1 == table.rows.len() {
        add_row(text, &table, row + 1, column, Placement::End)
    } else {
        go_to(text, &table, row + 1, column, Vec::new(), Placement::End)
    })
}

/// Adds an empty body row above or below the caret's row, retaining its column.
/// Inserting above the header is inert.
pub fn insert_row(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize, above: bool) -> Option<Step> {
    let table = Table::at(text, events, at)?;
    let (row, column) = table.locate(at)?;
    if above && row == 0 {
        return None;
    }
    Some(add_row(text, &table, row + usize::from(!above), column, Placement::Start))
}

/// Removes a body row and moves to the same column in the next row, else the previous.
/// Removing the header is inert.
pub fn delete_row(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<Step> {
    let table = Table::at(text, events, at)?;
    let (row, column) = table.locate(at)?;
    if row == 0 {
        return None;
    }
    let source = &table.rows[row];
    let edits = vec![(source.line.start..after_line(text, &source.line), String::new())];
    let next = if row + 1 < table.rows.len() { row + 1 } else { row - 1 };
    Some(go_to(text, &table, next, column, edits, Placement::Start))
}

/// Inserts an empty column left or right of the current one in every row.
/// Short body rows are first padded to the old header width.
pub fn insert_column(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize, left: bool) -> Option<Step> {
    let table = Table::at(text, events, at)?;
    let (selected_row, column) = table.locate(at)?;
    let column = column + usize::from(!left);
    let mut edits = Vec::new();
    let mut destination = None;
    for (index, row) in table.rows.iter().enumerate() {
        let mut values = row.values(text, table.columns);
        values.insert(column, "");
        let written = row.write(text, table.columns, &values, table.style());
        if index == selected_row {
            destination = Some(Destination::Written(row.source.start, written.cells[column].clone()));
        }
        edits.push((row.source.clone(), written.text));
    }
    let row = &table.delimiter;
    let mut values = row.values(text, table.columns);
    values.insert(column, "---");
    let written = row.write(text, table.columns, &values, table.style());
    edits.push((row.source.clone(), written.text));
    Some(finish(text, edits, destination?, Placement::Start))
}

/// Removes the current column from every row that reaches it, keeping hidden extras.
/// Removing the only column removes the table; otherwise the caret goes right, else left.
pub fn delete_column(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<Step> {
    let table = Table::at(text, events, at)?;
    let (selected_row, column) = table.locate(at)?;
    if table.columns == 1 {
        let range = table.rows[0].line.start..table.end(text);
        return Some(finish(text, vec![(range, String::new())], Destination::Existing(at..at), Placement::Start));
    }
    let next = column.min(table.columns - 2);
    let mut edits = Vec::new();
    let mut destination = Destination::Existing(table.rows[selected_row].content(text, next));
    for (index, row) in table.rows.iter().enumerate().chain(std::iter::once((table.rows.len(), &table.delimiter))) {
        if row.raw.len() <= column {
            continue;
        }
        let mut values = row.values(text, table.columns.min(row.raw.len()));
        values.remove(column);
        let written = row.write(text, table.columns, &values, table.style());
        if index == selected_row {
            let content = written.cells.get(next).cloned().unwrap_or(written.text.len()..written.text.len());
            destination = Destination::Written(row.source.start, content);
        }
        edits.push((row.source.clone(), written.text));
    }
    Some(finish(text, edits, destination, Placement::Start))
}

/// The cells from `anchor`'s to `caret`'s when the two are in different cells
/// of one table. None within one cell, or with either outside the table's
/// rows (its delimiter line included).
pub fn cell_block(text: &str, events: &[(Event<'_>, Range<usize>)], anchor: usize, caret: usize) -> Option<CellBlock> {
    let table = Table::at(text, events, anchor)?;
    let from = table.locate(anchor)?;
    let to = table.locate(caret)?;
    if from == to {
        return None;
    }
    Some(CellBlock {
        table: table.rows[0].source.start,
        rows: from.0.min(to.0)..from.0.max(to.0) + 1,
        columns: from.1.min(to.1)..from.1.max(to.1) + 1,
        table_rows: table.rows.len(),
        table_columns: table.columns,
    })
}

/// The block's cells as a table of their own: its first row the header,
/// under it the table's alignments for those columns.
pub fn block_text(text: &str, events: &[(Event<'_>, Range<usize>)], block: &CellBlock) -> Option<String> {
    let table = Table::at(text, events, block.table)?;
    let style = Style { leading: true, trailing: true };
    let columns = block.columns.start.min(table.columns)..block.columns.end.min(table.columns);
    let mut out = String::new();
    for (index, row) in block.rows.clone().enumerate() {
        let values = table.rows.get(row)?.values(text, table.columns);
        out.push_str(&write_row(&values[columns.clone()], style).text);
        out.push('\n');
        if index == 0 {
            let marks: Vec<&str> = columns
                .clone()
                .map(|column| match table.aligns.get(column) {
                    Some(Alignment::Left) => ":--",
                    Some(Alignment::Center) => ":-:",
                    Some(Alignment::Right) => "--:",
                    _ => "---",
                })
                .collect();
            out.push_str(&write_row(&marks, style).text);
            out.push('\n');
        }
    }
    Some(out)
}

/// Empties the block's cells, keeping the table's shape and spacing; the
/// caret goes to the first cell.
pub fn clear_block(text: &str, events: &[(Event<'_>, Range<usize>)], block: &CellBlock) -> Option<Step> {
    let table = Table::at(text, events, block.table)?;
    let first = table.rows.get(block.rows.start)?.content(text, block.columns.start);
    let mut edits = Vec::new();
    for row in block.rows.clone() {
        let source = table.rows.get(row)?;
        for column in block.columns.clone().filter(|&column| column < source.raw.len()) {
            let content = source.content(text, column);
            if !content.is_empty() {
                edits.push((content, String::new()));
            }
        }
    }
    Some(finish(text, edits, Destination::Existing(first), Placement::Start))
}

/// Deletes the block: every row and column takes the table, whole body rows
/// go (the caret to the row after, else before), whole columns go from every
/// row; any other block is emptied ([`clear_block`]).
pub fn delete_block(text: &str, events: &[(Event<'_>, Range<usize>)], block: &CellBlock) -> Option<Step> {
    let table = Table::at(text, events, block.table)?;
    let all_rows = block.rows.start == 0 && block.rows.end >= table.rows.len();
    let all_columns = block.columns.start == 0 && block.columns.end >= table.columns;
    if all_rows && all_columns {
        let start = table.rows[0].line.start;
        let edits = vec![(start..table.end(text), String::new())];
        return Some(finish(text, edits, Destination::Existing(start..start), Placement::Start));
    }
    if all_columns && block.rows.start > 0 {
        let first = &table.rows[block.rows.start];
        let last = &table.rows[(block.rows.end - 1).min(table.rows.len() - 1)];
        let edits = vec![(first.line.start..after_line(text, &last.line), String::new())];
        let next = if block.rows.end < table.rows.len() { block.rows.end } else { block.rows.start - 1 };
        return Some(go_to(text, &table, next, 0, edits, Placement::Start));
    }
    if all_rows {
        let next = block.columns.start.min(table.columns - block.columns.len() - 1);
        let mut edits = Vec::new();
        let mut destination = None;
        for (index, row) in table.rows.iter().enumerate().chain(std::iter::once((table.rows.len(), &table.delimiter))) {
            let mut values = row.values(text, table.columns.min(row.raw.len()));
            let len = values.len();
            if block.columns.start >= len {
                continue;
            }
            values.drain(block.columns.start..block.columns.end.min(len));
            let written = row.write(text, table.columns, &values, table.style());
            if index == 0 {
                let content = written.cells.get(next).cloned().unwrap_or(written.text.len()..written.text.len());
                destination = Some(Destination::Written(row.source.start, content));
            }
            edits.push((row.source.clone(), written.text));
        }
        return Some(finish(text, edits, destination?, Placement::Start));
    }
    clear_block(text, events, block)
}

/// Enter at the end of a one-line pipe paragraph adds its delimiter and an empty body row.
/// The header must start with a pipe and have at least two cells.
pub fn table_from_header(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<Step> {
    let containers = Containers::at(text, events, at);
    if at != containers.line.end || paragraph(text, events, &containers)? != (containers.line.start..after_line(text, &containers.line)) {
        return None;
    }
    let row = Row::read(text, containers.line.clone(), containers.content_start(text));
    if !row.style.leading || row.raw.len() < 2 {
        return None;
    }
    let prefix = containers.continuation();
    let delimiter = write_row(&vec!["---"; row.raw.len()], row.style);
    let empty = write_row(&vec![""; row.raw.len()], row.style);
    let mut inserted = prefix.clone() + &delimiter.text + "\n" + &prefix;
    let cell = &empty.cells[0];
    let content = inserted.len() + cell.start..inserted.len() + cell.end;
    inserted.push_str(&empty.text);
    inserted.push('\n');
    let at = after_line(text, &containers.line);
    Some(finish(text, vec![(at..at, inserted)], Destination::Written(at, content), Placement::Start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_options;
    use pulldown_cmark::Parser;

    /// Run every case, reporting all broken names even when a command itself panics.
    fn all<T>(cases: impl IntoIterator<Item = (&'static str, T)>, check: impl Fn(&str, T)) {
        let mut failed = Vec::new();
        for (rule, case) in cases {
            if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(rule, case))) {
                let message = panic.downcast_ref::<String>().cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                failed.push(format!("{rule}: {message}"));
            }
        }
        assert!(failed.is_empty(), "{} cases broke:\n{}", failed.len(), failed.join("\n---\n"));
    }

    fn selection(marked: &str) -> (String, Range<usize>) {
        if let Some(at) = marked.find('‸') {
            (marked.replace('‸', ""), at..at)
        } else {
            let start = marked.find('‹').expect("case has a selection start");
            let end = marked.find('›').expect("case has a selection end") - '‹'.len_utf8();
            (marked.replace(['‹', '›'], ""), start..end)
        }
    }

    fn step_case(
        rule: &str, before: &str, expected: Option<&str>,
        command: impl FnOnce(&str, &[(Event<'_>, Range<usize>)], usize) -> Option<Step>,
    ) {
        let (text, caret) = selection(before);
        assert!(text.ends_with('\n') || text.is_empty(), "{rule}: whole source lines");
        assert!(caret.is_empty(), "{rule}: input is a caret");
        let mut parser = Parser::new_ext(&text, parse_options());
        parser.assume_footnote_labels_defined();
        let events: Vec<_> = parser.into_offset_iter().collect();
        let step = command(&text, &events, caret.start);
        let Some(expected) = expected else {
            assert!(step.is_none(), "{rule}: inert command\nevents: {events:?}\nstep: {step:?}");
            return;
        };
        let step = step.unwrap_or_else(|| panic!("{rule}: expected a step\nevents: {events:?}"));
        let mut actual = text.clone();
        if let Some(plan) = &step.plan {
            assert!(!plan.edits.is_empty(), "{rule}: movement alone needs no plan");
            let mut end = 0;
            for (range, _) in &plan.edits {
                assert!(end <= range.start && range.start <= range.end && range.end <= text.len(),
                    "{rule}: source edits are ordered and disjoint");
                assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end),
                    "{rule}: source edits land on character boundaries");
                end = range.end;
            }
            actual = plan.apply(&text);
            assert_eq!(plan.caret, step.select.end, "{rule}: plan caret is the selection end");
            let mut previous = 0;
            for offset in text.char_indices().map(|(at, _)| at).chain([text.len()]) {
                let mapped = plan.map(offset);
                assert!(mapped >= previous && actual.is_char_boundary(mapped), "{rule}: valid ordered map");
                previous = mapped;
            }
        }
        assert!(step.select.start <= step.select.end && actual.is_char_boundary(step.select.start)
            && actual.is_char_boundary(step.select.end), "{rule}: valid selection");
        let (expected_text, expected_select) = selection(expected);
        assert_eq!(actual, expected_text, "{rule}: source\nevents: {events:?}");
        assert_eq!(step.select, expected_select, "{rule}: selection\nevents: {events:?}");
        if actual == text {
            assert!(step.plan.is_none(), "{rule}: unchanged source needs no plan");
        }
    }

    #[test]
    fn cells_in_source_rows() {
        all([
            ("header", ("| a‸ | b |\n| --- | --- |\n| c | d |\n", Some((0, 0, 2, 2, "| ‹a› | b |\n| --- | --- |\n| c | d |\n")))),
            ("body", ("| a | b |\n| --- | --- |\n| c | d‸ |\n", Some((1, 1, 2, 2, "| a | b |\n| --- | --- |\n| c | ‹d› |\n")))),
            ("separator belongs right", ("| a ‸| b |\n| --- | --- |\n", Some((0, 1, 1, 2, "| a | ‹b› |\n| --- | --- |\n")))),
            ("opening pipe", ("‸| a | b |\n| --- | --- |\n", Some((0, 0, 1, 2, "| ‹a› | b |\n| --- | --- |\n")))),
            ("line end", ("| a | b |‸\n| --- | --- |\n", Some((0, 1, 1, 2, "| a | ‹b› |\n| --- | --- |\n")))),
            ("empty padded cell", ("| a | b |\n| --- | --- |\n|‸  | d |\n", Some((1, 0, 2, 2, "| a | b |\n| --- | --- |\n| ‸ | d |\n")))),
            ("empty compact cell", ("| a | b |\n| --- | --- |\n‸|| d |\n", Some((1, 0, 2, 2, "| a | b |\n| --- | --- |\n|‸| d |\n")))),
            ("short row missing last", ("| a | b |\n| --- | --- |\n| c |‸\n", Some((1, 1, 2, 2, "| a | b |\n| --- | --- |\n| c |‸\n")))),
            ("no outer pipes", ("a | b\n--- | ---\nc‸ | d\n", Some((1, 0, 2, 2, "a | b\n--- | ---\n‹c› | d\n")))),
            ("delimiter", ("| a | b |\n| --‸- | --- |\n", None)),
            ("quote", ("> | a‸ |\n> | --- |\n", Some((0, 0, 1, 1, "> | ‹a› |\n> | --- |\n")))),
            ("quote prefix is outside", ("‸> | a |\n> | --- |\n", None)),
            ("escaped code pipe", ("| `a\\|‸b` | é |\n| --- | --- |\n", Some((0, 0, 1, 2, "| ‹`a\\|b`› | é |\n| --- | --- |\n")))),
            ("extra source cells clamp to last visible", ("| a | b |\n| --- | --- |\n| c | d | hidden‸ |\n", Some((1, 1, 2, 2, "| a | b |\n| --- | --- |\n| c | ‹d› | hidden |\n")))),
            ("paragraph", ("a‸ | b\n", None)),
        ], |rule, (before, expected)| {
            let (text, caret) = selection(before);
            let events: Vec<_> = Parser::new_ext(&text, parse_options()).into_offset_iter().collect();
            let expected = expected.map(|(row, column, rows, columns, marked)| {
                let (same, content) = selection(marked);
                assert_eq!(same, text, "{rule}: marked content is in the same source");
                Cell { row, column, rows, columns, content }
            });
            assert_eq!(cell_at(&text, &events, caret.start), expected, "{rule}\nevents: {events:?}");
        });
    }

    #[test]
    fn tables_insert_at_leaves() {
        all([
            ("blank between paragraphs", ("one\n\n‸\n\ntwo\n", Some("one\n\n| ‹Column 1› | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |\n\ntwo\n"), 2, 2)),
            ("after paragraph", ("o‸ne\n\ntwo\n", Some("one\n\n| ‹Column 1› | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |\n\ntwo\n"), 2, 2)),
            ("last paragraph", ("one‸\n", Some("one\n\n| ‹Column 1› | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |\n"), 2, 2)),
            ("EOF caret", ("one\n\n‸", Some("one\n\n| ‹Column 1› | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |\n"), 2, 2)),
            ("EOF after paragraph newline", ("one\n‸", Some("one\n\n| ‹Column 1› | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |\n"), 2, 2)),
            ("empty document", ("‸", Some("| ‹Column 1› | Column 2 |\n| --- | --- |\n|  |  |\n|  |  |\n"), 2, 2)),
            ("inside quote", ("> one‸\n", Some("> one\n>\n> | ‹Column 1› | Column 2 |\n> | --- | --- |\n> |  |  |\n> |  |  |\n"), 2, 2)),
            ("blank quoted line", ("> ‸\n", Some("> | ‹Column 1› | Column 2 |\n> | --- | --- |\n> |  |  |\n> |  |  |\n"), 2, 2)),
            ("list paragraph before child", ("- one‸\n  - child\n", Some("- one\n\n  | ‹Column 1› | Column 2 |\n  | --- | --- |\n  |  |  |\n  |  |  |\n\n  - child\n"), 2, 2)),
            ("empty item", ("- ‸\n", Some("- | ‹Column 1› | Column 2 |\n  | --- | --- |\n  |  |  |\n  |  |  |\n"), 2, 2)),
            ("header only", ("‸\n", Some("| ‹Column 1› |\n| --- |\n"), 1, 0)),
            ("inside table", ("| a‸ |\n| --- |\n", None, 2, 2)),
            ("inside code", ("```\nx‸\n```\n", None, 2, 2)),
            ("zero columns", ("‸\n", None, 0, 2)),
        ], |rule, (before, expected, columns, rows)| {
            step_case(rule, before, expected, |text, events, at| insert_table(text, events, at, columns, rows));
        });
    }

    #[test]
    fn stepping_and_below() {
        #[derive(Clone, Copy)]
        enum Command { Next, Back, Below }
        all([
            ("next cell", ("| a‸ | bé |\n| --- | --- |\n", Some("| a | ‹bé› |\n| --- | --- |\n"), Command::Next)),
            ("next row", ("| a | b‸ |\n| --- | --- |\n| c | d |\n", Some("| a | b |\n| --- | --- |\n| ‹c› | d |\n"), Command::Next)),
            ("back to header", ("| a | b |\n| --- | --- |\n| c‸ | d |\n", Some("| a | ‹b› |\n| --- | --- |\n| c | d |\n"), Command::Back)),
            ("append after last cell", ("| a | b |\n| --- | --- |\n| c | d‸ |\n", Some("| a | b |\n| --- | --- |\n| c | d |\n| ‸ |  |\n"), Command::Next)),
            ("header only appends after delimiter", ("| a | b‸ |\n| --- | --- |\n", Some("| a | b |\n| --- | --- |\n| ‸ |  |\n"), Command::Next)),
            ("pad short destination row", ("| a | b‸ |\n| --- | --- |\n| c |\n", Some("| a | b |\n| --- | --- |\n| ‹c› |  |\n"), Command::Next)),
            ("step into missing cell", ("| a | b |\n| --- | --- |\n| c‸ |\n", Some("| a | b |\n| --- | --- |\n| c | ‸ |\n"), Command::Next)),
            ("first cell backwards", ("| a‸ | b |\n| --- | --- |\n", None, Command::Back)),
            ("below middle row at content end", ("| a | b |\n| --- | --- |\n| c | d‸ |\n| e | éé |\n", Some("| a | b |\n| --- | --- |\n| c | d |\n| e | éé‸ |\n"), Command::Below)),
            ("below last row", ("| a | b |\n| --- | --- |\n| c | d‸ |\n", Some("| a | b |\n| --- | --- |\n| c | d |\n|  | ‸ |\n"), Command::Below)),
            ("below into missing cell", ("| a | b‸ |\n| --- | --- |\n| c |\n", Some("| a | b |\n| --- | --- |\n| c | ‸ |\n"), Command::Below)),
            ("quoted list continuation", ("> - | a | b |\n>   | --- | --- |\n>   | c | d‸ |\n", Some("> - | a | b |\n>   | --- | --- |\n>   | c | d |\n>   | ‸ |  |\n"), Command::Next)),
            ("delimiter is inert", ("| a | b |\n| ---‸ | --- |\n", None, Command::Next)),
            ("paragraph is inert", ("x‸\n", None, Command::Below)),
        ], |rule, (before, expected, command)| {
            step_case(rule, before, expected, |text, events, at| match command {
                Command::Next => table_step(text, events, at, false),
                Command::Back => table_step(text, events, at, true),
                Command::Below => cell_below(text, events, at),
            });
        });
    }

    #[test]
    fn rows_insert_and_delete() {
        #[derive(Clone, Copy)]
        enum Command { Above, Below, Delete }
        all([
            ("insert above body", ("| a | b |\n| --- | --- |\n| c | d‸ |\n", Some("| a | b |\n| --- | --- |\n|  | ‸ |\n| c | d |\n"), Command::Above)),
            ("insert below body", ("| a | b |\n| --- | --- |\n| c | d‸ |\n| e | f |\n", Some("| a | b |\n| --- | --- |\n| c | d |\n|  | ‸ |\n| e | f |\n"), Command::Below)),
            ("insert below header", ("| a | b‸ |\n| --- | --- |\n| c | d |\n", Some("| a | b |\n| --- | --- |\n|  | ‸ |\n| c | d |\n"), Command::Below)),
            ("inert above header", ("| a‸ | b |\n| --- | --- |\n", None, Command::Above)),
            ("delete middle row", ("| a | b |\n| --- | --- |\n| c | d |\n| e | f‸ |\n| g | h |\n", Some("| a | b |\n| --- | --- |\n| c | d |\n| g | ‸h |\n"), Command::Delete)),
            ("delete last row moves up", ("| a | b |\n| --- | --- |\n| c | d |\n| e | f‸ |\n", Some("| a | b |\n| --- | --- |\n| c | ‸d |\n"), Command::Delete)),
            ("delete sole body row", ("| a | b |\n| --- | --- |\n| c | d‸ |\n", Some("| a | ‸b |\n| --- | --- |\n"), Command::Delete)),
            ("delete pads next row", ("| a | b |\n| --- | --- |\n| c | d‸ |\n| e |\n", Some("| a | b |\n| --- | --- |\n| e | ‸ |\n"), Command::Delete)),
            ("delete quoted row", ("> | a |\n> | --- |\n> | c‸ |\n> | d |\n", Some("> | a |\n> | --- |\n> | ‸d |\n"), Command::Delete)),
            ("header deletion is inert", ("| a‸ | b |\n| --- | --- |\n", None, Command::Delete)),
        ], |rule, (before, expected, command)| {
            step_case(rule, before, expected, |text, events, at| match command {
                Command::Above => insert_row(text, events, at, true),
                Command::Below => insert_row(text, events, at, false),
                Command::Delete => delete_row(text, events, at),
            });
        });
    }

    #[test]
    fn columns_insert_and_delete() {
        #[derive(Clone, Copy)]
        enum Command { Left, Right, Delete }
        all([
            ("insert left retains alignments", ("| a | b |\n| :--- | ---: |\n| c | d‸ |\n", Some("| a |  | b |\n| :--- | --- | ---: |\n| c | ‸ | d |\n"), Command::Left)),
            ("insert right retains center", ("| a‸ | b |\n| :-: | ---: |\n| c | d |\n", Some("| a | ‸ | b |\n| :-: | --- | ---: |\n| c |  | d |\n"), Command::Right)),
            ("insert at left edge", ("| a | b |\n| --- | --- |\n| c‸ | d |\n", Some("|  | a | b |\n| --- | --- | --- |\n| ‸ | c | d |\n"), Command::Left)),
            ("append column pads short row", ("| a | b‸ |\n| --- | --- |\n| c |\n", Some("| a | b | ‸ |\n| --- | --- | --- |\n| c |  |  |\n"), Command::Right)),
            ("delete middle column", ("| a | b | c |\n| :--- | :-: | ---: |\n| d | e‸ | fé |\n", Some("| a | c |\n| :--- | ---: |\n| d | ‸fé |\n"), Command::Delete)),
            ("delete right edge moves left", ("| a | b |\n| --- | --- |\n| c | d‸ |\n", Some("| a |\n| --- |\n| ‸c |\n"), Command::Delete)),
            ("delete only column removes table", ("one\n\n| a |\n| --- |\n| b‸ |\n\ntwo\n", Some("one\n\n‸\ntwo\n"), Command::Delete)),
            ("delete entire sole table", ("| a‸ |\n| --- |\n", Some("‸"), Command::Delete)),
            ("no outer pipes insertion", ("a‸ | b\n:--- | ---:\nc | d\n", Some("a | ‸ | b\n:--- | --- | ---:\nc |  | d\n"), Command::Right)),
            ("no outer pipes deletion", ("a | b | c\n:--- | :-: | ---:\nd | e‸ | f\n", Some("a | c\n:--- | ---:\nd | ‸f\n"), Command::Delete)),
            ("short row not reaching deletion stays exact", ("| a | b‸ | c |\n| --- | --- | --- |\n| d  |\n", Some("| a | ‸c |\n| --- | --- |\n| d  |\n"), Command::Delete)),
            ("missing current cell deletion keeps short row", ("| a | b | c |\n| --- | --- | --- |\n| d  |‸\n", Some("| a | b |\n| --- | --- |\n| d  |‸\n"), Command::Delete)),
            ("delete only written cell of short row", ("| a | b | c |\n| --- | --- | --- |\n| d‸ |\n", Some("| b | c |\n| --- | --- |\n| ‸ |\n"), Command::Delete)),
            ("hidden extras preserved on insertion", ("| a | b |\n| --- | --- |\n| c‸ | d |  hidden  |x|\n", Some("| a |  | b |\n| --- | --- | --- |\n| c | ‸ | d |  hidden  |x|\n"), Command::Right)),
            ("hidden extras preserved on deletion", ("| a | b |\n| --- | --- |\n| c‸ | d |  hidden  |x|\n", Some("| b |\n| --- |\n| ‸d |  hidden  |x|\n"), Command::Delete)),
            ("escaped pipe survives rewrite", ("| a\\|b‸ | c |\n| --- | --- |\n", Some("| a\\|b | ‸ | c |\n| --- | --- | --- |\n"), Command::Right)),
            ("list and quote prefixes retained", ("> - | a‸ | b |\n>   | --- | --- |\n>   | c | d |\n", Some("> - | a | ‸ | b |\n>   | --- | --- | --- |\n>   | c |  | d |\n"), Command::Right)),
        ], |rule, (before, expected, command)| {
            step_case(rule, before, expected, |text, events, at| match command {
                Command::Left => insert_column(text, events, at, true),
                Command::Right => insert_column(text, events, at, false),
                Command::Delete => delete_column(text, events, at),
            });
        });
    }

    #[test]
    fn header_autoformat() {
        all([
            ("pipe header", ("| a | b |‸\n", Some("| a | b |\n| --- | --- |\n| ‸ |  |\n"))),
            ("midline is inert", ("| a‸ | b |\n", None)),
            ("one cell is inert", ("| a |‸\n", None)),
            ("no opening pipe is inert", ("a | b‸\n", None)),
            ("multiline paragraph is inert", ("first\n| a | b |‸\n", None)),
            ("code is inert", ("```\n| a | b |‸\n```\n", None)),
            ("existing table is inert", ("| a | b |‸\n| --- | --- |\n", None)),
            ("quote prefix", ("> | a | b |‸\n", Some("> | a | b |\n> | --- | --- |\n> | ‸ |  |\n"))),
            ("tight list prefix", ("- | a | b |‸\n", Some("- | a | b |\n  | --- | --- |\n  | ‸ |  |\n"))),
            ("escaped pipes do not add columns", ("| a\\|b | `c\\|d` |‸\n", Some("| a\\|b | `c\\|d` |\n| --- | --- |\n| ‸ |  |\n"))),
        ], |rule, (before, expected)| step_case(rule, before, expected, table_from_header));
    }

    #[test]
    fn cell_blocks_select_copy_clear_and_delete() {
        let text = "| a | b | c |\n|:--|:-:|--:|\n| d | e | f |\n| g | h | i |\n\nafter\n";
        let mut parser = Parser::new_ext(text, parse_options());
        parser.assume_footnote_labels_defined();
        let events: Vec<_> = parser.into_offset_iter().collect();
        let at = |s: &str| text.find(s).unwrap();
        let block = |from: &str, to: &str| cell_block(text, &events, at(from), at(to));
        // The result of a step, its caret marked.
        let applied = |step: Option<Step>| {
            let step = step.expect("a step");
            let mut out = step.plan.map_or_else(|| text.to_string(), |plan| plan.apply(text));
            out.insert(step.select.end, '‸');
            out
        };
        assert_eq!(cell_block(text, &events, at("a"), at("a") + 1), None, "a selection in one cell stays text");
        assert_eq!(block("e", "after"), None, "a selection leaving the table stays text");
        assert_eq!(block("e", "|:-:"), None, "the delimiter line holds no cells");
        let up = block("e", "b").expect("two cells make a block");
        assert_eq!((up.rows, up.columns), (0..2, 1..2), "the block runs from either end");
        let corner = block("e", "i").unwrap();
        assert!(!corner.whole_rows(), "two of three columns");
        assert_eq!(block_text(text, &events, &corner).unwrap(), "| e | f |\n| :-: | --: |\n| h | i |\n", "copied as a table, alignments kept");
        assert_eq!(applied(clear_block(text, &events, &corner)), "| a | b | c |\n|:--|:-:|--:|\n| d | ‸ |  |\n| g |  |  |\n\nafter\n", "cleared cells keep the pipes");
        assert_eq!(applied(delete_block(text, &events, &corner)), applied(clear_block(text, &events, &corner)), "a part of rows is cleared, not removed");
        let body = block("d", "i").unwrap();
        assert!(body.whole_rows(), "every column");
        assert_eq!(applied(delete_block(text, &events, &body)), "| ‸a | b | c |\n|:--|:-:|--:|\n\nafter\n", "whole body rows go");
        assert_eq!(applied(delete_block(text, &events, &block("a", "h").unwrap())), "| ‸c |\n| --: |\n| f |\n| i |\n\nafter\n", "whole columns go from every row");
        assert_eq!(applied(delete_block(text, &events, &block("a", "i").unwrap())), "‸\nafter\n", "every cell takes the table");
    }
}
