use super::*;
use makepad_civil_time::from_ymd;
use makepad_widgets::makepad_platform::storage::{StorageError, StorageList, StorageOp};
use makepad_widgets::stack_navigation::StackNavigationTransitionAction;

fn root() -> (Cx, WidgetRef) {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    let root = cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        crate::script_mod(vm);
        let value = script_eval!(vm, { use mod.widgets.* RemindersView{} });
        let root = WidgetRef::script_from_value(vm, value);
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "{errors:?}");
        root
    });
    {
        let mut view = root.borrow_mut::<RemindersView>().unwrap();
        view.started = true;
        view.now = Now {
            day: from_ymd(2026, 9, 9),
            minute: 720,
        };
        view.document = Some(seed(view.now.day));
        view.machine.phase = StoragePhase::Ready;
        view.machine.saved_revision = 1;
        view.rebuild(&mut cx);
    }
    (cx, root)
}

fn open_twelve(view: &mut RemindersView, cx: &mut Cx) -> Reminder {
    let original = view
        .document
        .as_ref()
        .unwrap()
        .reminder(12)
        .unwrap()
        .clone();
    view.open_draft(cx, crate::engine::draft_from(&original), Some(12));
    original
}

fn finish_push(view: &mut RemindersView, cx: &mut Cx, id: LiveId) {
    let nav = view.view.stack_navigation(cx, ids!(compact.nav));
    assert_eq!(nav.destination_view(), Some(id));
    let child = nav.view_by_id(cx, id);
    let actions = cx.capture_actions(|cx| {
        cx.widget_action(
            child.widget_uid(),
            StackNavigationTransitionAction::ShowDone,
        );
    });
    view.handle_event(cx, &Event::Actions(actions), &mut Scope::empty());
}

#[test]
fn compact_done_preserves_every_unedited_field() {
    let (mut cx, root) = root();
    let mut view = root.borrow_mut::<RemindersView>().unwrap();
    view.apply_layout(&mut cx, dvec2(402.0, 780.0));
    let original = open_twelve(&mut view, &mut cx);
    let form = view.form_parent(&mut cx, true);
    assert_eq!(
        form.text_input(&mut cx, ids!(fields.text_group.notes))
            .text(),
        original.notes
    );
    assert_eq!(
        form.text_input(&mut cx, ids!(fields.schedule.date)).text(),
        "2026-09-09"
    );
    assert_eq!(
        form.text_input(&mut cx, ids!(fields.schedule.time)).text(),
        "10:00"
    );
    assert_eq!(
        form.drop_down(&mut cx, ids!(fields.organization.list))
            .selected_item(),
        1
    );
    assert!(form
        .check_box(&mut cx, ids!(fields.organization.flag))
        .active(&mut cx));
    form.text_input(&mut cx, ids!(fields.text_group.title))
        .set_text(&mut cx, "Edited compact title");
    view.commit_draft(&mut cx);
    assert!(view.draft.is_none());
    assert_eq!(
        view.document.as_ref().unwrap().reminder(12).unwrap(),
        &Reminder {
            title: "Edited compact title".into(),
            ..original
        }
    );
    assert_eq!(view.document.as_ref().unwrap().revision, 2);
}

