use super::*;
use makepad_widgets::event::{TouchPoint, TouchState, TouchUpdateEvent};
use crate::{
    decoration::DecorationSet,
    document::{AnchorRange, CodeDocument, PreparedDocument},
    history::EditKind,
    selection::SelectionSet,
    session::{PreparedView, PreparedViewError, SessionId},
    text::{Change, Drift, Edit, Length},
};

fn pos(line_index: usize, byte_index: usize) -> Position {
    Position {
        line_index,
        byte_index,
    }
}

fn doc(source: &str) -> CodeDocument {
    CodeDocument::new(source.into(), DecorationSet::default())
}

fn edit(document: &CodeDocument, change: Change) {
    document.edit_selections(
        SessionId::default(),
        EditKind::Other,
        &SelectionSet::new(),
        &Settings::default(),
        |mut editor, _, _| {
            editor.apply_edit(Edit {
                change: change.clone(),
                drift: Drift::Before,
            });
        },
    );
}

fn editor() -> (Cx, CodeEditor) {
    editor_from(|vm| script_eval!(vm, { use mod.widgets.* CodeEditor{} }))
}

/// An editor whose bars scroll it when a finger drags the text.
fn drag_scrolling_editor() -> (Cx, CodeEditor) {
    editor_from(|vm| script_eval!(vm, {
        use mod.widgets.*
        CodeEditor{scroll_bars +: {scroll_bar_x.drag_scrolling: true scroll_bar_y.drag_scrolling: true}}
    }))
}

fn editor_from(eval: impl FnOnce(&mut ScriptVm) -> ScriptValue) -> (Cx, CodeEditor) {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let editor = cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        crate::script_mod(vm);
        vm.bx.captured_errors = Some(Vec::new());
        let value = eval(vm);
        let editor = CodeEditor::script_from_value(vm, value);
        assert!(vm.take_errors().is_empty());
        editor
    });
    (cx, editor)
}

fn measured_editor() -> (Cx, CodeEditor) {
    let (cx, mut editor) = editor();
    editor.cell_size = dvec2(8.0, 16.0);
    editor.ascent = 10.0;
    editor.descent = 3.0;
    editor.cell_offset_y = 1.5;
    editor.pad_left_top = dvec2(0.0, 0.0);
    editor.show_gutter = false;
    editor.viewport_rect = Rect {
        pos: dvec2(100.0, 50.0),
        size: dvec2(600.0, 300.0),
    };
    editor.unscrolled_rect = editor.viewport_rect;
    (cx, editor)
}

#[test]
fn anchors_follow_shared_insertions_and_deletion() {
    let document = doc("abcdef");
    let before = document.create_anchor(pos(0, 2), Drift::Before);
    let after = document.create_anchor(pos(0, 2), Drift::After);
    let clone = document.clone();
    edit(&clone, Change::Insert(pos(0, 0), "x".into()));
    assert_eq!(document.resolve_anchor(before), Some(pos(0, 3)));
    edit(&clone, Change::Insert(pos(0, 3), "yz".into()));
    assert_eq!(document.resolve_anchor(before), Some(pos(0, 3)));
    assert_eq!(document.resolve_anchor(after), Some(pos(0, 5)));
    edit(&clone, Change::Insert(pos(0, 8), "z".into()));
    assert_eq!(document.resolve_anchor(after), Some(pos(0, 5)));
    edit(
        &clone,
        Change::Delete(
            pos(0, 1),
            Length {
                line_count: 0,
                byte_count: 6,
            },
        ),
    );
    assert_eq!(document.resolve_anchor(before), Some(pos(0, 1)));
    assert_eq!(document.resolve_anchor(after), Some(pos(0, 1)));
    assert_eq!(doc("abc").resolve_anchor(before), None);
    document.remove_anchor(before);
    assert_eq!(clone.resolve_anchor(before), None);
}

#[test]
fn anchors_follow_multiline_edits_and_shared_undo() {
    let document = doc("abc\ndef\nghi");
    let anchor = document.create_anchor(pos(2, 1), Drift::After);
    let mut session = CodeSession::new(document.clone());
    session.set_selection(
        pos(0, 0),
        Affinity::After,
        SelectionMode::Simple,
        NewGroup::Yes,
    );
    session.paste("z\n".into());
    session.handle_changes();
    assert_eq!(document.resolve_anchor(anchor), Some(pos(3, 1)));
    assert!(session.undo());
    session.handle_changes();
    assert_eq!(document.resolve_anchor(anchor), Some(pos(2, 1)));
    edit(
        &document,
        Change::Delete(
            pos(0, 2),
            Length {
                line_count: 2,
                byte_count: 2,
            },
        ),
    );
    assert_eq!(document.resolve_anchor(anchor), Some(pos(0, 2)));
}

#[test]
fn range_retains_document_positions_and_follows_shared_edits() {
    let document = doc("prefix\nab界cd\nefgh\ntail");
    let anchors = AnchorRange {
        start: document.create_anchor(pos(1, 2), Drift::Before),
        end: document.create_anchor(pos(2, 2), Drift::After),
    };
    let mut session = CodeSession::new(document.clone());
    session.set_view_range(Some(anchors));
    assert_eq!(session.layout().view_line_range(), 1..3);
    assert_eq!(session.layout().line_byte_range(1), 2..7);
    assert_eq!(session.layout().line_byte_range(2), 0..2);
    assert_eq!(session.layout().line(1).y(), 0.0);
    assert_eq!(session.layout().height(), 2.0);
    assert_eq!(
        session
            .layout()
            .logical_to_normalized_position(pos(1, 5), Affinity::After),
        (4.0, 0.0)
    );
    edit(&document, Change::Insert(pos(0, 0), "new\n".into()));
    edit(&document, Change::Insert(pos(2, 5), "X".into()));
    session.handle_changes();
    assert_eq!(session.view_range(), Some(pos(2, 2)..pos(3, 2)));
    assert_eq!(session.layout().view_line_range(), 2..4);
    assert_eq!(session.layout().line(2).y(), 0.0);
    session.set_view_range(None);
    assert_eq!(session.layout().line(2).y(), 2.0);
}

#[test]
fn range_excludes_end_line_and_clamps_selection() {
    let document = doc("one\ntwo\nthree");
    let mut session = CodeSession::new(document.clone());
    session.set_view_range(Some(AnchorRange {
        start: document.create_anchor(pos(1, 1), Drift::Before),
        end: document.create_anchor(pos(2, 0), Drift::After),
    }));
    assert_eq!(session.layout().view_line_range(), 1..2);
    assert_eq!(session.layout().height(), 1.0);
    session.set_selection(pos(0, 0), Affinity::After, SelectionMode::All, NewGroup::No);
    assert_eq!(session.selections()[0].start(), pos(1, 1));
    assert_eq!(session.selections()[0].end(), pos(2, 0));
    edit(
        &document,
        Change::Delete(
            pos(0, 0),
            Length {
                line_count: 2,
                byte_count: 2,
            },
        ),
    );
    session.handle_changes();
    assert_eq!(session.view_range(), Some(pos(0, 0)..pos(0, 0)));
    assert_eq!(session.layout().height(), 1.0);
}

