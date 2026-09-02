//! Sheets on the AI bus: bounded reads and undoable cell writes.

use crate::{
    formula,
    sheet::{pos_name, Pos, Workbook},
};
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_widgets::makepad_platform::makepad_micro_serde::*;

const ROWS: usize = 1000;
const COLS: usize = 64;
const MAX_CELLS: usize = 2000;
const MAX_FIND_RESULTS: usize = 50;

/// The manifest shared by the standalone service and the module executor.
pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "sheets",
        "Sheets",
        "The spreadsheet on screen. Read its current shape and displayed cell values, search those values, or make undoable edits to cells. Formulas start with `=`.",
    )
    .with_tool(ToolDef::new(
        "summary",
        "The sheet on screen: its name, how many sheets the workbook has, the used range, the header row and the selection.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "read_range",
        "Read displayed values from one cell or a rectangular A1 range, up to 2000 cells. Rows are returned as tab-separated lines.",
        r#"{"type":"object","properties":{"range":{"type":"string","description":"A cell or rectangular range such as A1 or A1:C20"}},"required":["range"]}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "find",
        "Find a case-insensitive substring in displayed values in the active sheet's used range. Returns at most 50 matching cells.",
        r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "write_cell",
        "Write one cell. An input beginning with `=` is a formula; any other input is a value. The edit is recalculated and undoable.",
        r#"{"type":"object","properties":{"cell":{"type":"string","description":"A cell such as A1"},"input":{"type":"string"}},"required":["cell","input"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "set_range",
        "Write arrays of string inputs from the top-left of an A1 range, up to 2000 cells. Formula inputs begin with `=`; the edit is recalculated and undoable.",
        r#"{"type":"object","properties":{"range":{"type":"string","description":"A cell or rectangular range such as A1:C20"},"rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}},"required":["range","rows"]}"#,
        Risk::Act,
    ))
}

#[derive(DeJson)]
struct RangeArgs {
    range: String,
}

#[derive(DeJson)]
struct FindArgs {
    query: String,
}

#[derive(DeJson)]
struct WriteCellArgs {
    cell: String,
    input: String,
}

#[derive(DeJson)]
struct SetRangeArgs {
    range: String,
    rows: Vec<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellRange {
    first: Pos,
    last: Pos,
}

impl CellRange {
    fn rows(self) -> usize {
        self.last.0 - self.first.0 + 1
    }

    fn cols(self) -> usize {
        self.last.1 - self.first.1 + 1
    }

    fn cells(self) -> usize {
        self.rows() * self.cols()
    }
}

fn parse_cell(text: &str) -> Result<Pos, String> {
    let text = text.trim();
    if text.contains('$') {
        return Err(format!("malformed cell `{text}`; use A1 notation"));
    }
    let Some(cell) = formula::parse_a1(text) else {
        return Err(format!("malformed cell `{text}`; use A1 notation"));
    };
    let pos = (cell.row, cell.col);
    if pos.0 >= ROWS || pos.1 >= COLS {
        return Err(format!("cell `{text}` is outside A1:{}", pos_name((ROWS - 1, COLS - 1))));
    }
    Ok(pos)
}

fn parse_range(text: &str) -> Result<CellRange, String> {
    let text = text.trim();
    let mut parts = text.split(':');
    let first = parse_cell(parts.next().unwrap_or(""))?;
    let last = match parts.next() {
        Some(end) => parse_cell(end)?,
        None => first,
    };
    if parts.next().is_some() || last.0 < first.0 || last.1 < first.1 {
        return Err(format!("malformed range `{text}`; use A1 or A1:C20"));
    }
    let range = CellRange { first, last };
    if range.cells() > MAX_CELLS {
        return Err(format!(
            "refused: `{text}` contains {} cells; the limit is {MAX_CELLS}",
            range.cells()
        ));
    }
    Ok(range)
}

fn parse_args<T: DeJson>(call: &ServiceCall) -> Result<T, ToolResult> {
    T::deserialize_json_lenient(&call.args).map_err(|error| {
        ToolResult::refused(
            &call.call_id,
            format!("invalid arguments for sheets.{}: {error:?}", call.tool),
        )
    })
}

fn read_range(workbook: &Workbook, range: CellRange) -> String {
    let sheet = workbook.sheet();
    let mut lines = Vec::with_capacity(range.rows());
    for row in range.first.0..=range.last.0 {
        let values: Vec<String> = (range.first.1..=range.last.1)
            .map(|col| sheet.display((row, col)))
            .collect();
        lines.push(values.join("\t"));
    }
    lines.join("\n")
}

fn find(workbook: &Workbook, query: &str) -> Vec<(Pos, String)> {
    let query = query.to_lowercase();
    let Some((first, last)) = workbook.sheet().used_range() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    'rows: for row in first.0..=last.0 {
        for col in first.1..=last.1 {
            let value = workbook.sheet().display((row, col));
            if value.to_lowercase().contains(&query) {
                found.push(((row, col), value));
                if found.len() == MAX_FIND_RESULTS {
                    break 'rows;
                }
            }
        }
    }
    found
}