#[test]
fn resize_preserves_raw_fields_selection_filter_and_scroll_then_cancel_discards() {
    let (mut cx, root) = root();
    let mut view = root.borrow_mut::<RemindersView>().unwrap();
    view.open_filter(&mut cx, Filter::List(WORK_LIST_ID));
    open_twelve(&mut view, &mut cx);
    let before = view.document.clone();
    let form = view.form_parent(&mut cx, false);
    form.text_input(&mut cx, ids!(fields.text_group.title))
        .set_text(&mut cx, "Unsaved title");
    form.text_input(&mut cx, ids!(fields.text_group.notes))
        .set_text(&mut cx, "Notes\nwith a second line");
    form.text_input(&mut cx, ids!(fields.schedule.date))
        .set_text(&mut cx, "2026-09-");
    form.text_input(&mut cx, ids!(fields.schedule.time))
        .set_text(&mut cx, "17:");
    form.drop_down(&mut cx, ids!(fields.organization.list))
        .set_selected_item(&mut cx, 3);
    form.check_box(&mut cx, ids!(fields.organization.flag))
        .set_active(&mut cx, false, Animate::No);
    form.widget(&mut cx, ids!(fields.organization.priority))
        .borrow_mut::<GlassSegmented>()
        .unwrap()
        .set_selected(&mut cx, 1);
    view.list_parent(&mut cx, false)
        .portal_list(&mut cx, ids!(rows))
        .set_first_id_and_scroll(3, -12.0);
    for size in [
        dvec2(402.0, 780.0),
        dvec2(874.0, 300.0),
        dvec2(1240.0, 800.0),
        dvec2(402.0, 780.0),
    ] {
        view.apply_layout(&mut cx, size);
        let form = view.form_parent(&mut cx, view.nav.layout.is_compact());
        assert_eq!(
            form.text_input(&mut cx, ids!(fields.text_group.title))
                .text(),
            "Unsaved title"
        );
        assert_eq!(
            form.text_input(&mut cx, ids!(fields.text_group.notes))
                .text(),
            "Notes\nwith a second line"
        );
        assert_eq!(
            form.text_input(&mut cx, ids!(fields.schedule.date)).text(),
            "2026-09-"
        );
        assert_eq!(
            form.text_input(&mut cx, ids!(fields.schedule.time)).text(),
            "17:"
        );
        assert_eq!(
            form.drop_down(&mut cx, ids!(fields.organization.list))
                .selected_item(),
            3
        );
        assert!(!form
            .check_box(&mut cx, ids!(fields.organization.flag))
            .active(&mut cx));
        assert_eq!(
            form.widget(&mut cx, ids!(fields.organization.priority))
                .borrow::<GlassSegmented>()
                .unwrap()
                .selected(),
            1
        );
        let list = view
            .list_parent(&mut cx, view.nav.layout.is_compact())
            .portal_list(&mut cx, ids!(rows));
        assert_eq!(
            (
                list.borrow().unwrap().first_id(),
                list.borrow().unwrap().first_scroll()
            ),
            (3, -12.0)
        );
        assert_eq!(view.nav.selected, Some(12));
        assert_eq!(view.nav.filter, Filter::List(WORK_LIST_ID));
        assert_eq!(view.nav.current(), Route::Detail);
    }
    view.commit_draft(&mut cx);
    assert!(view.field_error.is_some());
    assert_eq!(view.document, before);
    assert!(view.draft.is_some());
    view.discard_draft(&mut cx);
    assert_eq!(view.document, before);
    assert!(view.draft_fields.is_none());
    assert_eq!(view.nav.current(), Route::List(Filter::List(WORK_LIST_ID)));
    open_twelve(&mut view, &mut cx);
    assert_eq!(
        view.draft_fields.as_ref().unwrap().title,
        "Review release checklist"
    );
}

#[test]
fn home_new_and_wide_draft_reach_detail_after_both_stack_transitions() {
    for from_home in [true, false] {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<RemindersView>().unwrap();
        if from_home {
            view.nav.resize(crate::engine::LayoutMode::Compact);
            view.nav.open_home();
            view.rebuild(&mut cx);
            view.open_draft(&mut cx, blank_draft(&home_create_defaults()), None);
        } else {
            open_twelve(&mut view, &mut cx);
            view.apply_layout(&mut cx, dvec2(402.0, 780.0));
        }
        assert_eq!(view.visual_route, Route::List(view.nav.filter));
        assert!(view.queued_nav);
        finish_push(&mut view, &mut cx, live_id!(list_view));
        assert_eq!(view.visual_route, Route::Detail);
        finish_push(&mut view, &mut cx, live_id!(detail_view));
        let nav = view.view.stack_navigation(&mut cx, ids!(compact.nav));
        assert_eq!(nav.current_view(), Some(live_id!(detail_view)));
        assert!(!nav.is_transitioning());
        view.discard_draft(&mut cx);
        assert_eq!(nav.destination_view(), Some(live_id!(list_view)));
        let child = nav.view_by_id(&mut cx, live_id!(detail_view));
        let actions = cx.capture_actions(|cx| {
            cx.widget_action(
                child.widget_uid(),
                StackNavigationTransitionAction::HideEnd(nav.widget_uid()),
            )
        });
        view.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::empty());
        assert_eq!(nav.current_view(), Some(live_id!(list_view)));
        assert!(!nav.is_transitioning());
    }
}

