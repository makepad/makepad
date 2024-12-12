use {
    std::{
        collections::{HashSet},
    },
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        fold_button::*,
        scroll_shadow::DrawScrollShadow,
        scroll_bars::ScrollBars
    }
};

live_design!{
    use link::theme::*;
    use makepad_draw::shader::std::*;
    use link::widgets::*;
    use crate::designer_theme::*;
        
    DrawNodeQuad = {{DrawNodeQuad}} {}
    DrawNodeText = {{DrawNodeText}} {}
    DrawNodeIcon = {{DrawNodeIcon}} {}
    
    pub DesignerOutlineTreeNodeBase = {{DesignerOutlineTreeNode}} {}
    pub DesignerOutlineTreeBase = {{DesignerOutlineTree}} {}
    
    pub DesignerOutlineTreeNode = <DesignerOutlineTreeNodeBase> {
        align: { y: 0.5 }
        padding: { left: (THEME_SPACE_1) },
        
        indent_width: 10.0
        min_drag_distance: 10.0
        button_open_width: 24.0,
        draw_eye: false,
        
        draw_bg: {
            instance selected: 0.0
            instance hover: 0.0
            instance focussed: 0.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    -2.,
                    self.rect_size.x,
                    self.rect_size.y + 3.0,
                    1.
                )
                sdf.fill_keep(
                    mix(
                        mix(
                            THEME_COLOR_BG_EVEN,
                            THEME_COLOR_BG_ODD,
                            self.is_even
                        ),
                        THEME_COLOR_CTRL_SELECTED,
                        self.selected
                    )
                )
                return sdf.result
            }
        }
        icon_walk:{
            margin:{top:3,left:3,right:5}
            width:12,
            height:12,
        }
        draw_icon: {
            instance selected: 0.0
            instance hover: 0.0
            instance focussed: 0.0
            fn get_color(self) -> vec4 {
                return self.color * self.scale;
            }
        }
        
        draw_name: {
            instance selected: 0.0
            instance hover: 0.0
            instance focussed: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    THEME_COLOR_TEXT_DEFAULT * self.scale,
                    THEME_COLOR_TEXT_SELECTED,
                    self.selected
                )
            }
            
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
                //top_drop: 1.2,
            }
        }
        
        button_open: <FoldButton> {
            height: 25, width: 15,
            margin: { left: (THEME_SPACE_2) }
            animator: { open = { default: off } },
            draw_bg: {
                uniform size: 3.75;
                instance open: 0.0
                
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    let left = 2;
                    let sz = self.size;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);
                    
                    // PLUS
                    sdf.box(0.5, sz * 3.0, sz * 2.5, sz * 0.7, 1.0); // rounding = 3rd value
                    // vertical
                    sdf.fill_keep(mix(#8F, #FF, self.hover));
                    sdf.box(sz * 1.0, sz * 2.125, sz * 0.7, sz * 2.5, 1.0); // rounding = 3rd value
                    
                    sdf.fill_keep(mix(mix(#8F, #FF, self.hover), #FFF0, self.open))
                    
                    return sdf.result
                }
            }
        }
        
        animator: {
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
        }
    }
    
    STUDIO_PALETTE_1 = #B2FF64
    STUDIO_PALETTE_2 = #80FFBF
    STUDIO_PALETTE_3 = #80BFFF
    STUDIO_PALETTE_4 = #BF80FF
    STUDIO_PALETTE_5 = #FF80BF
    STUDIO_PALETTE_6 = #FFB368
    STUDIO_PALETTE_7 = #FFD864
    
    STUDIO_COLOR_FILE = (THEME_COLOR_TEXT_DEFAULT)
    STUDIO_COLOR_FOLDER = (THEME_COLOR_TEXT_DEFAULT)
    STUDIO_COLOR_LAYOUT = (STUDIO_PALETTE_6)
    STUDIO_COLOR_WIDGET = (STUDIO_PALETTE_2)
    STUDIO_COLOR_ASSET = (STUDIO_PALETTE_5)
    STUDIO_COLOR_TEXT = (STUDIO_PALETTE_1)
    
    pub DesignerOutlineTree = <DesignerOutlineTreeBase> {
        flow: Down,
        
        scroll_bars: <ScrollBars> {}
        scroll_bars: {}
        node_height: (THEME_DATA_ITEM_HEIGHT),
        clip_x: true,
        clip_y: true
        
        File = <DesignerOutlineTreeNode> {
            draw_eye: true,
            draw_icon: {
                color: (STUDIO_COLOR_FILE)
                svg_file: dep("crate://self/resources/icons/icon_file.svg"),
            }
        }
        
        Folder = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_FOLDER)
                svg_file: dep("crate://self/resources/icons/icon_folder.svg"),
            }
        }
        
        Layout = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_LAYOUT)
                svg_file: dep("crate://self/resources/icons/icon_layout.svg"),
            }
        }
        
        Widget = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_WIDGET)
                svg_file: dep("crate://self/resources/icons/icon_widget.svg"),
            }
        }
        
        Asset = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_ASSET)
                svg_file: dep("crate://self/resources/icons/icon_image.svg"),
            }
        }
        
        Text = <DesignerOutlineTreeNode> {
            draw_icon: {
                color: (STUDIO_COLOR_TEXT)
                svg_file: dep("crate://self/resources/icons/icon_text.svg"),
            }
        }
        
        filler: {
            fn pixel(self) -> vec4 {
                return mix(
                    THEME_COLOR_BG_EVEN,
                    THEME_COLOR_BG_ODD,
                    self.is_even
                );
            }
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook, LiveRegister)]#[repr(C)]
struct DrawNodeQuad {
    #[deref] draw_super: DrawQuad,
    #[live] is_even: f32,
    #[live] scale: f32,
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook, LiveRegister)]#[repr(C)]
struct DrawNodeIcon {
    #[deref] draw_super: DrawIcon,
    #[live] scale: f32,
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook, LiveRegister)]#[repr(C)]
struct DrawNodeText {
    #[deref] draw_super: DrawText,
    #[live] scale: f32,
}

