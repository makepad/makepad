//! The table story: columns with headings, three looks, a header that stays
//! put, cells that hold widgets, and a sort that asks rather than reorders.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TableOverview = StoryPage{
        StoryNote{text: "The plain table: a fixed list of records lined up under headings. Every row is present, so there is no draw loop to write and no host callback to forget — a table with markup in it shows that markup. The grid next door is the other answer: it virtualises millions of rows, holds none of your data and asks for cells one at a time, which is right for a sheet and heavy for eleven prices."}

        StoryHeading{text: "A table"}
        StoryNote{text: "A column is a heading, a width and an alignment written as one string: `Amount|90|end`. Leave the width out and the column shares what the fixed columns leave over. A row is its cells, separated by the same bar."}
        StoryRow{
            subject := Table{
                width: 420. height: Fit
                columns: ["Item" "Price|90|end" "Status|130"]
                rows: [
                    "Kettle|12.50|In stock"
                    "Toaster|8.00|Backordered"
                    "Lamp|24.00|In stock"
                    "Clock|5.75|Discontinued"
                    "Mirror|31.20|In stock"
                ]
            }
        }

        StoryHeading{text: "Alignment belongs to the column"}
        StoryNote{text: "Numbers right, words left, marks in the middle. It is a property of the column rather than of each cell because a column where some numbers are right and some are not is unreadable, and that is exactly what per-cell alignment invites."}
        StoryRow{
            Table{
                width: 420. height: Fit
                framed: true
                columns: ["Part" "Made|80|center" "Weight|90|end" "Cost|80|end"]
                rows: [
                    "Bracket|1998|1.40|9.00"
                    "Hinge|2004|0.25|2.75"
                    "Spindle|1987|12.05|140.00"
                ]
            }
        }

        StoryHeading{text: "Three looks"}
        StoryNote{text: "Striped washes every other row, so a wide row can be followed across without a finger on the screen. Bordered adds a hairline between every column and a frame around the whole thing, for a table of short values where the columns are the point."}
        StoryRow{
            TableStriped{
                width: 300. height: Fit
                columns: ["Session" "Length|70|end" "Takes|60|end"]
                rows: [
                    "Morning|0:42|3"
                    "Afternoon|1:16|7"
                    "Evening|0:08|1"
                    "Overnight|6:02|2"
                    "Handover|0:21|4"
                ]
            }
            TableBordered{
                width: 300. height: Fit
                columns: ["Session" "Length|70|end" "Takes|60|end"]
                rows: [
                    "Morning|0:42|3"
                    "Afternoon|1:16|7"
                    "Evening|0:08|1"
                    "Overnight|6:02|2"
                    "Handover|0:21|4"
                ]
            }
        }
        StoryNote{text: "Compact tightens the rows. It does not shrink the type: a table you cannot read is not denser, it is worse."}
        StoryRow{
            Table{
                width: 300. height: Fit
                columns: ["Session" "Length|70|end" "Takes|60|end"]
                rows: [
                    "Morning|0:42|3"
                    "Afternoon|1:16|7"
                    "Evening|0:08|1"
                    "Overnight|6:02|2"
                    "Handover|0:21|4"
                ]
            }
            TableCompact{
                width: 300. height: Fit
                columns: ["Session" "Length|70|end" "Takes|60|end"]
                rows: [
                    "Morning|0:42|3"
                    "Afternoon|1:16|7"
                    "Evening|0:08|1"
                    "Overnight|6:02|2"
                    "Handover|0:21|4"
                ]
            }
        }

        StoryHeading{text: "The header stays put"}
        StoryNote{text: "Give the table a height instead of letting it fit its rows and the body scrolls under the wheel, or under the thumb on the right. The heading band is outside the scrolling part, so it never leaves — a table whose headings scroll away stops saying what its columns are exactly when there is most to read."}
        StoryRow{
            TableStriped{
                width: 420. height: 220.
                framed: true
                columns: ["Reading" "Taken|110|center" "Value|80|end"]
                rows: [
                    "North gate|09:00|11.2"
                    "North gate|10:00|11.9"
                    "North gate|11:00|13.4"
                    "North gate|12:00|14.0"
                    "South gate|09:00|8.6"
                    "South gate|10:00|9.1"
                    "South gate|11:00|9.9"
                    "South gate|12:00|10.4"
                    "River|09:00|3.2"
                    "River|10:00|3.4"
                    "River|11:00|3.9"
                    "River|12:00|4.6"
                    "Reservoir|09:00|21.0"
                    "Reservoir|10:00|20.4"
                    "Reservoir|11:00|19.8"
                    "Reservoir|12:00|19.1"
                ]
            }
        }

        StoryHeading{text: "Sorting says what was asked for"}
        StoryNote{text: "Press a heading: unsorted, up, down, unsorted again. The table moves the indicator and reports the ask — it does not reorder anything. The rows in this demo change order because the page below sorts them and hands them back, which is the whole contract."}
        StoryNote{text: "The third press is not decoration. It has to be able to get back to the order the data arrived in, and a two-state toggle never can."}
        StoryRow{
            sorted := Table{
                width: 420. height: Fit
                sortable: true
                striped: true
                columns: ["Item" "Price|90|end" "Status|130"]
                rows: [
                    "Kettle|12.50|In stock"
                    "Toaster|8.00|Backordered"
                    "Lamp|24.00|In stock"
                    "Clock|5.75|Discontinued"
                    "Mirror|31.20|In stock"
                ]
            }
        }
        StoryRow{
            sort_note := Label{text: "nothing sorted yet"}
        }
        StoryNote{text: "Note that price sorts as a number and item as a word. The table could not have known that: it holds the rendering of a value, not the value, so left to itself it would have put 8.00 after 31.20 and had nothing at all to say about dates or money."}

        StoryHeading{text: "A cell can be a widget"}
        StoryNote{text: "A cell written @name is built from the template of that name on the table's own instance. One named entry in the markup buys a column of controls, and the table keeps one widget per cell so the button in row four stays the button for row four."}
        StoryNote{text: "Widget cells are not measured, so give that column a width — and give the rows enough height for what stands in them."}
        StoryRow{
            with_widgets := Table{
                width: 420. height: Fit
                row_height: 36.
                columns: ["Report" "Written|110|center" "|110|center"]
                rows: [
                    "Spring survey|12 Mar|@open"
                    "Autumn survey|30 Sep|@open"
                    "Handover notes|02 Nov|@open"
                ]

                // A named entry: it is collected as a cell template rather
                // than drawn as a child of the table.
                open := Button{
                    text: "Open"
                }
            }
        }
        StoryRow{
            cell_note := Label{text: "nothing opened yet"}
        }
    }

    mod.stories.TableDiagonalHeaders = StoryPage{
        StoryNote{text: "A column of two-digit counts needs about thirty points. The name over it does not fit in that, and a flat heading row has only two answers: widen the column to its name, which turns a sheet of counts into white space, or cut the name short. With `header_angle` set, a heading whose name does not fit its column is written on the diagonal instead, standing on the middle of its own column. A heading that fits stays flat — so an angle on a table of wide columns changes nothing at all."}

        StoryHeading{text: "Narrow counts under long names"}
        StoryNote{text: "Six counts in 34-point columns beside a wide `Site` column. The six names cannot lie flat in 34 points, so they turn and rise to the right, the default lean; `Site` fits its own column and stays flat on the bottom line beside them. The band is as tall as the longest turned name needs. Pressing a heading still sorts the column the name stands on."}
        StoryRow{
            subject := TableBordered{
                width: Fit height: Fit
                sortable: true
                cell_pad: 6.
                header_angle: 45.
                columns: ["Site" "Readings taken|34|end" "Missed readings|34|end" "Alarms raised|34|end" "Alarms cleared|34|end" "Visits by staff|34|end" "Days without a reading|34|end"]
                rows: [
                    "North gate|12|2|4|4|3|0"
                    "South gate|9|5|1|1|2|4"
                    "River bend|11|1|7|6|5|1"
                    "Reservoir|14|0|2|2|1|0"
                    "Old quarry|6|8|3|0|0|6"
                ]
            }
        }
        StoryRow{
            angle_note := Label{text: "nothing pressed yet"}
        }

        StoryHeading{text: "Falling"}
        StoryNote{text: "`header_lean: Fall` ends each turned name on its own column, having begun up and to the left, so the names hang back over the wide column instead of past the last one. Pick it when the room is on that side."}
        StoryRow{
            falling := TableBordered{
                width: Fit height: Fit
                cell_pad: 6.
                header_angle: 45.
                header_lean: DiagonalLean.Fall
                columns: ["Site" "Readings taken|34|end" "Missed readings|34|end" "Alarms raised|34|end" "Alarms cleared|34|end" "Visits by staff|34|end" "Days without a reading|34|end"]
                rows: [
                    "North gate|12|2|4|4|3|0"
                    "South gate|9|5|1|1|2|4"
                    "River bend|11|1|7|6|5|1"
                    "Reservoir|14|0|2|2|1|0"
                    "Old quarry|6|8|3|0|0|6"
                ]
            }
        }
    }
}

