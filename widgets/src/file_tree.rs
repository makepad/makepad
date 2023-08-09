use {
    std::{
        collections::{HashSet},
    },
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        scroll_shadow::DrawScrollShadow,
        scroll_bars::ScrollBars
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawBgQuad = {{DrawBgQuad}} {
        fn pixel(self) -> vec4 {
            return mix(
                mix(
                    COLOR_BG_EDITOR,
                    COLOR_BG_ODD,
                    self.is_even
                ),
                mix(
                    COLOR_BG_UNFOCUSSED,
                    COLOR_BG_SELECTED,
                    self.focussed
                ),
                self.selected
            );
        }
    }
    
    DrawNameText = {{DrawNameText}} {
        fn get_color(self) -> vec4 {
            return mix(
                mix(
                    COLOR_TEXT_DEFAULT * self.scale,
                    COLOR_TEXT_SELECTED,
                    self.selected
                ),
                COLOR_TEXT_HOVER,
                self.hover
            )
        }
        
        text_style: <FONT_DATA> {
            top_drop: 1.2,
        }
    }
    
    DrawIconQuad = {{DrawIconQuad}} {
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            let w = self.rect_size.x;
            let h = self.rect_size.y;
            sdf.box(0. * w, 0.35 * h, 0.87 * w, 0.39 * h, 0.75);
            sdf.box(0. * w, 0.28 * h, 0.5 * w, 0.3 * h, 1.);
            sdf.union();
            return sdf.fill(mix(
                mix(
                    COLOR_TEXT_DEFAULT * self.scale,
                    COLOR_TEXT_SELECTED,
                    self.selected
                ),
                COLOR_TEXT_HOVER,
                self.hover
            ));
        }
    }
    
    FileTreeNode = {{FileTreeNode}} {
        
        layout: {
            align: {y: 0.5},
            padding: {left: 5.0, bottom: 0,},
        }
        
        icon_walk: {
            width: Fixed((DIM_DATA_ICON_WIDTH)),
            height: Fixed((DIM_DATA_ICON_HEIGHT)),
            margin: {
                left: 1
                top: 0
                right: 2
                bottom: 0
            },
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        hover: 0.0
                        draw_bg: {hover: 0.0}
                        draw_name: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                
                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {
                        hover: 1.0
                        draw_bg: {hover: 1.0}
                        draw_name: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    },
                }
            }
            
            focus = {
                default: on
                on = {
                    from: {all: Snap}
                    apply: {focussed: 1.0}
                }
                
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {focussed: 0.0}
                }
            }
            
            select = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        selected: 0.0
                        draw_bg: {selected: 0.0}
                        draw_name: {selected: 0.0}
                        draw_icon: {selected: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        selected: 1.0
                        draw_bg: {selected: 1.0}
                        draw_name: {selected: 1.0}
                        draw_icon: {selected: 1.0}
                    }
                }
                
            }
            
            open = {
                default: off
                off = {
                    //from: {all: Exp {speed1: 0.80, speed2: 0.97}}
                    //duration: 0.2
                    redraw: true
                    
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.80, d2: 0.97}
                    
                    //ease: Ease::OutExp
                    apply: {
                        opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]
                        draw_bg: {opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                        draw_name: {opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                        draw_icon: {opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                    }
                }
                
                on = {
                    //from: {all: Exp {speed1: 0.82, speed2: 0.95}}
                    
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.82, d2: 0.95}
                    
                    //from: {all: Exp {speed1: 0.82, speed2: 0.95}}
                    redraw: true
                    apply: {
                        opened: 1.0
                        draw_bg: {opened: 1.0}
                        draw_name: {opened: 1.0}
                        draw_icon: {opened: 1.0}
                    }
                }
            }
        }
        is_folder: false,
        indent_width: 10.0
        min_drag_distance: 10.0
    }
    
    FileTree = {{FileTree}} {
        node_height: (DIM_DATA_ITEM_HEIGHT),
        file_node: <FileTreeNode> {
            is_folder: false,
            draw_bg: {is_folder: 0.0}
            draw_name: {is_folder: 0.0}
        }
        folder_node: <FileTreeNode> {
            is_folder: true,
            draw_bg: {is_folder: 1.0}
            draw_name: {is_folder: 1.0}
        }
        layout: {flow: Down, clip_x: true, clip_y: true},
        scroll_bars: {}
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawBgQuad {
    #[deref] draw_super: DrawQuad,
    #[live] is_even: f32,
    #[live] scale: f32,
    #[live] is_folder: f32,
    #[live] focussed: f32,
    #[live] selected: f32,
    #[live] hover: f32,
    #[live] opened: f32,
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawNameText {
    #[deref] draw_super: DrawText,
    #[live] is_even: f32,
    #[live] scale: f32,
    #[live] is_folder: f32,
    #[live] focussed: f32,
    #[live] selected: f32,
    #[live] hover: f32,
    #[live] opened: f32,
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawIconQuad {
    #[deref] draw_super: DrawQuad,
    #[live] is_even: f32,
    #[live] scale: f32,
    #[live] is_folder: f32,
    #[live] focussed: f32,
    #[live] selected: f32,
    #[live] hover: f32,
    #[live] opened: f32,
}

#[derive(Live, LiveHook)]
pub struct FileTreeNode {
    #[live] draw_bg: DrawBgQuad,
    #[live] draw_icon: DrawIconQuad,
    #[live] draw_name: DrawNameText,
    #[live] layout: Layout,
    
    #[state] state: LiveState,
    
    #[live] indent_width: f64,
    
    #[live] icon_walk: Walk,
    
    #[live] is_folder: bool,
    #[live] min_drag_distance: f64,
    
    #[live] opened: f32,
    #[live] focussed: f32,
    #[live] hover: f32,
    #[live] selected: f32,
}

#[derive(Live)]
pub struct FileTree {
    #[live] scroll_bars: ScrollBars,
    #[live] file_node: Option<LivePtr>,
    #[live] folder_node: Option<LivePtr>,
    #[live] walk: Walk,
    #[live] layout: Layout,
    #[live] filler: DrawBgQuad,
    
    #[live] node_height: f64,
    
    #[live] draw_scroll_shadow: DrawScrollShadow,
    
    #[rust] draw_state: DrawStateWrap<()>,
    
    #[rust] dragging_node_id: Option<FileNodeId>,
    #[rust] selected_node_id: Option<FileNodeId>,
    #[rust] open_nodes: HashSet<FileNodeId>,
    
    #[rust] tree_nodes: ComponentMap<FileNodeId, (FileTreeNode, LiveId)>,
    
    #[rust] count: usize,
    #[rust] stack: Vec<f64>,
}

impl LiveHook for FileTree {
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, FileTree)
    }

    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for (_, (tree_node, id)) in self.tree_nodes.iter_mut() {
            if let Some(index) = nodes.child_by_name(index, id.as_field()) {
                tree_node.apply(cx, from, index, nodes);
            }
        }
        self.scroll_bars.redraw(cx);
    }
}