#[test]
fn prepared_document_and_view_are_send_and_attach_without_relayout() {
    fn assert_send<T: Send>() {}
    assert_send::<PreparedDocument>();
    assert_send::<PreparedView>();
    let (prepared, view) = std::thread::spawn(|| {
        let prepared = CodeDocument::prepare("// hello\nlet v = vec![界];".into());
        let view = CodeSession::prepare_view(
            &prepared,
            Some(pos(1, 0)..pos(1, prepared.as_text().as_lines()[1].len())),
        );
        (prepared, view)
    })
    .join()
    .unwrap();
    let digest = prepared.digest();
    let text_pointer = prepared.as_text().as_lines()[1].as_ptr();
    let document = CodeDocument::from_prepared(prepared);
    assert_eq!(document.digest(), digest);
    assert_eq!(document.as_text().as_lines()[1].as_ptr(), text_pointer);
    let session = CodeSession::from_prepared(document.clone(), view).unwrap();
    assert_eq!(session.layout().line(1).y(), 0.0);
    assert_eq!(
        session.layout().line(1).width(),
        document.as_text().as_lines()[1].column_count() as f64
    );
    assert!(document.layout().tokens[1]
        .iter()
        .any(|token| token.kind == TokenKind::Macro));
}

#[test]
fn prepared_view_rejects_wrong_digest_edited_version_and_invalid_range() {
    let prepared = CodeDocument::prepare("abc".into());
    let view = CodeSession::prepare_view(&prepared, None);
    assert_eq!(
        CodeSession::from_prepared(doc("xyz"), view.clone()).unwrap_err(),
        PreparedViewError::Stale
    );
    let document = CodeDocument::from_prepared(prepared.clone());
    document.replace("abc".into());
    assert_eq!(document.digest(), prepared.digest());
    assert_eq!(
        CodeSession::from_prepared(document, view).unwrap_err(),
        PreparedViewError::Stale
    );
    let invalid = CodeSession::prepare_view(&prepared, Some(pos(0, 2)..pos(0, 8)));
    assert_eq!(
        CodeSession::from_prepared(CodeDocument::from_prepared(prepared), invalid).unwrap_err(),
        PreparedViewError::InvalidRange
    );
}

#[test]
fn hit_test_matches_wide_utf8_cells_scroll_and_range() {
    let (_cx, mut editor) = measured_editor();
    let document = doc("hidden\na界\tz!");
    let mut session = CodeSession::new(document.clone());
    session.set_view_range(Some(AnchorRange {
        start: document.create_anchor(pos(1, 1), Drift::Before),
        end: document.create_anchor(pos(1, 6), Drift::After),
    }));
    let rect = editor.position_rect(&session, pos(1, 1)).unwrap();
    assert_eq!(rect.pos, dvec2(8.0, 0.0));
    assert_eq!(rect.size, dvec2(16.0, 16.0));
    let hit = editor
        .hit_test(&session, rect.pos + rect.size * 0.75)
        .unwrap();
    assert_eq!(hit.position, pos(1, 4));
    assert_eq!(hit.grapheme_range, pos(1, 1)..pos(1, 4));
    assert_eq!(hit.rect, rect);
    assert!(editor.position_rect(&session, pos(1, 2)).is_none());
    assert!(editor.position_rect(&session, pos(1, 6)).is_none());
    assert!(editor.hit_test(&session, dvec2(4.0, 8.0)).is_none());
    assert!(editor.hit_test(&session, dvec2(100.0, 8.0)).is_none());
    // Draw's viewport origin includes scrolling; public coordinates are outer-local.
    editor.viewport_rect.pos.x -= 4.0;
    assert_eq!(
        editor.position_rect(&session, pos(1, 1)).unwrap().pos.x,
        4.0
    );
    assert_eq!(
        editor.hit_test(&session, dvec2(5.0, 8.0)).unwrap().position,
        pos(1, 1)
    );
}

#[test]
fn base_font_scale_multiplies_fold_scale_without_frame_contamination() {
    let (_cx, mut editor) = measured_editor();
    let session = CodeSession::new(doc("        folded\nnext"));
    let before = editor.metrics(&session);
    editor.set_font_scale(0.5);
    let scaled = editor.metrics(&session);
    assert_eq!(scaled.line_advance, before.line_advance * 0.5);
    assert_eq!(scaled.column_advance, before.column_advance * 0.5);
    assert_eq!(scaled.ascent, before.ascent * 0.5);
    assert_eq!(scaled.descent, before.descent * 0.5);
    session.fold();
    while session.update_folds() {}
    assert_eq!(
        editor.position_rect(&session, pos(0, 8)).unwrap().size.y,
        0.8
    );
    editor.draw_text.font_scale = editor.base_font_scale * 0.1;
    editor.draw_gutter.font_scale = editor.base_font_scale * 0.1;
    editor.reset_draw_font_scale();
    assert_eq!(editor.draw_text.font_scale, 0.5);
    assert_eq!(editor.draw_gutter.font_scale, 0.5);
    assert_eq!(editor.metrics(&session), scaled);
    editor.set_font_scale(f32::NAN);
    assert_eq!(editor.metrics(&session), scaled);
}

#[test]
fn wrapped_range_rebases_first_visible_row_and_keeps_global_geometry() {
    let (_cx, editor) = measured_editor();
    let document = doc("one two three four\nlast");
    let mut session = CodeSession::new(document.clone());
    session.set_wrap_column(Some(8));
    session.set_view_range(Some(AnchorRange {
        start: document.create_anchor(pos(0, 8), Drift::Before),
        end: document.create_anchor(pos(0, 13), Drift::After),
    }));
    assert_eq!(
        editor.position_rect(&session, pos(0, 8)).unwrap().pos.y,
        0.0
    );
    assert_eq!(session.layout().height(), 1.0);
    assert_eq!(session.layout().width(), 5.0);
}

#[test]
fn macro_and_attribute_tokens_respect_lexical_context_and_incremental_edits() {
    let document = doc("vec![] foo ![]\n#![cfg(any(\n feature = \"x]y\", /* ] */ test\n))]\nfn f() {} // #[x] foo!\n\"#[x] foo!\"");
    let kinds = document.layout().tokens[0]
        .iter()
        .map(|token| token.kind)
        .collect::<Vec<_>>();
    assert_eq!(kinds[0], TokenKind::Macro);
    assert!(kinds.contains(&TokenKind::Identifier));
    for line in 1..=3 {
        assert!(document.layout().tokens[line]
            .iter()
            .all(|token| token.kind == TokenKind::Attribute));
    }
    assert_eq!(document.layout().tokens[4][0].kind, TokenKind::OtherKeyword);
    assert_eq!(document.layout().tokens[5][0].kind, TokenKind::String);
    edit(
        &document,
        Change::Delete(
            pos(1, 0),
            Length {
                line_count: 0,
                byte_count: 1,
            },
        ),
    );
    assert!(document.layout().tokens[2]
        .iter()
        .any(|token| token.kind != TokenKind::Attribute));
    for (line, tokens) in document
        .as_text()
        .as_lines()
        .iter()
        .zip(&document.layout().tokens)
    {
        assert_eq!(
            line.len(),
            tokens.iter().map(|token| token.len).sum::<usize>()
        );
    }
}