/// The rows in the order they arrived in. The sort demo works from this and
/// never from what the table is currently showing, because "unsorted" has to
/// mean this order and nothing else.
const STOCK: &[&str] = &[
    "Kettle|12.50|In stock",
    "Toaster|8.00|Backordered",
    "Lamp|24.00|In stock",
    "Clock|5.75|Discontinued",
    "Mirror|31.20|In stock",
];

fn cell_of(line: &str, col: usize) -> &str {
    line.split('|').nth(col).unwrap_or("").trim()
}

/// The rows in the order a heading press asked for. Numbers compare as
/// numbers and everything else as words, which is the decision the host has
/// to make and the table cannot.
fn ordered(col: usize, ascending: bool) -> Vec<String> {
    let mut lines: Vec<String> = STOCK.iter().map(|line| line.to_string()).collect();
    lines.sort_by(|left, right| {
        let (left, right) = (cell_of(left, col), cell_of(right, col));
        match (left.parse::<f64>(), right.parse::<f64>()) {
            (Ok(left), Ok(right)) => {
                left.partial_cmp(&right).unwrap_or(std::cmp::Ordering::Equal)
            }
            _ => left.to_lowercase().cmp(&right.to_lowercase()),
        }
    });
    if !ascending {
        lines.reverse();
    }
    lines
}