#[test]
fn enabling_time_validates_and_preserves_the_typed_date() {
    let (mut cx, root) = root();
    let mut view = root.borrow_mut::<RemindersView>().unwrap();
    let reminder = view.document.as_ref().unwrap().reminder(1).unwrap().clone();
    view.open_draft(&mut cx, crate::engine::draft_from(&reminder), Some(1));
    let form = view.form_parent(&mut cx, false);
    form.text_input(&mut cx, ids!(fields.schedule.date))
        .set_text(&mut cx, "2026-09-20");
    view.toggle_time(&mut cx, true);
    assert_eq!(
        form.text_input(&mut cx, ids!(fields.schedule.date)).text(),
        "2026-09-20"
    );
    assert_eq!(
        form.text_input(&mut cx, ids!(fields.schedule.time)).text(),
        "09:00"
    );
    assert_eq!(
        view.read_draft_from_form(&mut cx).unwrap().value.due,
        Some(Due {
            day: from_ymd(2026, 9, 20),
            minute: Some(540)
        })
    );
    view.toggle_time(&mut cx, false);
    form.text_input(&mut cx, ids!(fields.schedule.date))
        .set_text(&mut cx, "not a date");
    view.toggle_time(&mut cx, true);
    assert!(view.field_error.is_some());
    assert!(!form
        .check_box(&mut cx, ids!(fields.schedule.time_on))
        .active(&mut cx));
    assert_eq!(
        form.text_input(&mut cx, ids!(fields.schedule.date)).text(),
        "not a date"
    );
    assert_eq!(
        view.document.as_ref().unwrap().reminder(1).unwrap(),
        &reminder
    );
}

fn response(
    id: StorageRequestId,
    op: StorageOp,
    result: Result<StorageResult, StorageError>,
) -> StorageResponse {
    StorageResponse {
        request_id: id,
        namespace: "reminders.test".into(),
        op,
        result,
    }
}

#[test]
fn first_run_seed_waits_for_ack_and_reloads_absolute_dates() {
    let (mut cx, root) = root();
    let mut view = root.borrow_mut::<RemindersView>().unwrap();
    view.document = None;
    view.machine = StorageMachine::default();
    view.set_storage(cx.storage("reminders.test"));
    view.machine.start();
    view.get_id = Some(StorageRequestId(900));
    view.on_storage(
        &mut cx,
        &[response(
            StorageRequestId(899),
            StorageOp::Get,
            Ok(StorageResult::Value(None)),
        )],
    );
    assert!(view.document.is_none());
    let mut foreign = response(
        StorageRequestId(900),
        StorageOp::Get,
        Ok(StorageResult::Value(None)),
    );
    foreign.namespace = "some-other-app".into();
    view.on_storage(&mut cx, &[foreign]);
    view.on_storage(
        &mut cx,
        &[response(
            StorageRequestId(900),
            StorageOp::Set,
            Ok(StorageResult::Unit),
        )],
    );
    assert_eq!(view.get_id, Some(StorageRequestId(900)));
    assert_eq!(view.machine.phase, StoragePhase::GetInFlight);
    view.on_storage(
        &mut cx,
        &[response(
            StorageRequestId(900),
            StorageOp::Get,
            Ok(StorageResult::Value(None)),
        )],
    );
    assert!(view.document.is_none());
    let list_id = view.list_id.unwrap();
    view.on_storage(
        &mut cx,
        &[response(
            list_id,
            StorageOp::List,
            Ok(StorageResult::List(StorageList {
                keys: vec![],
                next_cursor: None,
            })),
        )],
    );
    let seed = view.document.clone().unwrap();
    assert_eq!(view.machine.saved_revision, 0);
    assert_eq!(view.machine.dirty_revision, 1);
    let set_id = view.set_id.expect("first seed must issue set immediately");
    let call = ServiceCall {
        call_id: "seed".into(),
        tool: "due".into(),
        args: r#"{"days":7}"#.into(),
    };
    let result = view.ai_answer(&call);
    assert_eq!(
        makepad_strict_json::parse(result.data.as_bytes())
            .unwrap()
            .get("saved_revision")
            .unwrap()
            .as_i64(),
        Some(0)
    );
    let bytes = encode(&seed).unwrap();
    view.on_storage(
        &mut cx,
        &[response(set_id, StorageOp::Set, Ok(StorageResult::Unit))],
    );
    assert_eq!(view.machine.saved_revision, 1);
    assert!(view.set_id.is_none());
    view.document = None;
    view.machine = StorageMachine::default();
    view.machine.start();
    view.get_id = Some(StorageRequestId(901));
    view.now.day += 1;
    view.on_storage(
        &mut cx,
        &[response(
            StorageRequestId(901),
            StorageOp::Get,
            Ok(StorageResult::Value(Some(bytes))),
        )],
    );
    assert_eq!(view.document, Some(seed));
    assert_eq!(view.machine.saved_revision, 1);
    assert!(view.set_id.is_none());
}

