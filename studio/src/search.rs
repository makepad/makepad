
use {
    crate::{
        file_system::file_system::FileSystem,
        makepad_file_protocol::SearchItem,
        makepad_platform::studio::JumpToFile,
        app::{AppAction, AppData},
        makepad_widgets::*,
        makepad_code_editor::code_view::*,
    },
    std::{
        env,
    },
};

live_design!{
    use link::shaders::*;
    use link::widgets::*;
    use link::theme::*;
    use makepad_widgets::designer_theme::*;
    use makepad_code_editor::code_view::CodeView;
    
    SearchResult = <View> {
        height: Fit, width: Fill
        padding: <THEME_MSPACE_2> {} // TODO: Fix. Changing this value to i.e. '0.' causes Makepad Studio to freeze when switching to the log tab.
        spacing: (THEME_SPACE_2)
        align: { x: 0.0, y: 0.0 }
        show_bg: true,
        draw_bg: {
            instance is_even: 0.0
            instance selected: 0.0
            instance hover: 0.0
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EVEN,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    THEME_COLOR_OUTSET_ACTIVE,
                    self.selected
                );
            }
        }
        animator: {
            ignore_missing: true,
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: 0.0}
                    }
                }
                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                    },
                }
            }
            
            select = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 1.0}
                    }
                }  
            }
        }
        flow = <TextFlow>{
            width: Fill,
            height: Fit
            width: Fill,
            height: Fit
            
            code_view = <CodeView>{
                editor:{
                    word_wrap: false
                    draw_bg: { color: (#0000) }
                    margin:{left:15}
                    draw_text: {
                       // text_style: {font_size:7}
                    }
                }
            }
            
            fold_button = <FoldButton>{
                animator:{
                    active={default:off}
                }
            }
        }
    }
    
    pub Search = {{Search}} <RectView> {
        height: Fill, width: Fill,
        //draw_bg: {color: (THEME_COLOR_BG_CONTAINER)}
        flow: Down,
        <DockToolbar> {
            content = {
                spacing: (THEME_SPACE_2)
                align: { y: 0.5 }
                search_input = <TextInput> {
                    width: Fill,
                    empty_text: "Search",
                }
            }
        }
        list = <PortalList> {
            capture_overload: false,
            grab_key_focus: false
            auto_tail: true
            drag_scrolling: false
            max_pull_down: 0,
            height: Fill, width: Fill,
            flow: Down
            SearchResult = <SearchResult> {
            }
            Empty = <SearchResult> {
                cursor: Default
                width: Fill
                height: 25,
                body = <P> {  margin: 0, text: "" }
            }
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum SearchAction {
    JumpTo(JumpToFile),
    None
}

#[derive(Live, LiveHook, Widget)]
pub struct Search{
    #[deref] view:View
}

#[derive(Clone, Debug, PartialEq)]
pub struct JumpToFileLink{item_id:usize}

impl Search{
    fn draw_results(&mut self, cx: &mut Cx2d, list:&mut PortalList, file_system:&mut FileSystem){
        
        list.set_item_range(cx, 0, file_system.search_results.len());
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even = item_id & 1 == 0;
            let mut location = String::new();
            if let Some(res) = file_system.search_results.get(item_id as usize) {
                
                let mut item = list.item(cx, item_id, live_id!(SearchResult)).as_view();
                
                item.apply_over(cx, live!{
                    draw_bg: {is_even: (if is_even {1.0} else {0.0})}
                });
                
                while let Some(step) = item.draw(cx, &mut Scope::empty()).step(){
                    if let Some(mut tf) = step.as_text_flow().borrow_mut(){
                        // alright what do we do
                       //tf.draw_item_counted(cx, map_level_to_icon(msg.level));
                        //let fold_button = if msg.explanation.is_some(){
                        let fold_button = tf.draw_item_counted_ref(cx, live_id!(fold_button)).as_fold_button();
                        //}
                        //else{
                        //    Default::default()
                        //};
                        // lets look up the file_name from file_id
                        // and also lets use the result-span info to fetch the right
                        // line and context
                        
                        fmt_over!(location, "{}: {}:{}", res.file_name, res.line + 1, res.column_byte + 1);
                        
                        tf.draw_link(cx, live_id!(link), JumpToFileLink{item_id}, &location);
                        
                        //tf.draw_text(cx, &res.result_line);
                        
                        let open = fold_button.open_float();
                        cx.turtle_new_line();
                        let code = tf.item_counted(cx, live_id!(code_view));
                        code.set_text(cx, &res.result_line);
                        if let Some(mut code_view) = code.as_code_view().borrow_mut(){
                            code_view.lazy_init_session();
                            let lines = code_view.session.as_ref().unwrap().document().as_text().as_lines().len();
                            code_view.editor.height_scale = open.max(1.0 / (lines + 1) as f64);
                        }
                        code.draw_all_unscoped(cx);
                        // lets check 
                        /*if let Some(explanation) = &msg.explanation{
                            
                        };*/
                    }
                }
                continue
            }
            let item = list.item(cx, item_id, live_id!(Empty)).as_view();
            item.apply_over(cx, live!{draw_bg: {is_even: (if is_even {1.0} else {0.0})}});
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for Search {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step(){
            if let Some(mut list) = step.as_portal_list().borrow_mut(){
                self.draw_results(cx, &mut *list, &mut scope.data.get_mut::<AppData>().unwrap().file_system)
            }
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let search_results = self.view.portal_list(id!(list));
        self.view.handle_event(cx, event, scope);
        let data = scope.data.get_mut::<AppData>().unwrap();
        if let Event::Actions(actions) = event{
            if let Some(search) = self.view.text_input(id!(search_input)).changed(&actions){
                let mut set = Vec::new();
                
                for item in search.split("|"){
                    if let Some(item) = item.strip_suffix("\\b"){
                        if let Some(item) = item.strip_prefix("\\b"){
                            set.push(SearchItem{
                                needle: item.to_string(),
                                prefixes: None,
                                pre_word_boundary: true,
                                post_word_boundary: true
                            })
                        }
                        else{
                            set.push(SearchItem{
                                needle: item.to_string(),
                                prefixes: None,
                                pre_word_boundary: false,
                                post_word_boundary: true
                            })
                        }
                    }
                    else if let Some(item) = item.strip_prefix("\\b"){
                        set.push(SearchItem{
                            needle: item.to_string(),
                            prefixes: None,
                            pre_word_boundary: true,
                            post_word_boundary: false
                        })
                    }
                    else{
                        set.push(SearchItem{
                            needle: item.to_string(),
                            prefixes: None,
                            pre_word_boundary: false,
                            post_word_boundary: false
                        })
                    }
                }
                data.file_system.search_string(cx, set);
            }
            if search_results.any_items_with_actions(&actions) {
                // alright lets figure out if someone clicked a link
                // alright so how do we now filter which link was clicked
                for jtf in actions.filter_actions_data::<JumpToFileLink>(){
                    if let Some(res) = data.file_system.search_results.get(jtf.item_id) {
                        cx.action(AppAction::JumpTo(JumpToFile{
                            file_name: res.file_name.clone(), 
                            line: res.line as u32,
                            column: res.column_byte as u32
                        }));
                    }
                }
            }
        }
    }
}

impl SearchRef{
}