#[derive(Live, LiveHook, LiveRegister)]
pub struct DesignerOutlineTreeNode {
    #[live] draw_bg: DrawNodeQuad,
    #[live] button_open: FoldButton,
    #[live] draw_icon: DrawNodeIcon,
    #[live] draw_name: DrawNodeText,
    #[live] icon_walk: Walk,
    #[live] button_open_width: f64,
    #[live] draw_eye: bool,
    #[layout] layout: Layout,
    
    #[animator] animator: Animator,
    
    #[live] min_drag_distance: f64,
    #[live] indent_width: f64,
    #[live] indent_shift: f64,
    
    #[live] selected: f64,
    #[live] opened: f64,
    #[live] hover: f64,
    #[live] focussed: f64
}

#[derive(Live, Widget)]
pub struct DesignerOutlineTree {
    #[redraw] #[live] scroll_bars: ScrollBars,
    
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
        
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[live] filler: DrawNodeQuad,
    
    #[live] node_height: f64,
    
    #[live] draw_scroll_shadow: DrawScrollShadow,
    
    #[rust] draw_state: DrawStateWrap<()>,
    
    #[rust] dragging_node_id: Option<LiveId>,
    #[rust] selected_node_id: Option<LiveId>,
    #[rust] open_nodes: HashSet<LiveId>,
    #[rust] scroll_into_view_id: Option<LiveId>,
    #[rust] scroll_into_view_rect: Option<Rect>,
        
    #[rust] tree_nodes: ComponentMap<LiveId, (DesignerOutlineTreeNode, LiveId)>,
    
    #[rust] count: usize,
    #[rust] stack: Vec<f64>,
}

impl LiveHook for DesignerOutlineTree {
    fn before_apply(&mut self, _cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc {..} = apply.from {
            self.templates.clear();
        }
    }
        
    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        if nodes[index].is_instance_prop() {
            if let Some(live_ptr) = apply.from.to_live_ptr(cx, index){
                let id = nodes[index].id;
                self.templates.insert(id, live_ptr);
                for (_, (node, templ_id)) in self.tree_nodes.iter_mut() {
                    if *templ_id == id {
                        let opened = node.opened;
                        node.apply(cx, apply, index, nodes);
                        node.opened = opened;
                    }
                }
            }
        }
        else {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }
    
}

#[derive(Clone, Debug, DefaultNone)]
pub enum OutlineTreeAction {
    None,
    Selected(LiveId, KeyModifiers),
    EyeClicked(LiveId, bool),
    ShouldStartDrag(LiveId),
}