#[derive(Clone, WidgetAction)]
pub enum FileTreeAction {
    None,
    FileClicked(FileNodeId),
    ShouldFileStartDrag(FileNodeId),
}

pub enum FileTreeNodeAction {
    None,
    WasClicked,
    Opening,
    Closing,
    ShouldStartDrag
}

impl FileTreeNode {
    pub fn set_draw_state(&mut self, is_even: f32, scale: f64) {
        self.draw_bg.scale = scale as f32;
        self.draw_bg.is_even = is_even;
        self.draw_name.scale = scale as f32;
        self.draw_name.is_even = is_even;
        self.draw_icon.scale = scale as f32;
        self.draw_icon.is_even = is_even;
        self.draw_name.font_scale = scale;
    }
    
    pub fn draw_folder(&mut self, cx: &mut Cx2d, name: &str, is_even: f32, node_height: f64, depth: usize, scale: f64) {
        self.set_draw_state(is_even, scale);
        
        self.draw_bg.begin(cx, Walk::size(Size::Fill, Size::Fixed(scale * node_height)), self.layout);
        
        cx.walk_turtle(self.indent_walk(depth));
        
        self.draw_icon.draw_walk(cx, self.icon_walk);
        
        self.draw_name.draw_walk(cx, Walk::fit(), Align::default(), name);
        self.draw_bg.end(cx);
    }
    
    pub fn draw_file(&mut self, cx: &mut Cx2d, name: &str, is_even: f32, node_height: f64, depth: usize, scale: f64) {
        self.set_draw_state(is_even, scale);
        
        self.draw_bg.begin(cx, Walk::size(Size::Fill, Size::Fixed(scale * node_height)), self.layout);
        
        cx.walk_turtle(self.indent_walk(depth));
        
        self.draw_name.draw_walk(cx, Walk::fit(), Align::default(), name);
        self.draw_bg.end(cx);
    }
    