#[test]
fn read_only_blocks_return_typing_paste_ime_undo_redo_cut_and_drop() {
    assert_mutation_events_refused(doc("unchanged"), true);
    let prepared = CodeDocument::prepare_diff(&[
        diff_row(DiffRowKind::Removed, Some(0), None, 1),
    ], "unchanged", "").unwrap();
    assert_mutation_events_refused(CodeDocument::from_prepared_diff(prepared), false);
}

fn assert_mutation_events_refused(document: CodeDocument, explicit_read_only: bool) {
    use std::sync::{Arc, Mutex};
    let (mut cx, mut editor) = editor();
    let mut session = CodeSession::new(document);
    record_editor(&mut cx, &mut editor, &mut session);
    editor.set_key_focus(&mut cx);
    cx.action(());
    cx.handle_actions();
    assert!(cx.has_key_focus(editor.scroll_bars.area()));
    session.set_selection(pos(0, 0), Affinity::After, SelectionMode::All, NewGroup::No);
    let copy = TextClipboardEvent { response: Default::default() };
    editor.handle_event(&mut cx, &Event::TextCopy(copy.clone()), &mut Scope::empty(), &mut session);
    assert_eq!(copy.response.borrow().as_deref(), Some("unchanged"));
    editor.blink_timer = cx.start_timeout(0.5);
    editor.set_read_only(&mut cx, explicit_read_only);
    if explicit_read_only { assert_eq!(editor.blink_timer.0, 0); }
    let mut events = Vec::new();
    for key_code in [
        KeyCode::ReturnKey,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::KeyZ,
        KeyCode::KeyY,
        KeyCode::KeyX,
        KeyCode::KeyV,
    ] {
        events.push(Event::KeyDown(KeyEvent {
            key_code,
            modifiers: KeyModifiers {
                control: true,
                ..Default::default()
            },
            ..Default::default()
        }));
    }
    for (was_paste, replace_last) in [(false, false), (true, false), (false, true)] {
        events.push(Event::TextInput(TextInputEvent {
            input: "change".into(),
            was_paste,
            replace_last,
            ..Default::default()
        }));
    }
    events.push(Event::TextRangeReplace(TextRangeReplaceEvent {
        start: 0,
        end: 1,
        text: "IME".into(),
        replaced_text: None,
        fallback_to_insert: true,
    }));
    events.push(Event::TextCut(TextClipboardEvent {
        response: Default::default(),
    }));
    events.push(Event::Drop(DropEvent {
        modifiers: KeyModifiers::default(),
        handled: Arc::new(Mutex::new(false)),
        abs: dvec2(0.0, 0.0),
        items: Arc::new(vec![DragItem::String {
            value: "drop".into(),
            internal_id: None,
        }]),
    }));
    for event in events {
        assert!(CodeEditor::is_mutation_event(&event));
        assert!(editor
            .handle_event(&mut cx, &event, &mut Scope::empty(), &mut session)
            .is_empty());
        assert_eq!(session.document().as_text().to_string(), "unchanged");
        assert_eq!(session.document().version(), 0);
        assert_eq!(editor.blink_timer.0, 0);
    }
}

#[test]
fn steady_caret_never_rearms_blink_and_read_only_cancels_pending_timeout() {
    let (mut cx, mut editor) = editor();
    editor.reset_cursor_blinker(&mut cx);
    assert_ne!(editor.blink_timer.0, 0);
    editor.set_caret_policy(CaretPolicy::Steady);
    assert_eq!(editor.blink_timer.0, 0);
    editor.reset_cursor_blinker(&mut cx);
    assert_eq!(editor.retired_blink_timer.0, 0);
    assert_eq!(editor.blink_timer.0, 0);
    editor.set_caret_policy(CaretPolicy::Blink);
    editor.reset_cursor_blinker(&mut cx);
    assert_ne!(editor.blink_timer.0, 0);
    editor.set_read_only(&mut cx, true);
    editor.reset_cursor_blinker(&mut cx);
    assert_eq!(editor.blink_timer.0, 0);
}

#[test]
fn content_opacity_is_clamped_without_changing_background_or_theme() {
    let (_cx, mut editor) = editor();
    let bg = editor.draw_bg.color;
    let gutter = editor.draw_gutter.color;
    let text = editor.token_colors.identifier;
    editor.set_content_opacity(0.25);
    assert_eq!(editor.content_opacity, 0.25);
    assert_eq!(editor.draw_bg.color, bg);
    assert_eq!(editor.draw_gutter.color, gutter);
    assert_eq!(editor.token_colors.identifier, text);
    editor.set_content_opacity(-1.0);
    assert_eq!(editor.content_opacity, 0.0);
    editor.set_content_opacity(2.0);
    editor.set_content_opacity(f32::NAN);
    assert_eq!(editor.content_opacity, 1.0);
}

fn record_editor(cx: &mut Cx, editor: &mut CodeEditor, session: &mut CodeSession) {
    record_frame(cx, 200.0, |cx| editor.draw_walk_editor(cx, session, Walk::fixed(480.0, 200.0)));
}

/// Records one frame of a pass 480 wide and `height` tall, drawn by `draw`.
fn record_frame(cx: &mut Cx, height: f64, draw: impl FnOnce(&mut Cx2d)) {
    let pass = DrawPass::new(cx);
    pass.set_size(cx, dvec2(480.0, height));
    let mut list = DrawList2d::new(cx);
    let event = DrawEvent::default();
    let mut cx_draw = CxDraw::new(cx, &event);
    let mut cx = Cx2d::new(&mut cx_draw);
    cx.begin_pass(&pass, Some(1.0));
    list.begin_always(&mut cx);
    cx.begin_root_turtle(dvec2(480.0, height), Layout::default());
    draw(&mut cx);
    // Ended like a pass, so the editor's area gets its clip and takes hits.
    cx.end_pass_sized_turtle();
    list.end(&mut cx);
    cx.end_pass(&pass);
}

