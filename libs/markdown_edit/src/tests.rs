use crate::*;

const FIXTURE: &str = "# Title

Intro with *emphasis* and a [link][ref].

- item one
- item two
  continued

Middle paragraph.

- other list
- second

> quote line
> more

after quote

a | b

<div>
html
</div>

Tail paragraph[^n].

```rust
fn main() {}
```

[ref]: https://example.com
[^n]: The note.";

fn lines_of(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

/// The (line, byte) of a byte offset in the joined text.
fn pos_of(lines: &[String], offset: usize) -> (usize, usize) {
    let mut at = 0;
    for (line, text) in lines.iter().enumerate() {
        if offset <= at + text.len() {
            return (line, offset - at);
        }
        at += text.len() + 1;
    }
    panic!("offset {offset} past the end")
}

/// Replaces the first `needle` (after `skip` matches) with `with`, as a
/// document does it: a delete, then an insert, each reported as a line edit.
fn replace(
    lines: &mut Vec<String>,
    index: &mut Index,
    step: &str,
    needle: &str,
    with: &str,
) -> Splice {
    let text = lines.join("\n");
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("{step}: `{needle}` not in the document"));
    let (l0, b0) = pos_of(lines, offset);
    let (l1, b1) = pos_of(lines, offset + needle.len());
    let mut edits = Vec::new();
    // Delete.
    let tail = lines[l1][b1..].to_string();
    lines[l0].truncate(b0);
    lines[l0].push_str(&tail);
    lines.drain(l0 + 1..=l1);
    edits.push(LineEdit {
        line: l0,
        removed: l1 - l0,
        added: 0,
    });
    // Insert.
    let tail = lines[l0][b0..].to_string();
    lines[l0].truncate(b0);
    let mut inserted = with.split('\n');
    lines[l0].push_str(inserted.next().unwrap_or(""));
    let mut at = l0;
    for part in inserted {
        at += 1;
        lines.insert(at, part.to_string());
    }
    lines[at].push_str(&tail);
    edits.push(LineEdit {
        line: l0,
        removed: 0,
        added: at - l0,
    });
    let rows_before = index.row_count();
    let splice = index
        .apply(lines, &edits)
        .unwrap_or_else(|| panic!("{step}: apply reported nothing"));
    let full = Index::new(lines);
    assert!(
        index.same_blocks(&full),
        "{step}: incremental index differs from a full parse\nincremental: {:?}\nfull: {:?}\ntext:\n{}",
        index.blocks(),
        full.blocks(),
        lines.join("\n")
    );
    assert_eq!(
        rows_before - splice.old_rows.len() + splice.new_rows.len(),
        index.row_count(),
        "{step}: the row splice does not account for the row count"
    );
    splice
}