    fn indent_walk(&self, depth: usize) -> Walk {
        Walk {
            abs_pos: None,
            width: Size::Fixed(depth as f64 * self.indent_width),
            height: Size::Fixed(0.0),
            margin: Margin {
                left: depth as f64 * 1.0,
                top: 0.0,
                right: depth as f64 * 4.0,
                bottom: 0.0,
            },
        }
    }
    
    fn set_is_selected(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_state(cx, is, animate, id!(select.on), id!(select.off))
    }
    
    fn set_is_focussed(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_state(cx, is, animate, id!(focus.on), id!(focus.off))
    }
    
    pub fn set_folder_is_open(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_state(cx, is, animate, id!(open.on), id!(open.off));
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FileTreeNodeAction),
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerMove(f) => {
                if f.abs.distance(&f.abs_start) >= self.min_drag_distance {
                    dispatch_action(cx, FileTreeNodeAction::ShouldStartDrag);
                }
            }
            Hit::FingerDown(_) => {
                self.animate_state(cx, id!(select.on));
                if self.is_folder {
                    if self.state.is_in_state(cx, id!(open.on)) {
                        self.animate_state(cx, id!(open.off));
                        dispatch_action(cx, FileTreeNodeAction::Closing);
                    }
                    else {
                        self.animate_state(cx, id!(open.on));
                        dispatch_action(cx, FileTreeNodeAction::Opening);
                    }
                    
                }
                dispatch_action(cx, FileTreeNodeAction::WasClicked);
            }
            _ => {}
        }
    }
}


impl FileTree {
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.scroll_bars.begin(cx, walk, self.layout);
        self.count = 0;
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        // lets fill the space left with blanks
        let height_left = cx.turtle().height_left();
        let mut walk = 0.0;
        while walk < height_left {
            self.count += 1;
            self.filler.is_even = Self::is_even(self.count);
            self.filler.draw_walk(cx, Walk::size(Size::Fill, Size::Fixed(self.node_height.min(height_left - walk))));
            walk += self.node_height.max(1.0);
        }
        
        self.draw_scroll_shadow.draw(cx, dvec2(0., 0.));
        self.scroll_bars.end(cx);
        