#[test]
fn recorded_content_opacity_and_folded_last_line_keep_metrics_stable() {
    let (mut cx, mut editor) = editor();
    let mut session = CodeSession::new(doc("first\n        folded"));
    session.fold();
    while session.update_folds() {}
    session.set_selection(pos(0, 0), Affinity::After, SelectionMode::All, NewGroup::No);
    editor.set_font_scale(0.5);
    editor.set_content_opacity(0.25);
    editor.set_caret_policy(CaretPolicy::Steady);
    record_editor(&mut cx, &mut editor, &mut session);
    let metrics = editor.metrics(&session);
    assert!(metrics.line_advance > 0.0);
    for vars in [
        &editor.draw_selection.draw_vars,
        &editor.draw_cursor.draw_vars,
        &editor.draw_cursor_bg.draw_vars,
    ] {
        let mut opacity = [-1.0];
        vars.get_instance(&mut cx, live_id!(content_opacity), &mut opacity);
        assert_eq!(opacity, [0.25]);
    }
    record_editor(&mut cx, &mut editor, &mut session);
    assert_eq!(editor.metrics(&session), metrics);
    assert_eq!(editor.draw_text.font_scale, 0.5);
    assert_eq!(editor.draw_gutter.font_scale, 0.5);
    assert_eq!(editor.blink_timer.0, 0);
}

fn grid_column(session: &CodeSession, byte_index: usize) -> usize {
    session
        .layout()
        .line(0)
        .logical_to_grid_position(byte_index, Affinity::After)
        .1
}

#[test]
fn tab_advances_to_the_next_stop() {
    use crate::char::CharExt;
    assert_eq!('\t'.column_count(), 1);
    assert_eq!('\t'.column_count_at(0, 4), 4);
    assert_eq!('\t'.column_count_at(2, 4), 2);
    assert_eq!('\t'.column_count_at(4, 4), 4);
    assert_eq!('f'.column_count_at(0, 4), 1);
    assert_eq!('界'.column_count(), 2);

    let session = CodeSession::new(doc("\tfoo"));
    assert_eq!(grid_column(&session, 1), 4, "f in \\tfoo is at column 4");

    let mut session = CodeSession::new(doc("ab\tc"));
    assert_eq!(grid_column(&session, 3), 4, "c in ab\\tc is at column 4 with tab width 4");
    session.set_tab_column_count(8);
    assert_eq!(grid_column(&session, 3), 8, "c in ab\\tc is at column 8 with tab width 8");

    let session = CodeSession::new(doc("\tfoo"));
    session.move_right(true);
    let cursor = session.selections()[0].cursor;
    assert_eq!(cursor.position, pos(0, 1));
    assert_eq!(
        grid_column(&session, cursor.position.byte_index),
        4,
        "cursor-right over a tab jumps to the tab stop"
    );

    let (_cx, editor) = measured_editor();
    let session = CodeSession::new(doc("\tfoo"));
    let hit = editor.hit_test(&session, dvec2(2.0, 8.0)).unwrap();
    assert_eq!(hit.position, pos(0, 0));
    assert_eq!(
        grid_column(&session, hit.position.byte_index),
        0,
        "hit-test at pixel column 2 of a leading tab selects column 0"
    );
    assert_eq!(hit.rect.size.x, 32.0);
}

use crate::{
    decoration::{Decoration, DecorationType, PreparedDecorationsError},
    diff::{DiffEndpoint, DiffRowKind, DiffRowSpec, GutterMode, LineTerminator, PrepareDiffError, PreparedDiffDocument},
    document::DocumentMutationError,
};

fn diff_row(kind: DiffRowKind, old_line: Option<u32>, new_line: Option<u32>, hunk: u32) -> DiffRowSpec {
    DiffRowSpec { kind, old_line, new_line, hunk }
}

fn attach_diff(prepared: PreparedDiffDocument) -> CodeSession {
    let view = CodeSession::prepare_view(prepared.document(), None);
    CodeSession::from_prepared(CodeDocument::from_prepared_diff(prepared), view).unwrap()
}

#[test]
fn diff_endpoint_token_context_survives_removed_multiline_comment() {
    let old = "/* removed\nlet retained = 1;\n*/\nfn after() {}\n";
    let new = "let retained = 1;\nfn after() {}\n";
    let specs = [
        diff_row(DiffRowKind::Removed, Some(0), None, 1),
        diff_row(DiffRowKind::Equal, Some(1), Some(0), 1),
        diff_row(DiffRowKind::Removed, Some(2), None, 2),
        diff_row(DiffRowKind::Equal, Some(3), Some(1), 2),
    ];
    let prepared = CodeDocument::prepare_diff(&specs, old, new).unwrap();
    let old_tokens = CodeDocument::prepare(old.into());
    let new_tokens = CodeDocument::prepare(new.into());
    for (row, spec) in specs.iter().enumerate() {
        let expected = if let Some(line) = spec.new_line {
            &new_tokens.layout().tokens[line as usize]
        } else {
            &old_tokens.layout().tokens[spec.old_line.unwrap() as usize]
        };
        assert_eq!(&prepared.document().layout().tokens[row], expected);
    }
    assert_eq!(prepared.document().layout().tokens[1][0].kind, TokenKind::OtherKeyword);
    assert_eq!(prepared.document().layout().tokens[2][0].kind, TokenKind::Comment);
    let combined = CodeDocument::prepare(prepared.as_text().clone());
    assert_eq!(combined.layout().tokens[1][0].kind, TokenKind::Comment);
    let tokens = prepared.document().layout().tokens.clone();
    let mut session = attach_diff(prepared);
    session.handle_changes();
    assert_eq!(session.document().layout().tokens, tokens);
}

#[test]
fn diff_equal_terminators_refine_and_reconstruct_both_endpoints() {
    let old = "α\t界\r\n\r\nlast\r\n";
    let new = "α\t界\n\nlast";
    let rows = (0..3).map(|line| diff_row(DiffRowKind::Equal, Some(line), Some(line), 7)).collect::<Vec<_>>();
    let prepared = CodeDocument::prepare_diff(&rows, old, new).unwrap();
    assert_eq!(prepared.row_count(), 6);
    assert_eq!(prepared.row_count(), prepared.as_text().as_lines().len());
    assert!(!prepared.as_text().to_string().contains('\r'));
    assert_eq!(prepared.reconstruct(DiffEndpoint::Old).as_bytes(), old.as_bytes());
    assert_eq!(prepared.reconstruct(DiffEndpoint::New).as_bytes(), new.as_bytes());
    let meta = prepared.metadata();
    assert!(meta.old_final_newline);
    assert!(!meta.new_final_newline);
    assert_eq!(meta.old_to_row, [0, 2, 4]);
    assert_eq!(meta.new_to_row, [1, 3, 5]);
    assert_eq!(meta.hunks.len(), 1);
    assert_eq!((meta.hunks[0].removed, meta.hunks[0].added), (3, 3));
    assert!(meta.rows.iter().all(|row| row.gutter_mark == Some(DecorationType::DiffChangedGutter)));
    assert_eq!(meta.rows[0].old.as_ref().unwrap().terminator, LineTerminator::CrLf);
    for row in &meta.rows {
        for (source, text, map) in [(row.old.as_ref(), old, &meta.old_to_row), (row.new.as_ref(), new, &meta.new_to_row)] {
            if let Some(source) = source {
                let display_row = map[source.line as usize];
                assert_eq!(&text[source.bytes.clone()], prepared.as_text().as_lines()[display_row]);
            }
        }
    }
    let session = attach_diff(prepared);
    assert_eq!(session.layout().height(), 6.0);
    assert_eq!(session.layout().line(0).width(), "α\t界".column_count_at(0, 4) as f64);
    assert_eq!(grid_column(&session, "α\t".len()), 4);
    assert_eq!(grid_column(&session, "α\t界".len()), 6);
}

