use super::*;
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
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let editor = cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        crate::script_mod(vm);
        vm.bx.captured_errors = Some(Vec::new());
        let value = script_eval!(vm, { use mod.widgets.* CodeEditor{} });
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
    use std::sync::{Arc, Mutex};
    let (mut cx, mut editor) = editor();
    let mut session = CodeSession::new(doc("unchanged"));
    editor.blink_timer = cx.start_timeout(0.5);
    editor.set_read_only(&mut cx, true);
    assert_eq!(editor.blink_timer.0, 0);
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
    let pass = DrawPass::new(cx);
    pass.set_size(cx, dvec2(480.0, 200.0));
    let mut list = DrawList2d::new(cx);
    let event = DrawEvent::default();
    let mut draw = CxDraw::new(cx, &event);
    let mut cx = Cx2d::new(&mut draw);
    cx.begin_pass(&pass, Some(1.0));
    list.begin_always(&mut cx);
    cx.begin_root_turtle(dvec2(480.0, 200.0), Layout::default());
    editor.draw_walk_editor(&mut cx, session, Walk::fixed(480.0, 200.0));
    cx.end_turtle();
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