pub enum OutlineTreeNodeAction {
    Selected(KeyModifiers),
    Opening,
    Closing,
    ShouldStartDrag
}

impl DesignerOutlineTreeNode {
    pub fn draw(&mut self, cx: &mut Cx2d, name: &str, is_even: f32, node_height: f64, depth: usize, scale: f64, draw_open_button:bool) {
        self.draw_bg.is_even = is_even;
        self.draw_bg.scale = scale as f32;
        self.draw_icon.scale = scale as f32;
        self.draw_name.scale = scale as f32;
                        
        
        self.draw_bg.begin(cx, Walk::size(Size::Fill, Size::Fixed(scale * node_height)), self.layout);
                
        cx.walk_turtle(self.indent_walk(depth));
        if draw_open_button{
            self.button_open.draw_all(cx, &mut Scope::empty());
        }
        else{
            cx.walk_turtle(Walk::fixed(self.button_open_width,0.0));
        }
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_name.draw_walk(cx, Walk::fit(), Align::default(), name);
        self.draw_bg.end(cx);
    }
    
    fn indent_walk(&self, depth: usize) -> Walk {
        Walk {
            abs_pos: None,
            width: Size::Fixed(depth as f64 * self.indent_width + self.indent_shift),
            height: Size::Fixed(0.0),
            margin: Margin::default()
        }
    }
    
    fn set_is_selected(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.animator_toggle(cx, is, animate, id!(select.on), id!(select.off))
    }
    
    fn set_is_focussed(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.animator_toggle(cx, is, animate, id!(focus.on), id!(focus.off))
    }
    
    pub fn set_is_open(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        if is{
            self.opened = 1.0
        }
        else{
            self.opened = 0.0
        }
        self.button_open.animator_toggle(cx, is, animate, id!(open.on), id!(open.off));
    }
    
    pub fn set_folder_is_open(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.animator_toggle(cx, is, animate, id!(open.on), id!(open.off));
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        node_id: LiveId,
        scope: &mut Scope,
        actions_out: &mut Vec<(LiveId, OutlineTreeNodeAction)>,
    ) {
        let actions = cx.capture_actions(|cx|{
            self.button_open.handle_event(cx, event, scope);
        });
        
        /*if let Some(km) = self.button_name.pressed_modifiers(&actions){
            actions_out.push((node_id, OutlineTreeNodeAction::NamePressed(km)));
        }*/
        
        if let Some(anim) = self.button_open.animating(&actions){
            self.opened = anim;
            self.draw_bg.redraw(cx);
        }
        if self.button_open.opening(&actions){
            actions_out.push((node_id, OutlineTreeNodeAction::Opening));
        }
        if self.button_open.closing(&actions){
            actions_out.push((node_id, OutlineTreeNodeAction::Closing));
        }
        
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
               self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerMove(f) => {
                if f.abs.distance(&f.abs_start) >= self.min_drag_distance {
                    actions_out.push((node_id, OutlineTreeNodeAction::ShouldStartDrag));
                }
            }
            Hit::FingerDown(e) => {
                self.animator_play(cx, id!(select.on));
                actions_out.push((node_id, OutlineTreeNodeAction::Selected(e.modifiers)));
                /*
                if self.is_folder {
                    if self.animator_in_state(cx, id!(open.on)) {
                        self.animator_play(cx, id!(open.off));
                        actions.push((node_id, OutlineTreeNodeAction::Closing));
                    }
                    else {
                        self.animator_play(cx, id!(open.on));
                        actions.push((node_id, OutlineTreeNodeAction::Opening));
                    }
                }
                actions.push((node_id, OutlineTreeNodeAction::WasClicked));*/
            }
            _ => {}
        }
    }
}