#[test]
fn diff_empty_sentinel_and_final_newline_do_not_invent_source_rows() {
    let empty = CodeDocument::prepare_diff(&[], "", "").unwrap();
    assert_eq!(empty.row_count(), 1);
    assert_eq!(empty.metadata().rows[0].kind, None);
    assert!(empty.metadata().old_to_row.is_empty());
    assert!(empty.metadata().new_to_row.is_empty());
    assert!(empty.decorations().is_empty());
    assert_eq!(empty.reconstruct(DiffEndpoint::Old), "");
    assert_eq!(attach_diff(empty).layout().height(), 1.0);
    for (old, new, kind, old_line, new_line) in [
        ("", "\r\n", DiffRowKind::Added, None, Some(0)),
        ("\n", "", DiffRowKind::Removed, Some(0), None),
        ("bare\r", "", DiffRowKind::Removed, Some(0), None),
        ("x\n", "x\n", DiffRowKind::Equal, Some(0), Some(0)),
    ] {
        let prepared = CodeDocument::prepare_diff(&[diff_row(kind, old_line, new_line, 0)], old, new).unwrap();
        assert_eq!(prepared.row_count(), 1);
        assert_eq!(prepared.reconstruct(DiffEndpoint::Old), old);
        assert_eq!(prepared.reconstruct(DiffEndpoint::New), new);
        let session = attach_diff(prepared);
        assert_eq!(session.document().as_text().as_lines().len(), 1);
        assert_eq!(session.layout().height(), 1.0);
    }
}

#[test]
fn diff_preparation_rejects_incomplete_or_invalid_maps() {
    assert_eq!(CodeDocument::prepare_diff(&[], "x", "").unwrap_err(), PrepareDiffError::IncompleteEndpoints);
    for rows in [
        vec![diff_row(DiffRowKind::Removed, Some(1), None, 0)],
        vec![diff_row(DiffRowKind::Added, Some(0), None, 0)],
        vec![diff_row(DiffRowKind::Equal, Some(0), None, 0)],
        vec![diff_row(DiffRowKind::Removed, Some(0), None, 0); 2],
    ] {
        assert!(matches!(CodeDocument::prepare_diff(&rows, "x", ""), Err(PrepareDiffError::InvalidRow { .. })));
    }
    assert_eq!(CodeDocument::prepare_diff(&[diff_row(DiffRowKind::Equal, Some(0), Some(0), 0)], "x", "y").unwrap_err(),
        PrepareDiffError::UnequalContent { row: 0 });
}

#[test]
fn diff_prepared_replacement_retains_runs_diagnostics_and_separate_hunks() {
    let prepared = CodeDocument::prepare_diff(&[
        diff_row(DiffRowKind::Removed, Some(0), None, 4),
        diff_row(DiffRowKind::Removed, Some(1), None, 4),
        diff_row(DiffRowKind::Added, None, Some(0), 4),
        diff_row(DiffRowKind::Equal, Some(2), Some(1), 4),
    ], "a\n\nb", "c\nb").unwrap();
    assert_eq!(prepared.decorations().len(), 2);
    assert_eq!(prepared.decorations()[0].start(), pos(0, 0));
    assert_eq!(prepared.decorations()[0].end(), pos(2, 0));
    let mut replacement = prepared.decorations().to_vec();
    replacement.push(Decoration::new(100, pos(3, 0), pos(3, 1), DecorationType::Warning));
    let mut set = DecorationSet::new();
    set.replace_prepared(replacement.clone()).unwrap();
    let retained = set.clone();
    assert_eq!(set.replace_prepared(vec![replacement[1], replacement[0]]),
        Err(PreparedDecorationsError::OverlappingOrUnsorted));
    assert_eq!(set, retained);
    set.add_decoration(Decoration::new(101, pos(3, 0), pos(3, 1), DecorationType::Error));
    assert_eq!(set.len(), 3);
    assert_eq!(set[2].ty, DecorationType::Error);
    let mut document = CodeDocument::from_prepared_diff(prepared);
    let hunks = document.diff_metadata().unwrap().clone();
    document.replace_prepared_decorations(replacement.clone()).unwrap();
    assert_eq!(&*document.decorations(), &replacement);
    assert_eq!(document.diff_metadata(), Some(&hunks));
    assert_eq!(document.version(), 0);
}

#[test]
fn diff_blank_line_backgrounds_record_full_clipped_code_width() {
    let prepared = CodeDocument::prepare_diff(&[
        diff_row(DiffRowKind::Removed, Some(0), None, 1),
        diff_row(DiffRowKind::Added, None, Some(0), 1),
        diff_row(DiffRowKind::Added, None, Some(1), 1),
    ], "\n", "\n\n").unwrap();
    let mut session = attach_diff(prepared);
    let (mut cx, mut editor) = editor();
    editor.show_gutter = false;
    editor.word_wrap = false;
    record_editor(&mut cx, &mut editor, &mut session);
    for (vars, row) in [(&editor.draw_diff_removed.draw_vars, 0), (&editor.draw_diff_added.draw_vars, 2)] {
        let rect = editor.diff_row_rect(&session, row).unwrap();
        let mut size = [0.0; 2];
        let mut position = [0.0; 2];
        assert!(vars.get_instance_on_area(&cx, live_id!(rect_size), &mut size));
        assert!(vars.get_instance_on_area(&cx, live_id!(rect_pos), &mut position));
        let mut color = [0.0; 4];
        assert!(vars.get_instance_on_area(&cx, live_id!(color), &mut color));
        assert!((color[3] - 0.14).abs() < 0.001);
        assert_eq!(size, [rect.size.x as f32, rect.size.y as f32]);
        assert_eq!(position, [rect.pos.x as f32, rect.pos.y as f32]);
        assert_eq!(size[0], (480.0 - editor.pad_left_top.x) as f32);
        assert!(size[1] > 0.0);
    }
    assert!(editor.draw_cursor.draw_vars.area().is_valid(&cx));
    let mut blink = [-1.0];
    assert!(editor.draw_cursor.draw_vars.get_instance_on_area(&cx, live_id!(blink), &mut blink));
    assert_eq!(blink, [0.0]);
    assert_eq!(editor.blink_timer.0, 0);
    editor.viewport_rect.pos.x -= 80.0;
    let rect = editor.diff_row_rect(&session, 0).unwrap();
    assert_eq!(rect.pos.x, editor.unscrolled_rect.pos.x + editor.pad_left_top.x);
    assert_eq!(rect.size.x, 480.0 - editor.pad_left_top.x);
    editor.show_gutter = true;
    record_editor(&mut cx, &mut editor, &mut session);
    assert_eq!(editor.gutter_chars, 8);
    assert!(editor.draw_diff_changed.draw_vars.area().is_valid(&cx));
    let diff_width = editor.metrics(&session).gutter_width;
    session.document().set_gutter_mode(GutterMode::Plain);
    assert!(editor.metrics(&session).gutter_width < diff_width);
    assert!(session.document().is_read_only());
}