#[test]
fn root_storage_errors_remain_visible_and_coalesce_edits_until_retry_ack() {
    let (mut cx, root) = root();
    let mut view = root.borrow_mut::<RemindersView>().unwrap();
    view.set_storage(cx.storage("reminders.test"));
    view.mutate(
        &mut cx,
        Command::SetFlagged {
            id: 1,
            flagged: true,
        },
    );
    let first = view.set_id.unwrap();
    view.on_storage(
        &mut cx,
        &[response(
            first,
            StorageOp::Set,
            Err(StorageError::Io("full".into())),
        )],
    );
    assert!(view.machine.error.is_some());
    assert!(view.set_id.is_none());
    open_twelve(&mut view, &mut cx);
    view.mutate(
        &mut cx,
        Command::SetFlagged {
            id: 2,
            flagged: true,
        },
    );
    view.persist(&mut cx);
    assert_eq!(view.machine.saved_revision, 1);
    assert_eq!(view.machine.dirty_revision, 3);
    assert!(view.set_id.is_none());
    assert!(view.machine.error.is_some());
    assert!(view.draft.is_some());
    assert!(view.view.view(&mut cx, ids!(save_strip)).visible());
    assert!(view.machine.retry_save());
    view.persist(&mut cx);
    let retry = view.set_id.unwrap();
    view.mutate(
        &mut cx,
        Command::SetFlagged {
            id: 3,
            flagged: true,
        },
    );
    assert_eq!(view.set_id, Some(retry));
    view.on_storage(
        &mut cx,
        &[response(first, StorageOp::Set, Ok(StorageResult::Unit))],
    );
    assert_eq!(view.machine.saved_revision, 1);
    view.on_storage(
        &mut cx,
        &[response(retry, StorageOp::Set, Ok(StorageResult::Unit))],
    );
    assert_eq!(view.machine.saved_revision, 3);
    let latest = view.set_id.unwrap();
    view.on_storage(
        &mut cx,
        &[response(latest, StorageOp::Set, Ok(StorageResult::Unit))],
    );
    assert_eq!(view.machine.saved_revision, 4);
    assert!(view.set_id.is_none());
    assert!(view.machine.error.is_none());
    assert!(view.draft.is_some());
}

#[test]
fn shutdown_releases_the_root_timer_idempotently() {
    let (mut cx, root) = root();
    let mut view = root.borrow_mut::<RemindersView>().unwrap();
    view.started = false;
    view.ensure_started(&mut cx);
    assert!(view.tick.is_some());
    view.shutdown(&mut cx);
    assert!(view.tick.is_none());
    view.shutdown(&mut cx);
    assert!(view.tick.is_none());
}