#[test]
fn incremental_reparse_equals_full_parse() {
    let mut lines = lines_of(FIXTURE);
    let mut index = Index::new(&lines);

    // The fixture's shape, so the cases below start from a known index.
    let kinds: Vec<&BlockKind> = index.blocks().iter().map(|b| &b.kind).collect();
    assert_eq!(kinds.len(), 12, "fixture block count: {kinds:?}");
    assert_eq!(index.link("REF"), Some(("https://example.com", "")), "link table");
    assert_eq!(index.footnote_number("n"), Some(1), "footnote table");
    let list = index.blocks().iter().position(|b| matches!(b.kind, BlockKind::List { .. })).unwrap();
    let rows = index.block_rows(list);
    assert_eq!(rows.len(), 2, "a top-level list draws one row per item");
    assert_eq!(index.row(rows.start + 1).lines, 5..8, "the second item row runs to the next block");
    assert_eq!(index.row_at_line(6), rows.start + 1, "row_at_line inside an item");

    // Typing inside a line: only the touched block is new.
    let splice = replace(&mut lines, &mut index, "typing", "Middle", "Middle typed");
    assert_eq!(splice.new_blocks.len(), 1, "typing reparses one block");

    // A list merges with the next list of the same marker when the
    // paragraph between them goes (the guard block resyncs past it).
    replace(&mut lines, &mut index, "lists merge", "Middle typed paragraph.\n\n", "");
    replace(&mut lines, &mut index, "lists split again", "- other list", "Split.\n\n- other list");

    // A setext underline turns the paragraph above into a heading.
    replace(&mut lines, &mut index, "setext", "Split.\n", "Split.\n---\n");
    // A lazy continuation: the paragraph after a quote joins it.
    replace(&mut lines, &mut index, "lazy continuation", "> more\n\n", "> more\n");
    // A table delimiter row appears under a header line.
    replace(&mut lines, &mut index, "table appears", "a | b\n", "a | b\n--|--\n");
    // An HTML block that ended at a blank line takes the next paragraph.
    replace(&mut lines, &mut index, "html block grows", "</div>\n\n", "</div>\n");
    // A comment left open turns the rest of the file into HTML.
    replace(&mut lines, &mut index, "comment opens", "Intro with", "<!--\nIntro with");
    replace(&mut lines, &mut index, "comment closes", "<!--\n", "");
    // A fence left open turns the rest of the file into code, and back.
    replace(&mut lines, &mut index, "fence opens", "- item one", "```\n- item one");
    replace(&mut lines, &mut index, "fence closes", "```\n- item one", "- item one");
    // Definitions come and go, and the tables follow.
    let splice = replace(&mut lines, &mut index, "ref def removed", "[ref]: https://example.com\n", "");
    assert!(splice.globals_changed, "removing a definition changes the tables");
    assert_eq!(index.link("ref"), None, "the removed definition is gone");
    replace(&mut lines, &mut index, "ref def added", "# Title", "# Title\n\n[ref]: /here");
    assert_eq!(index.link("ref"), Some(("/here", "")), "the added definition resolves");
    // Footnotes number in order of first reference; a definition nothing
    // references comes after them. References typed before their
    // definitions exist count once the definitions appear. (The tail
    // paragraph's `[^n]` is raw HTML by now: the html block took it.)
    replace(&mut lines, &mut index, "unresolved references", "Intro with", "Intro[^o] with[^n]");
    let splice = replace(&mut lines, &mut index, "footnote defs", "[^n]: The note.", "[^m]: First.\n[^n]: The note.\n[^o]: Third.");
    assert!(splice.globals_changed, "new definitions renumber");
    let numbers = |index: &Index| ["m", "n", "o"].map(|label| index.footnote_number(label).unwrap_or(0));
    assert_eq!(numbers(&index), [3, 2, 1], "footnotes number by first reference, unreferenced last");
    let splice = replace(&mut lines, &mut index, "reference removed", "Intro[^o] with", "Intro with");
    assert!(splice.globals_changed, "removing the first reference renumbers");
    assert_eq!(numbers(&index), [2, 1, 3], "the next reference moves up, the unreferenced follow by definition");
    // Front matter at line 0 appears, an unclosed one closes far below, and
    // it goes again.
    replace(&mut lines, &mut index, "front matter", "# Title", "---\ntitle: x\n---\n# Title");
    assert_eq!(index.blocks()[0].kind, BlockKind::FrontMatter, "front matter block");
    replace(&mut lines, &mut index, "front matter unclosed", "title: x\n---\n", "title: x\n");
    replace(&mut lines, &mut index, "closer typed below", "Tail paragraph", "...\nTail paragraph");
    replace(&mut lines, &mut index, "front matter removed", "---\ntitle: x\n", "");
    // A join across lines and blocks, then a long code block in rows.
    replace(&mut lines, &mut index, "join blocks", "item two\n  continued\n\nSplit.\n---\n\n- other", "item two other");
    let long_code: String = (0..2 * CODE_ROW_LINES + 3).map(|i| format!("line {i}\n")).collect();
    replace(&mut lines, &mut index, "long code", "fn main() {}\n", &long_code);
    let code = index.blocks().iter().position(|b| b.kind == BlockKind::CodeFenced).unwrap();
    assert_eq!(index.block_rows(code).len(), 3, "a long code block draws in runs of lines");
}