#[test]
fn diff_document_and_session_mutations_are_noops_but_removed_row_copy_works() {
    let prepared = CodeDocument::prepare_diff(&[
        diff_row(DiffRowKind::Removed, Some(0), None, 1),
        diff_row(DiffRowKind::Added, None, Some(0), 1),
    ], "removed", "target").unwrap();
    let mut session = attach_diff(prepared);
    let document = session.document().clone();
    assert_eq!(document.ensure_editable(), Err(DocumentMutationError::ReadOnlyDiff));
    session.set_selection(pos(0, 0), Affinity::After, SelectionMode::Simple, NewGroup::No);
    session.move_to(pos(0, 7), Affinity::After, NewGroup::No);
    assert_eq!(session.copy(), "removed");
    let selection = session.selections().to_vec();
    let digest = document.digest();
    let tokens = document.layout().tokens.clone();
    let decorations = document.decorations().to_vec();
    session.insert("(".into());
    session.paste("paste".into());
    session.paste_grouped("paste".into(), 42);
    session.enter();
    session.delete();
    session.backspace();
    session.indent();
    session.outdent();
    assert!(!session.undo());
    assert!(!session.redo());
    document.replace("replacement".into());
    edit(&document, Change::Insert(pos(0, 0), "direct".into()));
    document.edit_linewise(SessionId::default(), EditKind::Other, &SelectionSet::new(), |_, _| panic!("diff must refuse before invoking editor"));
    document.force_new_group();
    assert!(!document.undo(SessionId::default(), &SelectionSet::new()));
    assert!(!document.redo(SessionId::default(), &SelectionSet::new()));
    session.handle_changes();
    assert_eq!(&*session.selections(), &selection);
    assert_eq!(document.as_text().to_string(), "removed\ntarget");
    assert_eq!(document.version(), 0);
    assert_eq!(document.digest(), digest);
    assert_eq!(document.layout().tokens, tokens);
    assert_eq!(&*document.decorations(), &decorations);
}

#[test]
fn diff_admission_twenty_thousand_rows_off_thread() {
    use std::time::Instant;
    fn assert_send<T: Send>() {}
    assert_send::<PreparedDiffDocument>();
    let (prepared, view, prepare_ms) = std::thread::spawn(|| {
        let old = "let old = 1;\n".repeat(10_000);
        let new = "let new = 2;\n".repeat(10_000);
        let rows = (0..10_000).flat_map(|line| [
            diff_row(DiffRowKind::Removed, Some(line), None, line),
            diff_row(DiffRowKind::Added, None, Some(line), line),
        ]).collect::<Vec<_>>();
        let start = Instant::now();
        let prepared = CodeDocument::prepare_diff(&rows, &old, &new).unwrap();
        let view = CodeSession::prepare_view(prepared.document(), None);
        let prepare_ms = start.elapsed().as_secs_f64() * 1000.0;
        (prepared, view, prepare_ms)
    }).join().unwrap();
    let row_count = prepared.row_count();
    assert_eq!(row_count, 20_000);
    let text_ptr = prepared.as_text().as_lines().as_ptr();
    let rows_ptr = prepared.metadata().rows.as_ptr();
    let decorations_ptr = prepared.decorations().as_ptr();
    let start = Instant::now();
    let session = CodeSession::from_prepared(CodeDocument::from_prepared_diff(prepared), view).unwrap();
    let attach_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!("record=diff_admission rows={row_count} prepare_ms={prepare_ms:.6} attach_ms={attach_ms:.6}");
    assert!(attach_ms <= 1.0, "diff attachment exceeded 1 ms: {attach_ms}");
    assert_eq!(session.document().as_text().as_lines().as_ptr(), text_ptr);
    assert_eq!(session.document().diff_metadata().unwrap().rows.as_ptr(), rows_ptr);
    assert_eq!(session.document().decorations().as_ptr(), decorations_ptr);
    assert_eq!(session.layout().height(), row_count as f64);
}

fn touch(state: TouchState, abs: Vec2d, time: f64) -> Event {
    Event::TouchUpdate(TouchUpdateEvent {
        time,
        window_id: WindowId(0, 0),
        modifiers: KeyModifiers::default(),
        touches: vec![TouchPoint {
            state,
            abs,
            time,
            uid: 1,
            rotation_angle: 0.0,
            force: 0.0,
            radius: dvec2(0.0, 0.0),
            handled: std::cell::Cell::new(Area::Empty),
            sweep_lock: std::cell::Cell::new(Area::Empty),
        }],
    })
}

/// Records the given editor with three lines far wider than its 480pt viewport.
fn wide_editor((mut cx, mut editor): (Cx, CodeEditor)) -> (Cx, CodeEditor, CodeSession) {
    editor.word_wrap = false;
    let mut session = CodeSession::new(doc(&["x".repeat(400), "y".repeat(400), "z".repeat(400)].join("\n")));
    record_editor(&mut cx, &mut editor, &mut session);
    (cx, editor, session)
}

#[test]
fn a_touch_drag_scrolls_instead_of_selecting() {
    let (mut cx, mut editor, mut session) = wide_editor(drag_scrolling_editor());
    let before = session.selections()[0];
    let (start, end) = (dvec2(240.0, 20.0), dvec2(140.0, 20.0));
    for (state, abs, time) in [
        (TouchState::Start, start, 0.0),
        (TouchState::Move, end, 0.05),
        (TouchState::Stop, end, 0.1),
    ] {
        editor.handle_event(&mut cx, &touch(state, abs, time), &mut Scope::empty(), &mut session);
        assert_eq!(session.selections()[0], before);
        assert!(matches!(editor.keep_cursor_in_view, KeepCursorInView::Off));
    }
    assert_eq!(editor.scroll_bars.get_scroll_pos().x, 100.0);
    assert!(!cx.fingers.is_drag_claimed(live_id_num!(touch, 1).into()));
    cx.action(());
    cx.handle_actions();
    assert!(!cx.has_key_focus(editor.area()));
}

