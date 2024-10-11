use {
    crate::{
        app::{AppData, AppAction},
        ai_chat::ai_chat_manager::*,
        file_system::file_system::{EditSession,OpenDocument},
        makepad_widgets::*,
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_code_editor::code_view::CodeView;
    import makepad_widgets::base::*;
    import makepad_draw::shader::std::*;
        
    import makepad_widgets::theme_desktop_dark::*;

    User = <RoundedView> {
        height: Fit,
        flow: Down,
        margin: <THEME_MSPACE_3> {}
        padding: <THEME_MSPACE_2> { top: (THEME_SPACE_1), bottom: (THEME_SPACE_2) } 
        draw_bg: { color: (THEME_COLOR_U_1) }

        <View> {
            height: Fit, width: Fill,
            flow: Right,
            align: { x: 0., y: 0. },
            spacing: (THEME_SPACE_3),
            padding: { left: (THEME_SPACE_1), right: (THEME_SPACE_1), top: (THEME_SPACE_1) }
            margin: { bottom: -5.}

            run_button = <ButtonFlat> {
                width: Fit,
                height: Fit,
                padding: <THEME_MSPACE_2> {}
                margin: 0.

                text: "Run",
                draw_icon: {
                    color: (THEME_COLOR_U_4),
                    svg_file: dep("crate://self/resources/icons/icon_run.svg"),
                }
                icon_walk: { width: 9. }
            }

            <Vr> { height: 17.5}

            <View> {
                flow: Right,
                width: Fit,
                height: Fit,
                spacing: (THEME_SPACE_1)

                <Pbold> { width: Fit, text: "Model", margin: 0., padding: <THEME_MSPACE_V_1> {} }
                model_dropdown = <DropDownFlat> { width: Fit, popup_menu_position: BelowInput }
            }

            <View> {
                flow: Right,
                width: Fit,
                height: Fit,
                spacing: (THEME_SPACE_1)

                <Pbold> { width: Fit, text: "Context", margin: 0., padding: <THEME_MSPACE_V_1> {} }
                context_dropdown = <DropDownFlat>{ width: Fit, popup_menu_position: BelowInput }
            }

            <View> {
                flow: Right,
                width: Fit,
                spacing: (THEME_SPACE_1)

                <Pbold> { width: Fit, text: "Project", margin: 0., padding: <THEME_MSPACE_V_1> {} }
                project_dropdown = <DropDownFlat> { width: Fit, popup_menu_position: BelowInput }
            }

            <View> { width: Fill }

        }

        <View>{
            height:Fit, width: Fill,

            message_input = <TextInput> {
                width: Fill,
                height: Fit,

                text: ""
                empty_message: "Enter prompt"
            }

            send_button = <ButtonFlatter> {
                width: Fit,
                padding: <THEME_MSPACE_V_1> {}
                margin: { left: -35.}
                draw_icon: {
                    color: (THEME_COLOR_U_4),
                    svg_file: dep("crate://self/resources/icons/icon_run.svg"),
                }
                icon_walk: { width: 6. }
            }
            
            // <ButtonFlatter> {
            //     icon_walk: { width: 16, height: Fit}
            //     text: ">"
            // }
                    
            clear_button = <ButtonFlatter> {
                width: Fit,
                padding: <THEME_MSPACE_V_1> {}
                draw_icon: {
                    color: (THEME_COLOR_U_4),
                    svg_file: dep("crate://self/resources/icons/icon_times.svg"),
                }
                icon_walk: { width: 7. }
            }
        }
        
        
    }
    
    Assistant = <RoundedView> {
        flow: Down
        margin: <THEME_MSPACE_H_3> {}
        padding: <THEME_MSPACE_H_2> { bottom: (THEME_SPACE_2) }

        draw_bg: {
            color: (THEME_COLOR_D_2)
        }
        flow: Down
        md = <Markdown>{
            code_block = <View>{
                
                width:Fill,
                height:Fit,
                flow: Overlay
                code_view = <CodeView>{
                    editor:{
                        draw_bg: { color: ((THEME_COLOR_D_HIDDEN)) }
                    }
                }
                <View>{
                    width:Fill,
                    height:Fit,
                    align:{ x: 1.0 }
                    copy_button = <ButtonFlatter> {
                        icon_walk: {
                            width: 12, height: Fit,
                            margin: { left: 10 }
                        }
                        draw_icon: {
                            color: (THEME_COLOR_U_4)
                            svg_file: dep("crate://self/resources/icons/icon_copy.svg"),
                        }
                    }
                    
                }
                
            }
            use_code_block_widget: true,
            body:""
        }
        busy = <View>{
            width: 50, height: 10,
            margin: 0.,
            padding: 0.,
            show_bg: true,
            draw_bg:{
                fn pixel(self)->vec4{
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    let x = 0.;
                    for i in 0..3{
                        x = x + 8.;
                        sdf.circle(x,5.,2.5);
                        sdf.fill((THEME_COLOR_MAKEPAD));
                    }
                    return sdf.result
                }
            }
        }
    }
    
    AiChatView = {{AiChatView}}{
        flow: Down,
        height: Fill, width: Fill,
        spacing: (THEME_SPACE_1),
        show_bg: true,
        draw_bg: { color: (THEME_COLOR_D_1) },
        
        tb = <DockToolbar> {
            content = {
                height: Fill, width: Fill,
                flow: Right,
                align: { x: 0.0, y: 0.5},
                margin: <THEME_MSPACE_H_2> {}
                spacing: (THEME_SPACE_2),

                auto_run = <CheckBoxCustom> {
                    text: "Auto run",
                    align: { y: 0.5 }
                    draw_check: { check_type: None }
                    spacing: (THEME_SPACE_1),
                    padding: <THEME_MSPACE_V_2> {}
                    icon_walk: { width: 10. }
                    draw_icon: {
                        color: (THEME_COLOR_D_4),
                        color_active: (STUDIO_PALETTE_6),
                        svg_file: dep("crate://self/resources/icons/icon_auto.svg"),
                    }
                }
/*
                <P> {
                    width: Fit,
                    height: Fit,
                    draw_text: {
                        color: (THEME_COLOR_U_4)
                    }
                    text: "First Prompt / "
                }
                <Pbold> { width: Fit, text: "Last Prompt" }
*/
                <View> { width: Fill }

                history_left = <ButtonFlatter> {
                    width: Fit,
                    padding: <THEME_MSPACE_1> {}
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_history_rew.svg"),
                    }
                    icon_walk: { width: 5. }
                }

                slot = <Label> {
                    draw_text: {
                        color: (THEME_COLOR_U_4)
                    }
                    width: Fit,
                    text: "0"
                }

                history_right = <ButtonFlatter> {
                    width: Fit,
                    padding: <THEME_MSPACE_1> {}
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_history_ff.svg"),
                    }
                    icon_walk: { width: 5. }
                }

                history_delete = <ButtonFlat> {
                    width: Fit,
                    text: "Delete"
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_del.svg"),
                    }
                    icon_walk: { width: 10. }
                }

               <Vr> {} 

                <ButtonFlat> {
                    width: Fit,
                    text: "New"
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_add.svg"),
                    }
                    icon_walk: { width: 13. }
                }
            }
        }

        // lets make portal list with User and Assistant components
        // and lets fix the portal lists scroll
        list = <PortalList>{
            drag_scrolling: false
            //auto_tail: true
            User = <User>{}
            Assistant = <Assistant>{}
        }
    }
} 
 
