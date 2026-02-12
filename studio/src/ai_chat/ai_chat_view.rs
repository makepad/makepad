use {
    crate::{
        ai_chat::ai_chat_manager::*,
        app::{AppAction, AppData},
        file_system::file_system::{EditSession, OpenDocument},
        makepad_widgets::widget_tree::CxWidgetExt,
        makepad_widgets::*,
    },
    std::env,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AiChatViewBase = #(AiChatView::register_widget(vm))

    let User = mod.widgets.RoundedView {
        height: Fit
        flow: Down
        margin: theme.mspace_3
        padding: theme.mspace_2
        padding.top: theme.space_1 + 4
        padding.bottom: theme.space_2
        draw_bg +: { color: theme.color_bg_highlight }

        mod.widgets.View {
            height: Fit
            width: Fill
            flow: Right
            align: Align{ x: 0. y: 0. }
            spacing: theme.space_3
            padding: Inset{ left: theme.space_1 right: theme.space_1 top: theme.space_1 - 1 }
            margin: Inset{ bottom: -5. }

            mod.widgets.View { width: Fill }
        }

        mod.widgets.View {
            height: Fit
            width: Fill

            message_input := TextInput {
                width: Fill
                height: Fit
                empty_text: "Enter prompt"
            }
        }
    }

    let Assistant = mod.widgets.RoundedView {
        flow: Down
        margin: theme.mspace_h_3
        padding: theme.mspace_h_2
        padding.bottom: theme.space_2

        draw_bg +: {
            color: theme.color_inset
        }

        busy := View {
            width: 70
            height: 10
            margin: Inset{top: 10. bottom: 0}
            padding: 0.
            show_bg: true
            draw_bg +: {
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    let x = 0.
                    for i in 0..5 {
                        x = x + 8.
                        sdf.circle(x, 5., 2.5)
                        sdf.fill(theme.color_makepad)
                    }
                    return sdf.result
                }
            }
        }

        md := Markdown {
            code_block := View {
                width: Fill
                height: Fit
                flow: Overlay
                code_view := CodeView {
                    keep_cursor_at_end: true
                    editor +: {
                        height: 200
                        draw_bg +: { color: theme.color_d_hidden }
                    }
                }
                mod.widgets.View {
                    width: Fill
                    height: Fit
                    align: Align{ x: 1.0 }

                    run_button := ButtonFlat {
                        width: Fit
                        height: Fit
                        padding: theme.mspace_2
                        margin: 0.
                        icon_walk: Walk{
                            width: 12
                            height: Fit
                            margin: Inset{ left: 10 }
                        }
                        text: ""
                        draw_icon +: {
                            color: theme.color_u_4
                            svg_file: crate_resource("self://resources/icons/icon_run.svg")
                        }
                        icon_walk: Walk{ width: 9. }
                    }
                    copy_button := ButtonFlat {
                        width: Fit
                        height: Fit
                        padding: theme.mspace_2
                        margin: Inset{top: 0 right: 20}
                        text: ""
                        icon_walk: Walk{
                            width: 12
                            height: Fit
                            margin: Inset{ left: 10 }
                        }
                        draw_icon +: {
                            color: theme.color_u_4
                            svg_file: crate_resource("self://resources/icons/icon_copy.svg")
                        }
                    }
                }
            }
            use_code_block_widget: true
            body: ""
        }
    }

    mod.widgets.AiChatView = set_type_default() do mod.widgets.AiChatViewBase {
        flow: Down
        height: Fill
        width: Fill
        spacing: theme.space_1
        show_bg: true
        draw_bg +: { color: theme.color_d_1 }

        tb := RectShadowView {
            width: Fill
            height: 38.
            flow: Down
            align: Align{ x: 0. y: 0. }
            margin: Inset{ top: -1. }
            padding: theme.mspace_2
            spacing: 0.
            draw_bg +: {
                border_size: 0.0
                border_color: theme.color_bevel_outset_1
                shadow_color: theme.color_shadow
                shadow_radius: 5.0
                shadow_offset: vec2(0.0, 1.0)
                color: theme.color_fg_app
            }
            content := View {
                height: Fill
                width: Fill
                flow: Right
                padding: Inset{top: 1}
                align: Align{ x: 0.0 y: 0.5}
                margin: theme.mspace_h_2
                spacing: theme.space_2

                auto_run := CheckBoxCustom {
                    text: "Auto"
                    align: Align{ y: 0.5 }
                    spacing: theme.space_1
                    padding: theme.mspace_v_2
                    icon_walk: Walk{ width: 10. }
                    draw_icon +: {
                        color: theme.color_label_outer
                        svg_file: crate_resource("self://resources/icons/icon_auto.svg")
                    }
                }

                mod.widgets.View {
                    flow: Right
                    width: Fit
                    height: Fit
                    spacing: theme.space_1

                    Pbold { width: Fit text: "Model" margin: 0. padding: theme.mspace_v_1 }
                    model_dropdown := DropDownFlat { width: Fit }
                }

                mod.widgets.View {
                    flow: Right
                    width: Fit
                    height: Fit
                    spacing: theme.space_1

                    Pbold { width: Fit text: "Context" margin: 0. padding: theme.mspace_v_1 }
                    context_dropdown := DropDownFlat { width: Fit }
                }

                mod.widgets.View {
                    flow: Right
                    width: Fit
                    height: Fit
                    spacing: theme.space_1

                    Pbold { width: Fit text: "Project" margin: 0. padding: theme.mspace_v_1 }
                    project_dropdown := DropDownFlat { width: Fit }
                }

                mod.widgets.View { width: Fill }

                history_left := ButtonFlatter {
                    width: Fit
                    draw_bg +: { color_focus: #0000 }
                    padding: theme.mspace_1
                    draw_icon +: {
                        svg_file: crate_resource("self://resources/icons/icon_history_rew.svg")
                    }
                    icon_walk: Walk{ width: 5. }
                }

                slot := Label {
                    draw_text +: {
                        color: theme.color_u_4
                    }
                    width: Fit
                    text: "0"
                }

                history_right := ButtonFlatter {
                    width: Fit
                    padding: theme.mspace_1
                    draw_bg +: { color_focus: #0000 }
                    draw_icon +: {
                        svg_file: crate_resource("self://resources/icons/icon_history_ff.svg")
                    }
                    icon_walk: Walk{ width: 5. }
                }

                history_delete := ButtonFlatter {
                    width: Fit
                    text: ""
                    draw_bg +: { color_focus: #0000 }
                    draw_icon +: {
                        svg_file: crate_resource("self://resources/icons/icon_del.svg")
                    }
                    icon_walk: Walk{ width: 10. }
                }

                stop_button := ButtonFlatter {
                    width: Fit
                    text: ""
                    visible: false
                    draw_bg +: { color_focus: #0000 }
                    draw_icon +: {
                        color: theme.color_error
                        svg_file: crate_resource("self://resources/icons/icon_times.svg")
                    }
                    icon_walk: Walk{ width: 10. }
                }
            }
        }

        list := PortalList {
            drag_scrolling: false
            max_pull_down: 0.0
            auto_tail: true
            User := User {}
            Assistant := Assistant {}
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct AiChatView {
    #[deref]
    view: View,
    #[rust]
    initialised: bool,
    #[rust]
    history_slot: usize,
}

impl AiChatView {
    fn handle_own_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        let data = scope.data.get_mut::<AppData>().unwrap();
        let path = cx.widget_tree().path_to(self.widget_uid());
        let session_id = path.last().copied().unwrap_or(LiveId(0));

        if let Some(EditSession::AiChat(chat_id)) = data.file_system.get_session_mut(session_id) {
            let chat_id = *chat_id;
            if let Some(OpenDocument::AiChat(doc)) =
                data.file_system.open_documents.get_mut(&chat_id)
            {
                if let Some(value) = self.check_box(cx, ids!(auto_run)).changed(actions) {
                    doc.auto_run = value;
                }

                // items with actions
                let chat_list = self.view.portal_list(cx, ids!(list));
                for (item_id, item) in chat_list.items_with_actions(&actions) {
                    if item.button(cx, ids!(copy_button)).pressed(actions) {
                        //let code_view = item.widget(cx, ids!(code_view));
                    }
                    if item.button(cx, ids!(run_button)).pressed(actions) {
                        cx.action(AppAction::RunAiChat {
                            chat_id,
                            history_slot: self.history_slot,
                            item_id,
                        });
                    }
                }

                if self.button(cx, ids!(history_left)).pressed(actions) {
                    // first we check if our messages are the same as 'slot'.
                    // if not, we should create an undo item first
                    self.history_slot = self.history_slot.saturating_sub(1);
                    cx.action(AppAction::RedrawAiChat { chat_id });
                }
                if self.button(cx, ids!(history_right)).pressed(actions) {
                    self.history_slot =
                        (self.history_slot + 1).min(doc.file.history.len().saturating_sub(1));
                    cx.action(AppAction::RedrawAiChat { chat_id });
                }
                if self.button(cx, ids!(history_delete)).pressed(actions) {
                    doc.file.remove_slot(cx, &mut self.history_slot);
                    cx.action(AppAction::RedrawAiChat { chat_id });
                    cx.action(AppAction::SaveAiChat { chat_id });
                }
                if self.button(cx, ids!(stop_button)).pressed(actions) {
                    cx.action(AppAction::CancelAiGeneration { chat_id });
                }

                if let Some(ctx_id) = self.drop_down(cx, ids!(context_dropdown)).selected(actions) {
                    let ctx_name = &data.ai_chat_manager.contexts[ctx_id].name;
                    doc.file.set_base_context(self.history_slot, ctx_name);
                }

                if let Some(model_id) = self.drop_down(cx, ids!(model_dropdown)).selected(actions) {
                    let model = &data.ai_chat_manager.models[model_id].name;
                    doc.file.set_model(self.history_slot, model);
                }

                if let Some(project_id) =
                    self.drop_down(cx, ids!(project_dropdown)).selected(actions)
                {
                    let model = &data.ai_chat_manager.projects[project_id].name;
                    doc.file.set_project(self.history_slot, model);
                }

                let list = self.view.portal_list(cx, ids!(list));

                // handle escape globally to stop streaming
                for action in actions {
                    if let Some(action) = action.as_widget_action() {
                        if let TextInputAction::Escaped = action.cast() {
                            cx.action(AppAction::CancelAiGeneration { chat_id });
                        }
                    }
                }

                for (item_id, item) in list.items_with_actions(actions) {
                    //let item_id = items_len - item_id - 1;
                    let message_input = item.text_input(cx, ids!(message_input));
                    if let Some(text) = message_input.changed(actions) {
                        doc.file
                            .fork_chat_at(cx, &mut self.history_slot, item_id, text);
                        cx.action(AppAction::RedrawAiChat { chat_id });
                        cx.action(AppAction::SaveAiChat { chat_id });
                    }

                    if let Some(ke) = item
                        .text_input(cx, ids!(message_input))
                        .key_down_unhandled(actions)
                    {
                        if ke.key_code == KeyCode::ReturnKey && ke.modifiers.logo {
                            // run it
                            cx.action(AppAction::RunAiChat {
                                chat_id,
                                history_slot: self.history_slot,
                                item_id,
                            });
                        }
                        if ke.key_code == KeyCode::ArrowLeft && ke.modifiers.logo {
                            self.history_slot = self.history_slot.saturating_sub(1);
                            cx.action(AppAction::RedrawAiChat { chat_id });
                            if ke.modifiers.control {
                                cx.action(AppAction::RunAiChat {
                                    chat_id,
                                    history_slot: self.history_slot,
                                    item_id,
                                });
                            }
                        }
                        if ke.key_code == KeyCode::ArrowRight && ke.modifiers.logo {
                            self.history_slot = (self.history_slot + 1)
                                .min(doc.file.history.len().saturating_sub(1));
                            cx.action(AppAction::RedrawAiChat { chat_id });
                            if ke.modifiers.control {
                                cx.action(AppAction::RunAiChat {
                                    chat_id,
                                    history_slot: self.history_slot,
                                    item_id,
                                });
                            }
                        }
                    }

                    if item.button(cx, ids!(run_button)).pressed(actions) {
                        cx.action(AppAction::RunAiChat {
                            chat_id,
                            history_slot: self.history_slot,
                            item_id,
                        });
                    }

                    if item.button(cx, ids!(send_button)).pressed(actions)
                        || item
                            .text_input(cx, ids!(message_input))
                            .returned(actions)
                            .is_some()
                    {
                        // we'd already be forked
                        let text = message_input.text();

                        doc.file
                            .fork_chat_at(cx, &mut self.history_slot, item_id, text);
                        // alright so we press send/enter now what
                        // we now call 'setaichatlen' this will 'fork' our current index
                        // what if our chat is empty? then we dont fork
                        doc.file.clamp_slot(&mut self.history_slot);
                        // lets fetch the context
                        // println!("{}", dd.selected_item());
                        // alright lets collect the context
                        cx.action(AppAction::SendAiChatToBackend {
                            chat_id,
                            history_slot: self.history_slot,
                        });
                        cx.action(AppAction::SaveAiChat { chat_id });
                        cx.action(AppAction::RedrawAiChat { chat_id });
                        // scroll to end and enable tailing
                        let items_len = doc.file.history[self.history_slot].messages.len();
                        list.set_tail_range(true);
                        list.set_first_id_and_scroll(items_len.saturating_sub(1), 0.0);
                        list.redraw(cx);
                    }
                    // lets clear the messages
                    if item.button(cx, ids!(clear_button)).pressed(actions) {
                        doc.file
                            .fork_chat_at(cx, &mut self.history_slot, item_id, "".to_string());
                        cx.action(AppAction::SaveAiChat { chat_id });
                        cx.action(AppAction::RedrawAiChat { chat_id });
                    }
                }
            }
        }
    }
}
impl Widget for AiChatView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let data = scope.data.get_mut::<AppData>().unwrap();
        let path = cx.widget_tree().path_to(self.widget_uid());
        let session_id = path.last().copied().unwrap_or(LiveId(0));
        if let Some(EditSession::AiChat(chat_id)) = data.file_system.get_session_mut(session_id) {
            let chat_id = *chat_id;
            if let Some(OpenDocument::AiChat(doc)) = data.file_system.open_documents.get(&chat_id) {
                if !self.initialised {
                    self.initialised = true;
                    self.history_slot = doc
                        .file
                        .history
                        .iter()
                        .enumerate()
                        .max_by(|(_, a), (_, b)| a.last_time.total_cmp(&b.last_time))
                        .map(|(index, _)| index)
                        .unwrap_or(0);
                }

                self.check_box(cx, ids!(auto_run))
                    .set_active(cx, doc.auto_run);

                // show/hide stop button based on in_flight status
                self.button(cx, ids!(stop_button))
                    .set_visible(cx, doc.in_flight.is_some());

                let history_len = doc.file.history.len();
                self.label(cx, ids!(slot))
                    .set_text_with(|v| fmt_over!(v, "{}/{}", self.history_slot + 1, history_len));

                let messages = &doc.file.history[self.history_slot];
                // model dropdown
                let dd = self.drop_down(cx, ids!(model_dropdown));
                // ok how do we set these dropdown labels without causing memory changes
                let mut i = data.ai_chat_manager.models.iter();
                dd.set_labels_with(cx, |label| {
                    i.next().map(|m| label.push_str(&m.name));
                });
                if let Some(pos) = data
                    .ai_chat_manager
                    .models
                    .iter()
                    .position(|b| b.name == messages.model)
                {
                    dd.set_selected_item(cx, pos);
                }

                let dd = self.drop_down(cx, ids!(context_dropdown));
                let mut i = data.ai_chat_manager.contexts.iter();
                dd.set_labels_with(cx, |label| {
                    i.next().map(|m| label.push_str(&m.name));
                });

                if let Some(pos) = data
                    .ai_chat_manager
                    .contexts
                    .iter()
                    .position(|ctx| ctx.name == messages.base_context)
                {
                    dd.set_selected_item(cx, pos);
                }

                let dd = self.drop_down(cx, ids!(project_dropdown));
                let mut i = data.ai_chat_manager.projects.iter();
                dd.set_labels_with(cx, |label| {
                    i.next().map(|m| label.push_str(&m.name));
                });

                if let Some(pos) = data
                    .ai_chat_manager
                    .projects
                    .iter()
                    .position(|ctx| ctx.name == messages.project)
                {
                    dd.set_selected_item(cx, pos);
                }

                while let Some(item) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
                    if let Some(mut list) = item.as_portal_list().borrow_mut() {
                        doc.file.clamp_slot(&mut self.history_slot);
                        let items_len = doc.file.history[self.history_slot].messages.len();
                        list.set_item_range(cx, 0, items_len);

                        while let Some(item_id) = list.next_visible_item(cx) {
                            match doc.file.history[self.history_slot].messages.get(item_id) {
                                Some(AiChatMessage::Assistant(val)) => {
                                    let busy = item_id == items_len - 1 && doc.in_flight.is_some();
                                    let item = list.item(cx, item_id, id!(Assistant));
                                    // alright we got the assistant. lets set the markdown stuff
                                    item.widget(cx, ids!(md)).set_text(cx, &val);
                                    item.view(cx, ids!(busy)).set_visible(cx, busy);
                                    item.draw_all_unscoped(cx);
                                }
                                Some(AiChatMessage::User(val)) => {
                                    // lets set the value to the text input
                                    let item = list.item(cx, item_id, id!(User));

                                    item.widget(cx, ids!(message_input))
                                        .set_text(cx, &val.message);
                                    item.draw_all_unscoped(cx);
                                }
                                _ => (),
                            }
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let ac = cx.capture_actions(|cx| {
            self.view.handle_event(cx, event, scope);
        });
        if ac.len() > 0 {
            self.handle_own_actions(cx, &ac, scope)
        }
    }
}