#[test]
fn geometry_sizes_completed_tiles_and_wrapped_rows_for_each_mode() {
    let (mut cx, root) = root();
    let mut view = root.borrow_mut::<RemindersView>().unwrap();
    for (size, tile_height, list_height) in [
        (dvec2(1240.0, 800.0), 52.0, 44.0),
        (dvec2(402.0, 780.0), 56.0, 56.0),
        (dvec2(874.0, 300.0), 44.0, 44.0),
    ] {
        view.apply_layout(&mut cx, size);
        view.rebuild(&mut cx);
        let compact = view.nav.layout.is_compact();
        let home = view.home_parent(&mut cx, compact);
        let tile = home.widget(&mut cx, ids!(completed));
        assert_eq!(tile.walk(&mut cx).height, Size::Fixed(tile_height));
        assert_eq!(tile.label(&mut cx, ids!(body.top.name)).text(), "Completed");
        for path in [ids!(body.top.count), ids!(body.top.name)] {
            let label = tile.label(&mut cx, path);
            let label = label.borrow().unwrap();
            let measured = label.draw_text.layout(
                &mut cx,
                0.0,
                0.0,
                None,
                false,
                Align::default(),
                &label.text(),
            );
            assert!(
                measured.size_in_lpxs.height as f64 <= tile_height,
                "Completed content must fit"
            );
        }
        for path in [ids!(today), ids!(scheduled), ids!(all), ids!(flagged)] {
            let tile = home.widget(&mut cx, path);
            let tile_height = match tile.walk(&mut cx).height {
                Size::Fixed(h) => h,
                _ => panic!("tile height"),
            };
            let body = tile.view(&mut cx, ids!(body));
            let padding = body.borrow().unwrap().layout.padding;
            let top = tile.view(&mut cx, ids!(body.top));
            let top_height = match top.walk(&mut cx).height {
                Size::Fixed(h) => h,
                _ => panic!("top height"),
            };
            let count = tile.label(&mut cx, ids!(body.top.count));
            let count = count.borrow().unwrap();
            let count_h = count
                .draw_text
                .layout(&mut cx, 0.0, 0.0, None, false, Align::default(), "2000")
                .size_in_lpxs
                .height as f64;
            assert!(count_h <= top_height, "count: {count_h} > {top_height}");
            let name = tile.label(&mut cx, ids!(body.name));
            let name = name.borrow().unwrap();
            let name_h = name
                .draw_text
                .layout(
                    &mut cx,
                    0.0,
                    0.0,
                    None,
                    false,
                    Align::default(),
                    &name.text(),
                )
                .size_in_lpxs
                .height as f64;
            assert!(
                top_height + name_h + padding.top + padding.bottom <= tile_height,
                "tile name must fit below the count"
            );
        }

        assert_eq!(
            home.widget(&mut cx, ids!(lists.home)).walk(&mut cx).height,
            Size::Fixed(list_height)
        );
        let list_parent = view.list_parent(&mut cx, compact);
        let portal = list_parent.portal_list(&mut cx, ids!(rows));
        let row = portal
            .borrow_mut()
            .unwrap()
            .item(&mut cx, 0, live_id!(Item));
        let mut item = project(view.document.as_ref().unwrap(), Filter::Today, view.now).groups[0]
            .items[0]
            .clone();
        item.title = "Short".into();
        let short = view.measure_row(&mut cx, &row, &item, 300.0);
        item.title =
            "A reminder title which must wrap over two lines at the available narrow width".into();
        let wrapped = view.measure_row(&mut cx, &row, &item, 300.0);
        assert!(
            wrapped > short,
            "a wrapped title must increase row height: {short} → {wrapped}"
        );
        let text_height: f64 = [ids!(text.title), ids!(text.notes), ids!(text.metadata)]
            .iter()
            .map(
                |path| match row.widget(&mut cx, *path).walk(&mut cx).height {
                    Size::Fixed(h) => h,
                    _ => panic!("measured label height"),
                },
            )
            .sum();
        assert!(wrapped >= text_height + if compact { 20.0 } else { 16.0 });
        if view.nav.layout.is_short() {
            assert!(!list_parent.view(&mut cx, ids!(heading)).visible());
            assert!(list_parent
                .label(&mut cx, ids!(toolbar.short_title))
                .visible());
            let pop = view.view.widget(&mut cx, ids!(wide.overlay.popover));
            let walk = pop.walk(&mut cx);
            assert_eq!(walk.height, Size::Fixed(268.0));
            assert_eq!(walk.width, Size::Fixed(560.0));
            assert_eq!(walk.abs_pos, Some(dvec2(157.0, 16.0)));
        }
    }
}