#[derive(Live, LiveHook, Widget)] 
pub struct AiChatView{
    #[deref] view:View,
    #[rust] initialised: bool,
    #[rust] history_slot: usize,
}

impl AiChatView{
    fn handle_own_actions(&mut self, cx: &mut Cx, actions:&Actions, scope: &mut Scope){
        let data = scope.data.get_mut::<AppData>().unwrap();
        let session_id = scope.path.from_end(0);
        
        if let Some(EditSession::AiChat(chat_id)) = data.file_system.get_session_mut(session_id){
            let chat_id = *chat_id;
            if let Some(OpenDocument::AiChat(doc)) = data.file_system.open_documents.get_mut(&chat_id){
                                
                if let Some(value) = self.view.check_box(id!(auto_run)).changed(actions){
                    doc.auto_run = value;
                }
                
                if let Some(wa) = actions.widget_action(id!(copy_button)){
                    if wa.widget().as_button().pressed(actions){
                        let code_view = wa.widget_nth(2).widget(id!(code_view));
                        log!("COPY! {}", code_view.text( ));
                    }
                }
                if self.view.button(id!(history_left)).pressed(actions){
                    // first we check if our messages are the same as 'slot'.
                    // if not, we should create an undo item first
                    self.history_slot = self.history_slot.saturating_sub(1);
                    cx.action(AppAction::RedrawAiChat{chat_id});
                }
                if self.view.button(id!(history_right)).pressed(actions){
                    self.history_slot = (self.history_slot + 1).min(doc.file.history.len().saturating_sub(1));
                    cx.action(AppAction::RedrawAiChat{chat_id});
                }
                if self.view.button(id!(history_delete)).pressed(actions){
                    doc.file.remove_slot(cx, &mut self.history_slot);
                    cx.action(AppAction::RedrawAiChat{chat_id});
                    cx.action(AppAction::SaveAiChat{chat_id});
                }
                                
                let list = self.view.portal_list(id!(list));
                for (item_id,item) in list.items_with_actions(actions){
                    let message_input = item.text_input(id!(message_input));
                    if let Some(text) = message_input.changed(actions){
                        doc.file.fork_chat_at(cx, &mut self.history_slot, item_id, text);
                        cx.action(AppAction::RedrawAiChat{chat_id});
                        cx.action(AppAction::SaveAiChat{chat_id});
                    }
                    if message_input.escape(actions){
                        cx.action(AppAction::CancelAiGeneration{chat_id});
                    }
                    
                    
                    if let Some(ke) = item.text_input(id!(message_input)).key_down_unhandled(actions){
                        if ke.key_code == KeyCode::ReturnKey && ke.modifiers.logo{
                            // run it
                            cx.action(AppAction::RunAiChat{chat_id, history_slot: self.history_slot, item_id});
                        }
                        if ke.key_code == KeyCode::ArrowLeft && ke.modifiers.logo{
                            self.history_slot = self.history_slot.saturating_sub(1);
                            cx.action(AppAction::RedrawAiChat{chat_id});
                            if ke.modifiers.control{
                                cx.action(AppAction::RunAiChat{chat_id, history_slot: self.history_slot, item_id});
                            }
                        }
                        if ke.key_code == KeyCode::ArrowRight && ke.modifiers.logo{
                            self.history_slot = (self.history_slot + 1).min(doc.file.history.len().saturating_sub(1));
                            cx.action(AppAction::RedrawAiChat{chat_id});                 
                            if ke.modifiers.control{
                                cx.action(AppAction::RunAiChat{chat_id, history_slot: self.history_slot, item_id});
                            }
                        }
                    }
                    
                    if item.button(id!(run_button)).pressed(actions){
                        cx.action(AppAction::RunAiChat{chat_id, history_slot: self.history_slot, item_id});
                    }
                    
                    if let Some(ctx_id) = item.drop_down(id!(context_dropdown)).selected(actions){
                        let ctx_name = &data.ai_chat_manager.contexts[ctx_id].name;
                        doc.file.set_base_context(self.history_slot, item_id, ctx_name);
                    }
                    
                    if let Some(model_id) = item.drop_down(id!(model_dropdown)).selected(actions){
                        let model = &data.ai_chat_manager.models[model_id].name;
                        doc.file.set_model(self.history_slot, item_id, model);
                    }
                    
                    if let Some(project_id) = item.drop_down(id!(project_dropdown)).selected(actions){
                        let model = &data.ai_chat_manager.projects[project_id].name;
                        doc.file.set_project(self.history_slot, item_id, model);
                    }
                         
                    if item.button(id!(send_button)).pressed(actions) || 
                    item.text_input(id!(message_input)).returned(actions).is_some(){
                        // we'd already be forked
                        let text = message_input.text();
                        
                        doc.file.fork_chat_at(cx, &mut self.history_slot, item_id, text);
                        // alright so we press send/enter now what
                        // we now call 'setaichatlen' this will 'fork' our current index
                        // what if our chat is empty? then we dont fork
                        doc.file.clamp_slot(&mut self.history_slot);
                        // lets fetch the context
                        // println!("{}", dd.selected_item());
                        // alright lets collect the context
                        println!("SENDING TO BACKEND");
                        cx.action(AppAction::SendAiChatToBackend{chat_id, history_slot: self.history_slot});
                        cx.action(AppAction::SaveAiChat{chat_id});
                        cx.action(AppAction::RedrawAiChat{chat_id});
                    }
                    // lets clear the messages
                    if item.button(id!(clear_button)).pressed(actions){
                        doc.file.fork_chat_at(cx, &mut self.history_slot, item_id, "".to_string());
                        cx.action(AppAction::SaveAiChat{chat_id});
                        cx.action(AppAction::RedrawAiChat{chat_id});
                    }
                }
            }
        }
       
    }
}
impl Widget for AiChatView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        let data = scope.data.get_mut::<AppData>().unwrap();
        let session_id = scope.path.from_end(0);
        
        if let Some(EditSession::AiChat(chat_id)) = data.file_system.get_session_mut(session_id){
            let chat_id = *chat_id;
            if let Some(OpenDocument::AiChat(doc)) = data.file_system.open_documents.get(&chat_id){
                if !self.initialised{
                    self.initialised = true;
                    self.history_slot = doc.file.history.iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.last_time.total_cmp(&b.last_time))
                    .map(|(index, _)| index).unwrap_or(0);
                }
                
                self.view.check_box(id!(auto_run)).set_selected(cx, doc.auto_run);
                
                let history_len = doc.file.history.len(); 
                self.view.label(id!(slot)).set_text_with(|v| fmt_over!(v, "{}/{}", self.history_slot+1, history_len));
                
                while let Some(item) =  self.view.draw_walk(cx, &mut Scope::empty(), walk).step(){
                    
                    if let Some(mut list) = item.as_portal_list().borrow_mut() {
                        doc.file.clamp_slot(&mut self.history_slot);
                        list.set_item_range(cx, 0,doc.file.history[self.history_slot].messages.len());
                        while let Some(item_id) = list.next_visible_item(cx) {
                            match doc.file.history[self.history_slot].messages.get(item_id){
                                Some(AiChatMessage::Assistant(val))=>{
                                    let item = list.item(cx, item_id, live_id!(Assistant));
                                    // alright we got the assistant. lets set the markdown stuff
                                    item.widget(id!(md)).set_text(&val);
                                    item.view(id!(busy)).set_visible(
                                        item_id + 1 == doc.file.history[self.history_slot].messages.len() && 
                                        doc.in_flight.is_some()
                                    );
                                    item.draw_all_unscoped(cx);
                                }
                                Some(AiChatMessage::User(val))=>{
                                    // lets set the value to the text input
                                    let item = list.item(cx, item_id, live_id!(User));
                                    
                                    // model dropdown
                                    let dd = item.drop_down(id!(model_dropdown));
                                    // ok how do we set these dropdown labels without causing memory changes
                                    let mut i = data.ai_chat_manager.models.iter();
                                    dd.set_labels_with(|label|{i.next().map(|m| label.push_str(&m.name));});
                                    if let Some(pos) = data.ai_chat_manager.models.iter().position(|b| b.name == val.model){
                                        dd.set_selected_item(pos);
                                    }
                                    
                                    
                                    let dd = item.drop_down(id!(context_dropdown));
                                    let mut i = data.ai_chat_manager.contexts.iter();
                                    dd.set_labels_with(|label|{i.next().map(|m| label.push_str(&m.name));});
                                    
                                    if let Some(pos) = data.ai_chat_manager.contexts.iter().position(|ctx| ctx.name == val.base_context){
                                        dd.set_selected_item(pos);
                                    }
                                    
                                    let dd = item.drop_down(id!(project_dropdown));
                                    let mut i = data.ai_chat_manager.projects.iter();
                                    dd.set_labels_with(|label|{i.next().map(|m| label.push_str(&m.name));});
                                                                        
                                    if let Some(pos) = data.ai_chat_manager.projects.iter().position(|ctx| ctx.name == val.base_context){
                                        dd.set_selected_item(pos);
                                    }
                                    
                                    item.widget(id!(message_input)).set_text(&val.message);
                                    item.draw_all_unscoped(cx);
                                }
                                _=>()
                            }
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let ac = cx.capture_actions(|cx|{
            self.view.handle_event(cx, event, scope);
        });
        if ac.len()>0{
            self.handle_own_actions(cx, &ac, scope)
        }
    }
}