/// Answer one call against the active workbook. Both service front ends call
/// this function; the view adds its redraw/chrome refresh after successful
/// writes.
pub fn answer(call: &ServiceCall, summary: impl FnOnce() -> String, workbook: &mut Workbook) -> ToolResult {
    match call.tool.as_str() {
        "summary" => ToolResult::ok(&call.call_id, summary(), "the sheet on screen"),
        "read_range" => {
            let args: RangeArgs = match parse_args(call) {
                Ok(args) => args,
                Err(result) => return result,
            };
            let range = match parse_range(&args.range) {
                Ok(range) => range,
                Err(error) => return ToolResult::refused(&call.call_id, error),
            };
            ToolResult::ok(
                &call.call_id,
                read_range(workbook, range),
                format!("{} rows × {} cols", range.rows(), range.cols()),
            )
        }
        "find" => {
            let args: FindArgs = match parse_args(call) {
                Ok(args) => args,
                Err(result) => return result,
            };
            if args.query.is_empty() {
                return ToolResult::refused(&call.call_id, "find query must not be empty");
            }
            let found = find(workbook, &args.query);
            let text = if found.is_empty() {
                format!("no cells contain {:?}", args.query)
            } else {
                found
                    .iter()
                    .map(|(pos, value)| format!("{}: {value}", pos_name(*pos)))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            ToolResult::ok(&call.call_id, text, format!("{} matches", found.len()))
        }
        "write_cell" => {
            let args: WriteCellArgs = match parse_args(call) {
                Ok(args) => args,
                Err(result) => return result,
            };
            let pos = match parse_cell(&args.cell) {
                Ok(pos) => pos,
                Err(error) => return ToolResult::refused(&call.call_id, error),
            };
            workbook.set_input(pos, &args.input);
            let name = pos_name(pos);
            ToolResult::ok(&call.call_id, format!("wrote {name}"), format!("wrote {name}"))
        }
        "set_range" => {
            let args: SetRangeArgs = match parse_args(call) {
                Ok(args) => args,
                Err(result) => return result,
            };
            let range = match parse_range(&args.range) {
                Ok(range) => range,
                Err(error) => return ToolResult::refused(&call.call_id, error),
            };
            let input_cells: usize = args.rows.iter().map(Vec::len).sum();
            let widest = args.rows.iter().map(Vec::len).max().unwrap_or(0);
            if input_cells > MAX_CELLS {
                return ToolResult::refused(
                    &call.call_id,
                    format!("refused: rows contain {input_cells} cells; the limit is {MAX_CELLS}"),
                );
            }
            if args.rows.len() > range.rows() || widest > range.cols() {
                return ToolResult::refused(
                    &call.call_id,
                    format!(
                        "rows do not fit in {}:{} ({} rows × {} cols)",
                        pos_name(range.first),
                        pos_name(range.last),
                        range.rows(),
                        range.cols()
                    ),
                );
            }
            workbook.paste_block(range.first, &args.rows);
            let target = if range.first == range.last {
                pos_name(range.first)
            } else {
                format!("{}:{}", pos_name(range.first), pos_name(range.last))
            };
            ToolResult::ok(
                &call.call_id,
                format!("wrote {input_cells} cells from {}", pos_name(range.first)),
                format!("wrote {target}"),
            )
        }
        other => ToolResult::refused(
            &call.call_id,
            format!("sheets has no tool `{other}`; it has summary, read_range, find, write_cell, set_range"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::wire::ToolOutcome;

    fn call(id: &str, tool: &str, args: &str) -> ServiceCall {
        ServiceCall { call_id: id.into(), tool: tool.into(), args: args.into() }
    }

    fn answer_call(call: &ServiceCall, workbook: &mut Workbook) -> ToolResult {
        answer(call, || "Sheet summary".to_string(), workbook)
    }

    #[test]
    fn the_manifest_validates_with_read_and_act_tools() {
        let m = manifest();
        assert_eq!(m.id, "sheets");
        m.validate().expect("a manifest the wire accepts");
        assert_eq!(m.tools.len(), 5);
        assert_eq!(m.tool("summary").unwrap().risk, Risk::Read);
        assert_eq!(m.tool("read_range").unwrap().risk, Risk::Read);
        assert_eq!(m.tool("find").unwrap().risk, Risk::Read);
        assert_eq!(m.tool("write_cell").unwrap().risk, Risk::Act);
        assert_eq!(m.tool("set_range").unwrap().risk, Risk::Act);
    }

    #[test]
    fn cell_and_range_parsing_enforces_sheet_and_tool_bounds() {
        assert_eq!(parse_cell("A1"), Ok((0, 0)));
        assert_eq!(parse_cell("bl1000"), Ok((999, 63)));
        for bad in ["", "A0", "$A$1", "BM1", "A1001", "1A", "A1:B2"] {
            assert!(parse_cell(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_range("A1:C4").unwrap().cells(), 12);
        assert!(parse_range("C4:A1").is_err());
        assert!(parse_range("A1:BL31").is_ok());
        let error = parse_range("A1:BL32").unwrap_err();
        assert!(error.contains("2048 cells"), "{error}");
    }

    #[test]
    fn read_range_returns_display_values_and_dimensions() {
        let mut workbook = Workbook::default();
        workbook.paste_block(
            (0, 0),
            &[vec!["Name".into(), "Count".into()], vec!["Apples".into(), "3".into()]],
        );
        let result = answer_call(&call("r", "read_range", r#"{"range":"A1:B2"}"#), &mut workbook);
        assert_eq!(result.outcome, ToolOutcome::Ok);
        assert_eq!(result.text, "Name\tCount\nApples\t3");
        assert_eq!(result.note, "2 rows × 2 cols");
    }

    #[test]
    fn find_is_case_insensitive_and_returns_cell_names() {
        let mut workbook = Workbook::default();
        workbook.paste_block(
            (0, 0),
            &[vec!["Alpha".into(), "beta".into()], vec!["ALPHABET".into(), "other".into()]],
        );
        let result = answer_call(&call("f", "find", r#"{"query":"alpha"}"#), &mut workbook);
        assert_eq!(result.outcome, ToolOutcome::Ok);
        assert_eq!(result.text, "A1: Alpha\nA2: ALPHABET");
    }

    #[test]
    fn a_write_is_visible_to_the_next_read_and_recalculates() {
        let mut workbook = Workbook::default();
        let write = answer_call(
            &call("w1", "write_cell", r#"{"cell":"A1","input":"21"}"#),
            &mut workbook,
        );
        assert_eq!(write.outcome, ToolOutcome::Ok);
        assert_eq!(write.note, "wrote A1");
        let formula = answer_call(
            &call("w2", "write_cell", r#"{"cell":"B1","input":"=A1*2"}"#),
            &mut workbook,
        );
        assert_eq!(formula.outcome, ToolOutcome::Ok);
        let read = answer_call(&call("r", "read_range", r#"{"range":"A1:B1"}"#), &mut workbook);
        assert_eq!(read.text, "21\t42");
    }

    #[test]
    fn unknown_tools_and_oversized_ranges_are_refused() {
        let mut workbook = Workbook::default();
        let result = answer_call(&call("u", "nope", "{}"), &mut workbook);
        assert_eq!(result.outcome, ToolOutcome::Refused);
        let result = answer_call(&call("r", "read_range", r#"{"range":"A1:BL1000"}"#), &mut workbook);
        assert_eq!(result.outcome, ToolOutcome::Refused);
        assert!(result.text.contains("64000 cells"));
    }
}