#[test]
fn a_touch_drag_selects_like_the_mouse_without_drag_scrolling() {
    let (mut cx, mut editor, mut session) = wide_editor(editor());
    editor.handle_event(&mut cx, &touch(TouchState::Start, dvec2(240.0, 20.0), 0.0), &mut Scope::empty(), &mut session);
    assert!(cx.fingers.is_drag_claimed(live_id_num!(touch, 1).into()));
    let end = dvec2(140.0, 20.0 + 2.0 * editor.cell_size.y);
    editor.handle_event(&mut cx, &touch(TouchState::Move, end, 0.05), &mut Scope::empty(), &mut session);
    assert_eq!(session.selections()[0].cursor.position.line_index, 2);
    assert_eq!(editor.scroll_bars.get_scroll_pos(), dvec2(0.0, 0.0));
}

#[test]
fn a_touch_tap_places_the_cursor_like_a_click() {
    let abs = dvec2(240.0, 20.0);
    let (mut cx, mut editor, mut session) = wide_editor(drag_scrolling_editor());
    let before = session.selections()[0];
    editor.handle_event(&mut cx, &touch(TouchState::Start, abs, 0.0), &mut Scope::empty(), &mut session);
    assert_eq!(session.selections()[0], before);
    editor.handle_event(&mut cx, &touch(TouchState::Stop, abs, 0.1), &mut Scope::empty(), &mut session);
    assert!(matches!(editor.keep_cursor_in_view, KeepCursorInView::Off));
    cx.action(());
    cx.handle_actions();
    assert!(cx.has_key_focus(editor.area()));
    let tapped = session.selections()[0];

    let (mut cx, mut editor, mut session) = wide_editor(drag_scrolling_editor());
    let press = Event::MouseDown(MouseDownEvent {
        abs,
        button: MouseButton::PRIMARY,
        window_id: WindowId(0, 0),
        modifiers: KeyModifiers::default(),
        handled: std::cell::Cell::new(Area::Empty),
        time: 0.0,
    });
    editor.handle_event(&mut cx, &press, &mut Scope::empty(), &mut session);
    assert_eq!(session.selections()[0], tapped);
    assert_ne!(tapped, before);
}

#[test]
fn a_mouse_drag_selects_and_claims_the_drag_from_the_scroll_bars() {
    let (mut cx, mut editor, mut session) = wide_editor(drag_scrolling_editor());
    cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(0, 0)));
    let press = Event::MouseDown(MouseDownEvent {
        abs: dvec2(240.0, 20.0),
        button: MouseButton::PRIMARY,
        window_id: WindowId(0, 0),
        modifiers: KeyModifiers::default(),
        handled: std::cell::Cell::new(Area::Empty),
        time: 0.0,
    });
    editor.handle_event(&mut cx, &press, &mut Scope::empty(), &mut session);
    assert!(cx.fingers.is_drag_claimed(live_id!(mouse).into()));
    assert!(matches!(editor.keep_cursor_in_view, KeepCursorInView::Always(..)));
    let drag = Event::MouseMove(MouseMoveEvent {
        abs: dvec2(140.0, 20.0 + 2.0 * editor.cell_size.y),
        lock_delta: dvec2(0.0, 0.0),
        window_id: WindowId(0, 0),
        modifiers: KeyModifiers::default(),
        time: 0.05,
        handled: std::cell::Cell::new(Area::Empty),
    });
    editor.handle_event(&mut cx, &drag, &mut Scope::empty(), &mut session);
    assert_eq!(session.selections()[0].cursor.position.line_index, 2);
    assert_eq!(editor.scroll_bars.get_scroll_pos(), dvec2(0.0, 0.0));
}

#[test]
fn a_fit_editor_in_a_scroll_view_lays_out_every_line_but_draws_only_the_ones_in_view() {
    let (mut cx, mut editor) = editor();
    editor.word_wrap = false;
    editor.pad_left_top = dvec2(0.0, 0.0);
    let text: String = (0..3000).map(|i| format!("\"key_{i}\": {i},\n")).collect();
    let mut session = CodeSession::new(doc(&text));
    let line_count = session.document().as_text().as_lines().len();
    // The editor sits 100pt down a scroll view that shows at most 500pt.
    let mut draw = |cx: &mut Cx, editor: &mut CodeEditor, scroll_y: f64| {
        record_frame(cx, 600.0, |cx| {
            cx.begin_turtle(
                Walk { height: Size::Fit { min: None, max: Some(FitBound::Abs(500.0)) }, ..Walk::fill() },
                Layout::flow_down().with_scroll(dvec2(0.0, scroll_y)),
            );
            cx.walk_turtle(Walk::fixed(480.0, 100.0));
            editor.draw_walk_editor(cx, &mut session, Walk { height: Size::fit(), ..Walk::fill() });
            cx.end_turtle();
        })
    };

    draw(&mut cx, &mut editor, 0.0);
    let line_height = editor.cell_size.y;
    let text_height = line_count as f64 * line_height;
    assert!((editor.area().rect(&cx).size.y - text_height).abs() < 0.5);
    assert_eq!(editor.line_start, 0);
    assert!(editor.line_end as f64 * line_height < 400.0 + 3.0 * line_height);

    draw(&mut cx, &mut editor, 100.0 + text_height - 500.0);
    assert_eq!(editor.line_end, line_count);
    assert!((line_count - editor.line_start) as f64 * line_height < 500.0 + 3.0 * line_height);
}

#[test]
fn a_capped_fit_editor_scrolled_out_of_view_draws_nothing() {
    let (mut cx, mut editor) = editor();
    editor.word_wrap = false;
    let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
    let mut session = CodeSession::new(doc(&text));
    let capped = Walk { height: Size::Fit { min: None, max: Some(FitBound::Abs(300.0)) }, ..Walk::fill() };
    // The editor sits `above` the top of a 400pt scroll view scrolled down by `scroll_y`.
    let mut draw = |cx: &mut Cx, editor: &mut CodeEditor, above: f64, scroll_y: f64| {
        record_frame(cx, 400.0, |cx| {
            cx.begin_turtle(Walk::fill(), Layout::flow_down().with_scroll(dvec2(0.0, scroll_y)));
            cx.walk_turtle(Walk::fixed(480.0, above));
            editor.draw_walk_editor(cx, &mut session, capped);
            cx.end_turtle();
        })
    };

    // Scrolled past the editor, with the editor scrolled inside itself too.
    draw(&mut cx, &mut editor, 0.0, 0.0);
    editor.set_scroll_pos(&mut cx, dvec2(0.0, 1000.0));
    draw(&mut cx, &mut editor, 0.0, 600.0);
    assert_eq!(editor.line_start, editor.line_end);

    // Not scrolled down far enough to reach it.
    editor.set_scroll_pos(&mut cx, dvec2(0.0, 0.0));
    draw(&mut cx, &mut editor, 1000.0, 0.0);
    assert_eq!(editor.line_start, editor.line_end);
}