fn table_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let sorted = root.table(cx, ids!(sorted));
    if let Some((col, ascending)) = sorted.sort_changed(actions) {
        let lines = match ascending {
            Some(up) => ordered(col, up),
            None => STOCK.iter().map(|line| line.to_string()).collect(),
        };
        sorted.set_row_lines(cx, &lines);
        let note = match ascending {
            Some(true) => format!("asked for column {col} up"),
            Some(false) => format!("asked for column {col} down"),
            None => "asked for the order it arrived in".to_string(),
        };
        root.label(cx, ids!(sort_note)).set_text(cx, &note);
    }

    let widgets = root.table(cx, ids!(with_widgets));
    for row in 0..widgets.row_count() {
        if widgets.cell_widget(row, 2).as_button().clicked(actions) {
            root.label(cx, ids!(cell_note))
                .set_text(cx, &format!("opened the report in row {}", row + 1));
        }
    }
}

/// The diagonal page's own listener: which column a press on a turned
/// heading asked for, written under the table. The point of it is that the
/// answer is the column the name stands on and not one its ink crosses.
fn diagonal_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.table(cx, ids!(subject));
    if let Some((col, ascending)) = subject.sort_changed(actions) {
        let lines = match ascending {
            Some(up) => ordered_counts(col, up),
            None => COUNTS.iter().map(|line| line.to_string()).collect(),
        };
        subject.set_row_lines(cx, &lines);
        let note = match ascending {
            Some(true) => format!("asked for column {col} up"),
            Some(false) => format!("asked for column {col} down"),
            None => "asked for the order it arrived in".to_string(),
        };
        root.label(cx, ids!(angle_note)).set_text(cx, &note);
    }
}