#[test]
fn visible_text_maps_to_source_and_back() {
    let source = "# Head\n\nA \\* &amp; **bold** and [link](u) `code` ![alt](i.png)\nsoft line[^f]\n\n- [x] done\n\n[^f]: note\n";
    let lines = lines_of(source.trim_end_matches('\n'));
    let index = Index::new(&lines);
    let slice = Slice::new(&lines, 0..lines.len());
    let events = index.parse_slice(&slice);
    let projection = Projection::new(&slice.text, &events);
    let text = &projection.text;
    assert!(
        text.starts_with("Head\nA * & bold and link code \u{FFFC} soft line\u{FFFC}"),
        "visible text: {text:?}"
    );

    // Every caret stop maps to the source and back to itself, both ways.
    let inside_atom = |at: usize| {
        projection.pieces.iter().any(|piece| {
            matches!(piece.kind, PieceKind::Atomic(_))
                && at > piece.visible.start
                && at < piece.visible.end
        })
    };
    for at in text.char_indices().map(|(i, _)| i).chain([text.len()]) {
        if inside_atom(at) {
            continue;
        }
        for affinity in [Affinity::Left, Affinity::Right] {
            let src = projection.to_source(at, affinity);
            assert_eq!(
                projection.to_visible(src),
                at,
                "visible {at} -> source {src} ({affinity:?}) -> back"
            );
        }
    }
    // Every source offset lands on a stop that maps back to the same stop.
    for src in (0..=slice.text.len()).filter(|&i| slice.text.is_char_boundary(i)) {
        let at = projection.to_visible(src);
        let again = projection.to_source(at, Affinity::Left);
        assert_eq!(projection.to_visible(again), at, "source {src} -> visible {at} is stable");
    }

    let src_of = |needle: &str| slice.text.find(needle).unwrap();
    let vis_of = |needle: &str| text.find(needle).unwrap();
    // An escape hides its backslash: the star is visible, the slash is not.
    let star = vis_of("*");
    assert_eq!(projection.to_source(star, Affinity::Right), src_of("\\*") + 1, "escaped star");
    assert_eq!(projection.to_source(star, Affinity::Left), src_of("\\*"), "before the escape");
    // An entity is one atomic character for its whole source.
    assert_eq!(projection.to_visible(src_of("&amp;") + 2), vis_of("&"), "inside an entity");
    // Typing at the end of bold stays bold; at the end of a link it does not.
    let bold_end = vis_of("bold") + 4;
    assert_eq!(projection.caret_source(bold_end), src_of("bold**") + 4, "inside the bold end");
    assert_eq!(projection.to_source(bold_end, Affinity::Right), src_of("** and") + 2, "after the bold");
    let link_end = vis_of("link") + 4;
    assert_eq!(projection.caret_source(link_end), src_of("](u)") + 4, "after the link");
    // A code span's content maps one to one inside its backticks.
    assert_eq!(projection.to_source(vis_of("code") + 1, Affinity::Left), src_of("code`") + 1, "code span");
    let code = projection.inlines.iter().find(|i| i.kind == InlineKind::Code).unwrap();
    assert_eq!(&slice.text[code.open.clone()], "`", "code open marker");
    let bold = projection.inlines.iter().find(|i| i.kind == InlineKind::Strong).unwrap();
    assert_eq!(&slice.text[bold.close.clone()], "**", "strong close marker");
    // A soft break is one visible space for the line end.
    let soft = projection.pieces.iter().find(|p| p.kind == PieceKind::Atomic(Atom::SoftBreak)).unwrap();
    assert_eq!(&slice.text[soft.src.clone()], "\n", "soft break source");
    // The task marker is an atomic object, and the footnote reference too.
    assert!(
        projection.pieces.iter().any(|p| p.kind == PieceKind::Atomic(Atom::TaskMarker(true))),
        "task marker"
    );
    assert!(
        projection.pieces.iter().any(|p| p.kind == PieceKind::Atomic(Atom::FootnoteRef)),
        "footnote reference"
    );
}