#[test]
fn a_second_finger_never_restarts_a_touch_drag() {
    let (mut cx, mut editor, mut session) = wide_editor(drag_scrolling_editor());
    let second = |state, abs: Vec2d, time| match touch(state, abs, time) {
        Event::TouchUpdate(mut e) => {
            e.touches[0].uid = 2;
            Event::TouchUpdate(e)
        }
        _ => unreachable!(),
    };
    editor.handle_event(&mut cx, &touch(TouchState::Start, dvec2(240.0, 20.0), 0.0), &mut Scope::empty(), &mut session);
    editor.handle_event(&mut cx, &touch(TouchState::Move, dvec2(200.0, 20.0), 0.05), &mut Scope::empty(), &mut session);
    editor.handle_event(&mut cx, &second(TouchState::Start, dvec2(100.0, 40.0), 0.1), &mut Scope::empty(), &mut session);
    editor.handle_event(&mut cx, &touch(TouchState::Move, dvec2(190.0, 20.0), 0.15), &mut Scope::empty(), &mut session);
    assert_eq!(editor.scroll_bars.get_scroll_pos().x, 50.0);
}

/// An editor that only scrolls sideways, and does so when a finger drags it.
fn sideways_editor() -> (Cx, CodeEditor) {
    editor_from(|vm| script_eval!(vm, {
        use mod.widgets.*
        CodeEditor{scroll_bars +: {show_scroll_y: false scroll_bar_x.drag_scrolling: true}}
    }))
}

/// A quick touch flick to the left, which leaves the text gliding.
fn flick_left(cx: &mut Cx, editor: &mut CodeEditor, session: &mut CodeSession) {
    for (state, x, time) in [
        (TouchState::Start, 240.0, 0.0),
        (TouchState::Move, 200.0, 0.01),
        (TouchState::Move, 140.0, 0.02),
        (TouchState::Stop, 140.0, 0.03),
    ] {
        editor.handle_event(cx, &touch(state, dvec2(x, 20.0), time), &mut Scope::empty(), session);
    }
    // The platform lets go of a finger once it lifts.
    cx.release_touch_captures(1);
}

#[test]
fn setting_the_scroll_stops_a_fling() {
    let press = touch(TouchState::Start, dvec2(240.0, 20.0), 0.05);

    let (mut cx, mut editor, mut session) = wide_editor(drag_scrolling_editor());
    flick_left(&mut cx, &mut editor, &mut session);
    assert!(editor.scroll_bars.catch_fling_on_press(&mut cx, &press), "the flick is still gliding");

    let (mut cx, mut editor, mut session) = wide_editor(drag_scrolling_editor());
    flick_left(&mut cx, &mut editor, &mut session);
    editor.set_scroll_pos(&mut cx, dvec2(0.0, 0.0));
    assert!(!editor.scroll_bars.catch_fling_on_press(&mut cx, &press));
}

#[test]
fn a_mouse_press_that_selects_still_stops_a_fling() {
    let (mut cx, mut editor, mut session) = wide_editor(drag_scrolling_editor());
    flick_left(&mut cx, &mut editor, &mut session);
    let mouse_press = Event::MouseDown(MouseDownEvent {
        abs: dvec2(240.0, 20.0),
        button: MouseButton::PRIMARY,
        window_id: WindowId(0, 0),
        modifiers: KeyModifiers::default(),
        handled: std::cell::Cell::new(Area::Empty),
        time: 0.05,
    });
    editor.handle_event(&mut cx, &mouse_press, &mut Scope::empty(), &mut session);
    let press = touch(TouchState::Start, dvec2(240.0, 20.0), 0.06);
    assert!(!editor.scroll_bars.catch_fling_on_press(&mut cx, &press));
}

#[test]
fn a_touch_on_code_leaves_the_axes_it_cant_scroll_to_a_scroller_around_it() {
    let (mut cx, mut editor, mut session) = wide_editor(sideways_editor());
    editor.handle_event(&mut cx, &touch(TouchState::Start, dvec2(240.0, 20.0), 0.0), &mut Scope::empty(), &mut session);
    let digit = live_id_num!(touch, 1).into();
    assert!(!cx.fingers.can_join_scroll_drag(digit, Vec2Index::X, Area::Empty));
    assert!(cx.fingers.can_join_scroll_drag(digit, Vec2Index::Y, Area::Empty));

    // Code that fits takes neither axis, but it still holds the press for a scroller around it.
    let (mut cx, mut editor) = sideways_editor();
    let mut session = CodeSession::new(doc("{}"));
    record_editor(&mut cx, &mut editor, &mut session);
    editor.handle_event(&mut cx, &touch(TouchState::Start, dvec2(20.0, 10.0), 0.0), &mut Scope::empty(), &mut session);
    assert!(cx.fingers.can_join_scroll_drag(digit, Vec2Index::X, Area::Empty));
    assert!(cx.fingers.can_join_scroll_drag(digit, Vec2Index::Y, Area::Empty));
}

#[test]
fn a_shared_drag_catches_up_with_the_finger_once_it_picks_the_code_axis() {
    let (mut cx, mut editor, mut session) = wide_editor(sideways_editor());
    editor.handle_event(&mut cx, &touch(TouchState::Start, dvec2(240.0, 20.0), 0.0), &mut Scope::empty(), &mut session);
    // A scroller around the code takes the other axis, so the drag waits to see which way it goes.
    assert!(cx.is_scroll_drag_owner(live_id_num!(touch, 1).into(), Vec2Index::Y, Area::Empty));
    editor.handle_event(&mut cx, &touch(TouchState::Move, dvec2(237.0, 20.0), 0.05), &mut Scope::empty(), &mut session);
    assert_eq!(editor.scroll_bars.get_scroll_pos().x, 0.0);
    editor.handle_event(&mut cx, &touch(TouchState::Move, dvec2(200.0, 21.0), 0.1), &mut Scope::empty(), &mut session);
    assert_eq!(editor.scroll_bars.get_scroll_pos().x, 40.0);
}

#[test]
fn a_wrapped_fit_editor_draws_every_line_on_its_first_frame() {
    let (mut cx, mut editor) = editor();
    editor.word_wrap = true;
    editor.pad_left_top = dvec2(0.0, 0.0);
    let text: String = (0..30).map(|i| format!("{i} {}\n", "word ".repeat(60))).collect();
    let mut session = CodeSession::new(doc(&text));
    let line_count = session.document().as_text().as_lines().len();
    record_frame(&mut cx, 5000.0, |cx| {
        editor.draw_walk_editor(cx, &mut session, Walk { height: Size::fit(), ..Walk::fill() });
    });
    assert!(session.layout().height() > line_count as f64, "the lines wrapped");
    assert_eq!(editor.line_end, line_count);
    assert!((editor.area().rect(&cx).size.y - session.layout().height() * editor.cell_size.y).abs() < 0.5);
}