impl DesignerOutlineTree {
    
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
            self.filler.is_even = Self::is_even_as_f32(self.count);
            self.filler.draw_walk(cx, Walk::size(Size::Fill, Size::Fixed(self.node_height.min(height_left - walk))));
            walk += self.node_height.max(1.0);
        }
        
        self.draw_scroll_shadow.draw(cx, dvec2(0., 0.));
        self.scroll_bars.end(cx);
        
        if let Some(rect) = self.scroll_into_view_rect.take(){
            let rect = rect.add_margin(dvec2(0.0,self.node_height*3.0)).translate(self.scroll_bars.get_scroll_pos());
            self.scroll_bars.scroll_into_view_abs(cx, rect);
        }
        
        let selected_node_id = self.selected_node_id;
        self.tree_nodes.retain_visible_and( | node_id, _ | Some(*node_id) == selected_node_id);
    }
    
    pub fn is_even_as_f32(count: usize) -> f32 {
        if count % 2 == 1 {0.0}else {1.0}
    }
    
    pub fn should_node_draw(&mut self, node_id: LiveId, cx: &mut Cx2d) -> bool {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        let height = self.node_height * scale;
        let walk = Walk::size(Size::Fill, Size::Fixed(height));
        if scale > 0.01 && cx.walk_turtle_would_be_visible(walk) {
            if let Some(view_id) = &self.scroll_into_view_id{
                if *view_id == node_id{
                    self.scroll_into_view_id.take();
                }
            }
            return true
        }
        else {
            // alright so the node is NOT visible. what if we should be
            let rect = cx.walk_turtle(walk);
            if let Some(view_id) = &self.scroll_into_view_id{
                if *view_id == node_id{
                    self.scroll_into_view_id.take();
                    self.scroll_into_view_rect = Some(rect);
                }
            }
            return false
        }
    }
    
    pub fn begin_node(
        &mut self,
        cx: &mut Cx2d,
        node_id: LiveId,
        name: &str,
        template: LiveId,
    ) -> Result<(), ()> {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        if scale > 0.2 {
            self.count += 1;
        }
        
        let is_open = self.open_nodes.contains(&node_id);
        let is_selected = self.selected_node_id == Some(node_id);
        if self.should_node_draw(node_id, cx) {
            // lets create the node
            if let Some(ptr) = self.templates.get(&template){
                let (tree_node, _) = self.tree_nodes.get_or_insert(cx, node_id, | cx | {
                    let mut tree_node = DesignerOutlineTreeNode::new_from_ptr(cx, Some(*ptr));
                    if is_open {
                        tree_node.set_is_open(cx, true, Animate::No)
                    }
                    if is_selected{
                        tree_node.set_is_selected(cx, true, Animate::No)
                    }
                    (tree_node, template)
                });
                tree_node.draw(cx, name, Self::is_even_as_f32(self.count), self.node_height, self.stack.len(), scale, true);
                self.stack.push(tree_node.opened as f64 * scale);
                if tree_node.opened <= 0.001 {
                    self.end_node();
                    return Err(());
                }
            }
            else{
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
    
    pub fn end_node(&mut self) {
        self.stack.pop();
    }
    
    pub fn node(&mut self, cx: &mut Cx2d, node_id: LiveId, name: &str, template: LiveId) {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        if scale > 0.2 {
            self.count += 1;
        }
        let is_selected = self.selected_node_id == Some(node_id);
        if self.should_node_draw(node_id, cx) {
            if let Some(ptr) = self.templates.get(&template){
                let (tree_node, _) = self.tree_nodes.get_or_insert(cx, node_id, | cx | {
                    let mut tree_node = DesignerOutlineTreeNode::new_from_ptr(cx, Some(*ptr));
                    if is_selected{
                        tree_node.set_is_selected(cx, true, Animate::No)
                    }
                    (tree_node, template)
                    
                });
                tree_node.draw(cx, name, Self::is_even_as_f32(self.count), self.node_height, self.stack.len(), scale, false);
            }
        }
    }
    
    pub fn forget(&mut self) {
        self.tree_nodes.clear();
    }
    
    pub fn forget_node(&mut self, file_node_id: LiveId) {
        self.tree_nodes.remove(&file_node_id);
    }
    
    pub fn select_and_show_node(&mut self, cx:&mut Cx, id_path:&[LiveId])  {
        if id_path.len() == 0{
            return
        }
        for i in 0..id_path.len()-1{
            let id = id_path[i];
            self.open_nodes.insert(id);
            if let Some((tree_node,_)) = self.tree_nodes.get_mut(&id){
                tree_node.set_is_open(cx, true, Animate::No);
            }
        }
        let last = *id_path.last().unwrap();
        self.scroll_into_view_id = Some(last);
        self.selected_node_id = Some(last);
        
        for (id,(tree_node,_)) in self.tree_nodes.iter_mut(){
            if *id == last{
                tree_node.set_is_selected(cx, true, Animate::No);
            }
            else{
                tree_node.set_is_selected(cx, false, Animate::No);
            }
        }
        self.redraw(cx);
    }
    
    pub fn start_dragging_file_node(
        &mut self,
        cx: &mut Cx,
        node_id: LiveId,
        items: Vec<DragItem>,
    ) {
        self.dragging_node_id = Some(node_id);

        log!("makepad: start_dragging_file_node");

        cx.start_dragging(items);
    }
    
    pub fn set_folder_is_open(
        &mut self,
        cx: &mut Cx,
        node_id: LiveId,
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
    /*
    pub fn set_open_by_path(
        &mut self,
        cx: &mut Cx,
        path: &str,
        is_open: bool,
        animate: Animate,
    ) {
        for (idx,_) in path.match_indices('/'){
            let slice = &path[0..idx+1];
            let hash =  LiveId::from_str(slice).into();
            self.set_folder_is_open(cx, hash, is_open, animate);
        }
    }*/
/*    
    let folder1 =  LiveId::from_str("examples/").into();
    file_tree.set_folder_is_open(cx, folder1, true, Animate::No);
    let folder1 =  LiveId::from_str("examples/ironfish/").into();
    file_tree.set_folder_is_open(cx, folder1, true, Animate::No);
    let folder1 =  LiveId::from_str("examples/ironfish/src/app_desktop.rs/").into();
    file_tree.set_folder_is_open(cx, folder1, true, Animate::No);
*/    
}

//pub type LiveId = LiveId;
//#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
//pub struct LiveId(pub LiveId);

impl Widget for DesignerOutlineTree {

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        
        self.scroll_bars.handle_event(cx, event, scope);
                
        match event {
            Event::DragEnd => self.dragging_node_id = None,
            _ => ()
        }
        
        let mut node_actions = Vec::new();
                
        for (node_id, (node, _)) in self.tree_nodes.iter_mut() {
            node.handle_event(cx, event, *node_id, scope, &mut node_actions);
        }
                
        for (node_id, node_action) in node_actions {
            match node_action {
                OutlineTreeNodeAction::Opening => {
                    self.open_nodes.insert(node_id);
                }
                OutlineTreeNodeAction::Closing => {
                    self.open_nodes.remove(&node_id);
                }
                /*OutlineTreeNodeAction::EyeClicked(_checked) => {
                    
                }*/
                OutlineTreeNodeAction::Selected(km) => {
                    cx.widget_action(uid, &scope.path, OutlineTreeAction::Selected(node_id, km));
                    cx.set_key_focus(self.scroll_bars.area());
                    if let Some(last_selected) = self.selected_node_id {
                        if last_selected != node_id {
                            if let Some(node) = self.tree_nodes.get_mut(&last_selected){
                                node.0.set_is_selected(cx, false, Animate::Yes);
                            }
                        }
                    }
                    self.selected_node_id = Some(node_id);
                    //cx.widget_action(uid, &scope.path, OutlineTreeAction::LinkClicked(node_id));
                }
                OutlineTreeNodeAction::ShouldStartDrag => {
                    if self.dragging_node_id.is_none() {
                        cx.widget_action(uid, &scope.path, OutlineTreeAction::ShouldStartDrag(node_id));
                    }
                }
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
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope,walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            self.begin(cx, walk);
            return DrawStep::make_step()
        }
        if let Some(()) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl DesignerOutlineTreeRef{
    pub fn should_file_start_drag(&self, actions: &Actions) -> Option<LiveId> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let OutlineTreeAction::ShouldStartDrag(file_id) = item.cast() {
                return Some(file_id)
            }
        }
        None
    }
    /*
    pub fn file_c/licked(&self, actions: &Actions) -> Option<LiveId> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let OutlineTreeAction::FileClicked(file_id) = item.cast() {
                return Some(file_id)
            }
        }
        None
    }*/
    
        
    pub fn select_and_show_node(&self, cx:&mut Cx, id_path:&[LiveId])  {
        if let Some(mut inner) = self.borrow_mut(){
            inner.select_and_show_node(cx, id_path);
        }
    }
        
    
    pub fn selected(&self, actions: &Actions) -> Option<(LiveId,KeyModifiers)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let OutlineTreeAction::Selected(file_id, km) = item.cast() {
                return Some((file_id,km))
            }
        }
        None
    }
    
    
    pub fn start_drag(&self, cx: &mut Cx, _file_id: LiveId, item: DragItem) {
        cx.start_dragging(vec![item]);
    }
}