        let selected_node_id = self.selected_node_id;
        self.tree_nodes.retain_visible_and( | node_id, _ | Some(*node_id) == selected_node_id);
    }
    
    pub fn is_even(count: usize) -> f32 {
        if count % 2 == 1 {0.0}else {1.0}
    }
    
    pub fn should_node_draw(&mut self, cx: &mut Cx2d) -> bool {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        let height = self.node_height * scale;
        let walk = Walk::size(Size::Fill, Size::Fixed(height));
        if scale > 0.01 && cx.walk_turtle_would_be_visible(walk) {
            return true
        }
        else {
            cx.walk_turtle(walk);
            return false
        }
    }
    
    pub fn begin_folder(
        &mut self,
        cx: &mut Cx2d,
        node_id: FileNodeId,
        name: &str,
    ) -> Result<(), ()> {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        if scale > 0.2 {
            self.count += 1;
        }
        
        let is_open = self.open_nodes.contains(&node_id);
        
        if self.should_node_draw(cx) {
            let folder_node = self.folder_node;
            let (tree_node, _) = self.tree_nodes.get_or_insert(cx, node_id, | cx | {
                let mut tree_node = FileTreeNode::new_from_ptr(cx, folder_node);
                if is_open {
                    tree_node.set_folder_is_open(cx, true, Animate::No)
                }
                (tree_node, live_id!(folder_node))
            });
            
            tree_node.draw_folder(cx, name, Self::is_even(self.count), self.node_height, self.stack.len(), scale);
            self.stack.push(tree_node.opened as f64 * scale);
            if tree_node.opened == 0.0 {
                self.end_folder();
                return Err(());
            }
        }
        else {
            if is_open {
                self.stack.push(scale * 1.0);
            }
            else {
                return Err(());
            }
        }
        Ok(())
    }
    
    pub fn end_folder(&mut self) {
        self.stack.pop();
    }
    
    pub fn file(&mut self, cx: &mut Cx2d, node_id: FileNodeId, name: &str) {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        if scale > 0.2 {
            self.count += 1;
        }
        if self.should_node_draw(cx) {
            let file_node = self.file_node;
            let (tree_node, _) = self.tree_nodes.get_or_insert(cx, node_id, | cx | {
                (FileTreeNode::new_from_ptr(cx, file_node), live_id!(file_node))
            });
            tree_node.draw_file(cx, name, Self::is_even(self.count), self.node_height, self.stack.len(), scale);
        }
    }
    
    pub fn forget(&mut self) {
        self.tree_nodes.clear();
    }
    
    pub fn forget_node(&mut self, file_node_id: FileNodeId) {
        self.tree_nodes.remove(&file_node_id);
    }
    
    pub fn set_folder_is_open(
        &mut self,
        cx: &mut Cx,
        node_id: FileNodeId,
        is_open: bool,
        animate: Animate,
    ) {
        if is_open {
            self.open_nodes.insert(node_id);
        }
        else {
            self.open_nodes.remove(&node_id);
        }
        if let Some((tree_node, _)) = self.tree_nodes.get_mut(&node_id) {
            tree_node.set_folder_is_open(cx, is_open, animate);
        }
    }
    
    pub fn start_dragging_file_node(
        &mut self,
        cx: &mut Cx,
        node_id: FileNodeId,
        items: Vec<DragItem>,
    ) {
        self.dragging_node_id = Some(node_id);
        cx.start_dragging(items);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<FileTreeAction> {
        let mut a = Vec::new();
        self.handle_event_with(cx, event, &mut | _, v | a.push(v));
        a
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FileTreeAction),
    ) {
        self.scroll_bars.handle_event_with(cx, event, &mut | _, _ | {});
        
        match event {
            Event::DragEnd => self.dragging_node_id = None,
            _ => ()
        }
        
        let mut actions = Vec::new();
        for (node_id, (node, _)) in self.tree_nodes.iter_mut() {
            node.handle_event_with(cx, event, &mut | _, e | actions.push((*node_id, e)));
        }
        
        for (node_id, action) in actions {
            match action {
                FileTreeNodeAction::Opening => {
                    self.open_nodes.insert(node_id);
                }
                FileTreeNodeAction::Closing => {
                    self.open_nodes.remove(&node_id);
                }
                FileTreeNodeAction::WasClicked => {
                    cx.set_key_focus(self.scroll_bars.area());
                    if let Some(last_selected) = self.selected_node_id {
                        if last_selected != node_id {
                            self.tree_nodes.get_mut(&last_selected).unwrap().0.set_is_selected(cx, false, Animate::Yes);
                        }
                    }
                    self.selected_node_id = Some(node_id);
                    dispatch_action(cx, FileTreeAction::FileClicked(node_id));
                }
                FileTreeNodeAction::ShouldStartDrag => {
                    if self.dragging_node_id.is_none() {
                        dispatch_action(cx, FileTreeAction::ShouldFileStartDrag(node_id));
                    }
                }
                _ => ()
            }
        }
        
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::KeyFocus(_) => {
                if let Some(node_id) = self.selected_node_id {
                    self.tree_nodes.get_mut(&node_id).unwrap().0.set_is_focussed(cx, true, Animate::Yes);
                }
            }
            Hit::KeyFocusLost(_) => {
                if let Some(node_id) = self.selected_node_id {
                    self.tree_nodes.get_mut(&node_id).unwrap().0.set_is_focussed(cx, false, Animate::Yes);
                }
            }
            _ => ()
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct FileNodeId(pub LiveId);

impl Widget for FileTree {
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, ()) {
            self.begin(cx, walk);
            return WidgetDraw::hook_above()
        }
        if let Some(()) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        WidgetDraw::done()
    }
}

#[derive(Debug, Clone, Default, PartialEq, WidgetRef)]
pub struct FileTreeRef(WidgetRef);

impl FileTreeRef{
    pub fn should_file_start_drag(&self, actions: &WidgetActions) -> Option<FileNodeId> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FileTreeAction::ShouldFileStartDrag(file_id) = item.action() {
                return Some(file_id)
            }
        }
        None
    }
    
    pub fn file_clicked(&self, actions: &WidgetActions) -> Option<FileNodeId> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FileTreeAction::FileClicked(file_id) = item.action() {
                return Some(file_id)
            }
        }
        None
    }
    
    
    pub fn file_start_drag(&self, cx: &mut Cx, _file_id: FileNodeId, item: DragItem) {
        cx.start_dragging(vec![item]);
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct FileTreeSet(WidgetSet);