#[test]
fn picker_options_have_full_44_point_hit_targets() {
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;

    let (mut cx, root) = root();
    let mut view = root.borrow_mut::<RemindersView>().unwrap();
    open_twelve(&mut view, &mut cx);
    for size in [
        dvec2(1240.0, 800.0),
        dvec2(402.0, 780.0),
        dvec2(874.0, 300.0),
    ] {
        view.apply_layout(&mut cx, size);
        let picker = view
            .form_parent(&mut cx, view.nav.layout.is_compact())
            .widget(&mut cx, ids!(fields.organization.list));
        assert_eq!(picker.walk(&mut cx).height, Size::Fixed(44.0));
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, size);
        let mut draw_list = DrawList2d::new(&mut cx);
        let overlay = cx.with_vm(Overlay::script_new);
        picker.borrow_mut::<DropDown>().unwrap().set_active(&mut cx);
        {
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            draw_list.begin_always(&mut cx2d);
            overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            picker.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            overlay.end(&mut cx2d);
            draw_list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }
        let mut rows = Vec::new();
        picker.children(&mut |id, item| rows.push((id, item)));
        rows.sort_by_key(|(id, _)| id.0);
        assert_eq!(
            rows.len(),
            4,
            "the actual dropdown must expose all four options"
        );
        let mut previous_bottom = None;
        for ((_, row), label) in rows.iter().zip(["Groceries", "Work", "Home", "Travel"]) {
            assert_eq!(row.borrow::<PopupMenuItem>().unwrap().label, label);
            let rect = row.area().clipped_rect(&cx);
            assert!(rect.size.x >= 44.0, "{size:?} {label}: {rect:?}");
            assert_eq!(rect.size.y, 44.0, "{size:?} {label}: {rect:?}");
            if let Some(bottom) = previous_bottom {
                assert_eq!(rect.pos.y, bottom, "picker hit targets must be contiguous");
            }
            previous_bottom = Some(rect.pos.y + rect.size.y);
        }
        picker.borrow_mut::<DropDown>().unwrap().set_closed(&mut cx);
    }
    cx.with_vm(|vm| {
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "picker layout and drawing: {errors:?}");
    });
}

#[test]
fn theme_roles_come_from_each_roots_owning_vm() {
    use makepad_widgets::widget_async::{enter_isolate, leave_isolate};
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    let mut roots = Vec::new();
    for colour in [vec4(0.1, 0.2, 0.3, 1.0), vec4(0.8, 0.7, 0.6, 1.0)] {
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let root = cx.with_script_vm_id_trusted(vm_id, |vm| {
            makepad_widgets::script_mod(vm);
            script_eval!(vm,{
                mod.theme.color_text = #(colour)
                mod.theme.color_focus = #(colour)
                mod.theme.color_bg_highlight = #(colour)
                mod.theme.color_error = #(colour)
            });
            crate::script_mod(vm);
            let value = script_eval!(vm,{use mod.widgets.* RemindersView{}});
            let root = WidgetRef::script_from_value(vm, value);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{errors:?}");
            root
        });
        roots.push((root, colour, vm_id));
    }
    for (root, colour, vm_id) in roots {
        let entry = enter_isolate(&mut cx, vm_id);
        {
            let mut view = root.borrow_mut::<RemindersView>().unwrap();
            view.refresh_theme(&mut cx);
            assert_eq!(view.theme_color("today"), colour);
            assert_eq!(view.theme_ink, colour);
            assert_eq!(view.theme_selected, colour);
            assert_eq!(view.theme_scheduled, colour);
            view.document = Some(seed(from_ymd(2026, 9, 9)));
            view.rebuild(&mut cx);
            view.open_filter(&mut cx, Filter::List(2));
            let title = view
                .list_parent(&mut cx, false)
                .label(&mut cx, ids!(heading.title));
            assert_eq!(title.borrow().unwrap().draw_text.color, colour);
            let updated = vec4(0.2, 0.3, 0.4, 1.0);
            cx.with_script_vm_id(vm_id, |vm| {
                script_eval!(vm,{mod.theme.color_focus = #(updated)});
            });
            view.rebuild(&mut cx);
            let title = view
                .list_parent(&mut cx, false)
                .label(&mut cx, ids!(heading.title));
            assert_eq!(title.borrow().unwrap().draw_text.color, updated);
        }
        leave_isolate(&mut cx, entry);
    }
}