/// The counts in the order they arrived in, for the page above.
const COUNTS: &[&str] = &[
    "North gate|12|2|4|4|3|0",
    "South gate|9|5|1|1|2|4",
    "River bend|11|1|7|6|5|1",
    "Reservoir|14|0|2|2|1|0",
    "Old quarry|6|8|3|0|0|6",
];

/// The same ordering the overview page does, over these rows: numbers as
/// numbers, everything else as words.
fn ordered_counts(col: usize, ascending: bool) -> Vec<String> {
    let mut lines: Vec<String> = COUNTS.iter().map(|line| line.to_string()).collect();
    lines.sort_by(|left, right| {
        let (left, right) = (cell_of(left, col), cell_of(right, col));
        match (left.parse::<f64>(), right.parse::<f64>()) {
            (Ok(left), Ok(right)) => {
                left.partial_cmp(&right).unwrap_or(std::cmp::Ordering::Equal)
            }
            _ => left.to_lowercase().cmp(&right.to_lowercase()),
        }
    });
    if !ascending {
        lines.reverse();
    }
    lines
}

pub const STORIES: &[Story] = &[Story {
    key: "collections/table/overview",
    category: "Collections",
    component: "Table",
    also: &["TableStriped", "TableBordered", "TableCompact"],
    name: "Overview",
    dsl: "TableOverview",
    added: "2026-09-10",
    tags: &["new", "data", "rows", "columns", "sort", "static"],
    doc: "# Table

The plain table: columns with headings, and rows that are all present.

It is for a fixed list of records where the whole job is to line them up and be readable, which is far more common than a sheet, and a table with markup in it draws the markup. Which of the widgets that show many rows to reach for is set out once, on Lists > Overview.

## Columns

A column is a heading, a width and an alignment, written as one string — `\"Amount|90|end\"`. The fields are positional, so an alignment without a width leaves the width empty: `\"Status||center\"`. A column with no width shares what the fixed columns leave over, and shares it on top of what its own content needs, so a column of long names does not come out the same width as a column of ticks.

Three parallel lists — headings, widths, alignments — was the alternative. They go out of step the first time somebody adds a column to one of them, and they do it silently.

## Cells

A cell is text, or a widget. A cell written `@name` is built from the template of that name on the table's own instance, so a column of buttons costs one named entry in the markup and nothing in the Rust. The table keeps one widget per cell, and a widget scrolled out of view stops taking events rather than staying pressable where it used to be. Widget cells are not measured — give that column a width.

## Sorting says what was asked for

Pressing a heading cycles that column: unsorted, up, down, unsorted again. The table does **not** reorder anything. It moves the indicator and raises `SortChanged`; the host, which owns the rows, puts them in the new order and hands them back with `set_row_lines` or `set_rows`. That is the same contract the grid keeps.

It is not laziness. The table holds the *rendering* of a value, not the value: left to itself it would sort \"10\" before \"9\", and would have nothing at all to say about dates, money or names in a language with accents. The host knows which column is which. And the third press matters as much as the first two — it has to be able to get back to the order the data arrived in, which a two-state toggle can never do.

`sortable` is off by default: a heading that does something when pressed has to mean it, and most tables are read-only. `set_unsortable_cols` exempts the columns with no order to be in — a column of controls, a column of pictures.

## The header stays put

Give the table a height rather than letting it fit its rows, and the body scrolls under the wheel or under the thumb while the heading band stays where it is. A table whose headings scroll away stops saying what its columns are exactly when there is most to read.

## What it deliberately does not do

No virtualisation — every row is held and the ones in view are drawn, which is honest for tens of rows and dishonest for millions. No selection and no editing, so there is no hover wash on a body row: a cell that has to be pressable is a widget cell. No column resizing or reordering, which are what make the grid a grid. And no keyboard, because there is nothing to move a cursor between.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Row height", target: "subject", kind: ControlKind::Number { prop: "row_height", min: 16., max: 64., step: 1., default: 26. } },
        Control { label: "Header height", target: "subject", kind: ControlKind::Number { prop: "header_height", min: 16., max: 64., step: 1., default: 28. } },
        Control { label: "Cell padding", target: "subject", kind: ControlKind::Number { prop: "cell_pad", min: 0., max: 32., step: 1., default: 10. } },
        Control { label: "Hairline", target: "subject", kind: ControlKind::Number { prop: "line_size", min: 0., max: 3., step: 0.5, default: 1. } },
        Control { label: "Heading band", target: "subject", kind: ControlKind::Bool { prop: "show_header", default: true } },
        Control { label: "Striped", target: "subject", kind: ControlKind::Bool { prop: "striped", default: false } },
        Control { label: "Row lines", target: "subject", kind: ControlKind::Bool { prop: "row_lines", default: true } },
        Control { label: "Column lines", target: "subject", kind: ControlKind::Bool { prop: "column_lines", default: false } },
        Control { label: "Frame", target: "subject", kind: ControlKind::Bool { prop: "framed", default: false } },
        Control { label: "Sortable", target: "subject", kind: ControlKind::Bool { prop: "sortable", default: false } },
    ],
    on_actions: Some(table_actions),
}, Story {
    key: "collections/table/diagonal-headings",
    category: "Collections",
    component: "Table",
    also: &["DataGrid"],
    name: "Diagonal headings",
    dsl: "TableDiagonalHeaders",
    added: "2026-09-21",
    tags: &["new", "data", "columns", "header", "diagonal", "angle"],
    doc: "# Diagonal headings

`header_angle` lets a table write a heading on the diagonal when — and only when — its name does not fit its column. It is nought by default, which turns nothing.

## Only where it earns its place

A column of two-digit counts needs about thirty points; the name over it needs four or five times that. Flat, the table has to widen the column to the name, which makes a sheet of counts mostly white space, or cut the name short. Turned, the name stands on the middle of its own column and runs over its neighbours, and the column keeps the width of its values.

A heading whose name fits its column gains nothing from being turned, so it is not: the fit is measured, name plus the cell padding against the column's width, column by column, and a heading that fits stays flat on the bottom line of the band. A table of wide columns with an angle set is exactly the table without one. The band is as tall as the longest turned name needs, and no taller than the stated `header_height` when nothing turns.

## Which way it leans

`header_lean: Rise`, the default, starts each turned name on its column and takes it up to the right; the table keeps the room beyond its last column that the ink needs. `header_lean: Fall` ends each name on its column, so the ink hangs back over the columns to the left — over a wide first column, as on this page, it needs no extra room at all.

## What still works

Sorting, the hover wash and the press belong to the column, not to the ink: the column a name names is the upright strip under it, and that is what a press lands on. The sort mark rides the end of a turned name, and sorting never tips a heading from flat to turned. `DataGrid` takes the same two properties and decides the same way, per column, again whenever a column is resized.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Angle", target: "subject", kind: ControlKind::Number { prop: "header_angle", min: 0., max: 80., step: 5., default: 45. } },
        Control { label: "Striped", target: "subject", kind: ControlKind::Bool { prop: "striped", default: false } },
        Control { label: "Column lines", target: "subject", kind: ControlKind::Bool { prop: "column_lines", default: true } },
    ],
    on_actions: Some(diagonal_actions),
}];
