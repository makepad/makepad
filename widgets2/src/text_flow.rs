use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    animator::*,
    widget::*,
}; 

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    let FlowBlockType = set_type_default() do #(FlowBlockType::script_api(vm))
    
    mod.widgets.DrawFlowBlock = set_type_default() do #(DrawFlowBlock::script_shader(vm)){
        ..mod.draw.DrawQuad
        
        block_type: instance(FlowBlockType.Quote)
        line_color: #fff
        sep_color: #888
        code_color: #333
        quote_bg_color: #222
        quote_fg_color: #aaa
        
        space_1: uniform(4.0)
        space_2: uniform(8.0)
    }
    
    mod.widgets.FlowBlockType = FlowBlockType
    
    mod.widgets.TextFlowBase = #(TextFlow::register_widget(vm)){
        font_size: 8
        flow: Flow.Right{wrap: true}
    }
    
    mod.widgets.TextFlowLinkBase = #(TextFlowLink::register_widget(vm)){}
    
    mod.widgets.TextFlowLink = set_type_default() do mod.widgets.TextFlowLinkBase{
        color: #xa
        color_hover: #xf
        color_down: #x3
        margin: Inset{right: 5}
        
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: 0.0
                        down: 0.0
                    }
                }
                
                on: AnimatorState{
                    redraw: true
                    from: {
                        all: Forward {duration: 0.1}
                        down: Forward {duration: 0.01}
                    }
                    apply: {
                        hovered: snap(1.0)
                        down: snap(1.0)
                    }
                }
                                
                down: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: snap(1.0)
                        down: snap(1.0)
                    }
                }
            }
        }
    }
        
    mod.widgets.TextFlow = set_type_default() do mod.widgets.TextFlowBase{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        padding: 0
                
        font_size: theme.font_size_p
        font_color: theme.color_text
                
        draw_normal +: {
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
            color: theme.color_text
        }
                
        draw_italic +: {
            text_style: theme.font_italic{
                font_size: theme.font_size_p
            }
            color: theme.color_text
        }
                
        draw_bold +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
            }
            color: theme.color_text
        }
                
        draw_bold_italic +: {
            text_style: theme.font_bold_italic{
                font_size: theme.font_size_p
            }
            color: theme.color_text
        }
                
        draw_fixed +: {
            text_style: theme.font_code{
                font_size: theme.font_size_p
            }
            color: theme.color_text
        }
                
        code_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        code_walk: Walk{width: Fill, height: Fit}
                
        quote_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        quote_walk: Walk{width: Fill, height: Fit}
                
        list_item_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: theme.mspace_1
        }
        list_item_walk: Walk{
            height: Fit width: Fill
        }
                
        inline_code_padding: theme.mspace_1
        inline_code_margin: theme.mspace_1
                
        sep_walk: Walk{
            width: Fill height: 4.
            margin: theme.mspace_v_1
        }
                
        $link: mod.widgets.TextFlowLink{}
                
        draw_block +: {
            line_color: theme.color_text
            sep_color: theme.color_shadow
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_text
            code_color: theme.color_bg_highlight
            space_1: uniform(theme.space_1)
            space_2: uniform(theme.space_2)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                match self.block_type {
                    FlowBlockType.Quote => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.quote_bg_color)
                        sdf.box(self.space_1 self.space_1 self.space_1 self.rect_size.y-self.space_2 1.5)
                        sdf.fill(self.quote_fg_color)
                        return sdf.result
                    }
                    FlowBlockType.Sep => {
                        sdf.box(0. 1. self.rect_size.x-1. self.rect_size.y-2. 2.)
                        sdf.fill(self.sep_color)
                        return sdf.result
                    }
                    FlowBlockType.Code => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.InlineCode => {
                        sdf.box(1. 1. self.rect_size.x-2. self.rect_size.y-2. 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.Underline => {
                        sdf.box(0. self.rect_size.y-2. self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                    FlowBlockType.Strikethrough => {
                        sdf.box(0. self.rect_size.y * 0.45 self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                }
                return #f00
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(u32)]
pub enum FlowBlockType {
    #[pick] Quote = 1,
    Sep = 2,
    Code = 3,
    InlineCode = 4,
    Underline = 5,
    Strikethrough = 6
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFlowBlock {
    #[deref] draw_super: DrawQuad,
    #[live] pub line_color: Vec4f,
    #[live] pub sep_color: Vec4f,
    #[live] pub code_color: Vec4f,
    #[live] pub quote_bg_color: Vec4f,
    #[live] pub quote_fg_color: Vec4f,
    #[live] pub block_type: FlowBlockType
}

#[derive(Default)]
pub struct StackCounter(usize);
impl StackCounter{
    pub fn push(&mut self){
        self.0 += 1;
    }
    pub fn pop(&mut self){
        if self.0 > 0{
            self.0 -=1;
        }
    }
    pub fn clear(&mut self){
        self.0 = 0
    }
    pub fn value(&self)->usize{
        self.0
    }
}
      
// this widget has a retained and an immediate mode api
#[derive(Script, Widget)]
pub struct TextFlow {
    #[live] pub draw_normal: DrawText,
    #[live] pub draw_italic: DrawText,
    #[live] pub draw_bold: DrawText,
    #[live] pub draw_bold_italic: DrawText,
    #[live] pub draw_fixed: DrawText,
    #[live] pub draw_block: DrawFlowBlock,
    
    /// The default font size used for all text if not otherwise specified.
    #[live] pub font_size: f32,
    /// The default font color used for all text if not otherwise specified.
    #[live] pub font_color: Vec4f,
    #[walk] walk: Walk,
    
    #[rust] area_stack: SmallVec<[Area;4]>,
    #[rust] pub font_sizes: SmallVec<[f32;8]>,
    #[rust] pub font_colors: SmallVec<[Vec4f;8]>,
    #[rust] pub combine_spaces: SmallVec<[bool;4]>,
    #[rust] pub ignore_newlines: SmallVec<[bool;4]>,
    #[rust] pub bold: StackCounter,
    #[rust] pub italic: StackCounter,
    #[rust] pub fixed: StackCounter,
    #[rust] pub underline: StackCounter,
    #[rust] pub strikethrough: StackCounter,
    #[rust] pub inline_code: StackCounter,
        
    #[rust] pub item_counter: u64,
    #[rust] pub first_thing_on_a_line: bool,
    
    #[rust] pub areas_tracker: RectAreasTracker,
    
    #[layout] layout: Layout,
    
    #[live] quote_layout: Layout,
    #[live] quote_walk: Walk,
    #[live] code_layout: Layout,
    #[live] code_walk: Walk,
    #[live] sep_walk: Walk, 
    #[live] list_item_layout: Layout,
    #[live] list_item_walk: Walk,
    #[live] pub inline_code_padding: Inset,
    #[live] pub inline_code_margin: Inset,
    #[live(Inset{top:0.5,bottom:0.5,left:0.0,right:0.0})] pub heading_margin: Inset,
    #[live(Inset{top:0.5,bottom:0.5,left:0.0,right:0.0})] pub paragraph_margin: Inset,
    
    #[redraw] #[rust] area:Area,
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust(Some(Default::default()))] items: Option<ComponentMap<LiveId,(WidgetRef, LiveId)>>,
    #[rust] templates: ComponentMap<LiveId, ScriptValue>,
}

impl TextFlow {
    fn apply_template(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope, id: LiveId, obj: ScriptValue) {
        self.templates.insert(id, obj);
        // Apply to existing items with matching template
        for (node, templ_id) in self.items.as_mut().unwrap().values_mut() {
            if *templ_id == id {
                node.script_apply(vm, apply, scope, obj);
            }
        }
    }
}

impl ScriptHook for TextFlow {
    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope, value: ScriptValue) {
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    if let Some(id) = kv.key.as_id() {
                        if kv.value.as_object().is_some() {
                            self.apply_template(vm, apply, scope, id, kv.value);
                        }
                    }
                }
            });
        }
    }
}

#[derive(Default)]
pub struct RectAreasTracker{
    pub areas: SmallVec<[Area;4]>,
    pos: usize,
    stack: SmallVec<[usize;2]>,
}

impl RectAreasTracker{
    fn clear_stack(&mut self){
        self.pos = 0;
        self.areas.clear();
        self.stack.clear();
    }
    
    pub fn push_tracker(&mut self){
        self.stack.push(self.pos);
    }
    
    // this returns the range in the area vec    
    pub fn pop_tracker(&mut self)->(usize, usize){
        return (self.stack.pop().unwrap(), self.pos)
    }
    
    pub fn track_rect(&mut self, cx:&mut Cx2d, rect:Rect){
        if self.stack.len() >0{
            if self.pos >= self.areas.len(){
                self.areas.push(Area::Empty);
            }
            cx.add_aligned_rect_area(&mut self.areas[self.pos], rect);
            self.pos += 1;
        }
    }
}

#[derive(Clone)]
enum DrawState {
    Begin,
    Drawing,
}

impl Widget for TextFlow {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        if self.draw_state.begin(cx, DrawState::Begin) {
            self.begin(cx, walk);
            return DrawStep::make_step()
        }
        if let Some(_) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for (id,(entry,_)) in self.items.as_mut().unwrap().iter_mut(){
            scope.with_id(*id, |scope| {
                entry.handle_event(cx, event, scope);
            });
        }
    }
}

impl TextFlow{
    pub fn begin(&mut self, cx: &mut Cx2d, walk:Walk){
        cx.begin_turtle(walk, self.layout);
        self.draw_state.set(DrawState::Drawing);
        self.draw_block.append_to_draw_call(cx);
        self.clear_stacks();
    }
    
    fn clear_stacks(&mut self){
        self.item_counter = 0;
        self.areas_tracker.clear_stack();
        self.bold.clear();
        self.italic.clear();
        self.fixed.clear();
        self.underline.clear();
        self.strikethrough.clear();
        self.inline_code.clear();
        self.font_sizes.clear();
        self.font_colors.clear();
        self.area_stack.clear();
        self.combine_spaces.clear();
        self.ignore_newlines.clear();
        self.first_thing_on_a_line = true;
    }
    
        
    pub fn push_size_rel_scale(&mut self, scale: f64){
        self.font_sizes.push(
            self.font_sizes.last().unwrap_or(&self.font_size) * (scale as f32)
        );
    }
            
    pub fn push_size_abs_scale(&mut self, scale: f64){
        self.font_sizes.push(
            self.font_size * (scale  as f32)
        );
    }

    pub fn end(&mut self, cx: &mut Cx2d){
        cx.end_turtle_with_area(&mut self.area);
        self.items.as_mut().unwrap().retain_visible();
    } 

    pub fn begin_code(&mut self, cx:&mut Cx2d){
        self.draw_block.block_type = FlowBlockType::Code;
        self.draw_block.begin(cx, self.code_walk, self.code_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
        self.first_thing_on_a_line = true;
    }
    
    pub fn end_code(&mut self, cx:&mut Cx2d){
        self.draw_block.draw_vars.area = self.area_stack.pop().unwrap();
        self.draw_block.end(cx);
    }
    
    pub fn begin_list_item(&mut self, cx:&mut Cx2d, dot:&str, pad:f64){
        let fs = self.font_sizes.last().unwrap_or(&self.font_size);
        self.draw_normal.text_style.font_size = *fs as _;
        let fc = self.font_colors.last().unwrap_or(&self.font_color);
        self.draw_normal.color = *fc;

        let font_based_padding = self.draw_normal.text_style.font_size as f64 * pad;

        cx.begin_turtle(self.list_item_walk, Layout{
            padding:Inset{
                left: self.list_item_layout.padding.left + font_based_padding,
                ..self.list_item_layout.padding
            },
            ..self.list_item_layout
        });

        cx.turtle_mut().move_right_down(dvec2(-font_based_padding, 0.0));

        self.draw_text(cx, dot);
        self.draw_text(cx, " ");
        
        self.area_stack.push(self.draw_block.draw_vars.area);
    }
    
    pub fn end_list_item(&mut self, cx:&mut Cx2d){
        cx.end_turtle();
        self.first_thing_on_a_line = true;
    }
    
    pub fn new_line_collapsed(&mut self, cx:&mut Cx2d){
        cx.turtle_new_line();
        self.first_thing_on_a_line = true;
    }
    
    pub fn new_line_collapsed_with_spacing(&mut self, cx:&mut Cx2d, spacing: f64){
        cx.turtle_new_line_with_spacing(spacing);
        self.first_thing_on_a_line = true;
    }
    
    pub fn sep(&mut self, cx:&mut Cx2d){
        self.draw_block.block_type = FlowBlockType::Sep;
        self.draw_block.draw_walk(cx, self.sep_walk);
    }
    
    pub fn begin_quote(&mut self, cx:&mut Cx2d){
        self.draw_block.block_type = FlowBlockType::Quote;
        self.draw_block.begin(cx, self.quote_walk, self.quote_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
    }
        
    pub fn end_quote(&mut self, cx:&mut Cx2d){
        self.draw_block.draw_vars.area = self.area_stack.pop().unwrap();
        self.draw_block.end(cx);
    }
    
    pub fn draw_item_counted(&mut self, cx: &mut Cx2d, template: LiveId,)->LiveId{
        let entry_id = self.new_counted_id();
        self.item_with(cx, entry_id, template, |cx, item, tf|{
            item.draw_all(cx, &mut Scope::with_data(tf));
        });
        entry_id
    }
    
    pub fn new_counted_id(&mut self)->LiveId{
        self.item_counter += 1;
        LiveId(self.item_counter)
    }
    
    pub fn draw_item(&mut self, cx: &mut Cx2d, entry_id: LiveId, template: LiveId){
        self.item_with(cx, entry_id, template, |cx, item, tf|{
            item.draw_all(cx, &mut Scope::with_data(tf));
        });
    }
    
    pub fn draw_item_counted_ref(&mut self, cx: &mut Cx2d, template: LiveId,)->WidgetRef{
        let entry_id = self.new_counted_id();
        self.item_with(cx, entry_id, template, |cx, item, tf|{
            item.draw_all(cx, &mut Scope::with_data(tf));
            item.clone()
        })
    }
        
    pub fn draw_item_ref(&mut self, cx: &mut Cx2d, entry_id: LiveId, template: LiveId)->WidgetRef{
        self.item_with(cx, entry_id, template, |cx, item, tf|{
            item.draw_all(cx, &mut Scope::with_data(tf));
            item.clone()
        })
    }
    
    pub fn item_with<F,R:Default>(&mut self, cx: &mut Cx2d, entry_id:LiveId, template: LiveId, f:F)->R
    where F:FnOnce(&mut Cx2d, &WidgetRef, &mut TextFlow)->R{
        let mut items = self.items.take().unwrap();
        let r = if let Some(template_value) = self.templates.get(&template).copied() {
            let entry = items.get_or_insert(cx, entry_id, | cx | {
                let widget = cx.with_vm(|vm| {
                    WidgetRef::script_from_value(vm, template_value)
                });
                (widget, template)
            });
            f(cx, &entry.0, self)
        }else{
            R::default()
        };
        self.items = Some(items);
        r
    }
        
    
    pub fn item(&mut self, cx: &mut Cx, entry_id: LiveId, template: LiveId) -> WidgetRef {
        if let Some(template_value) = self.templates.get(&template).copied() {
            let entry = self.items.as_mut().unwrap().get_or_insert(cx, entry_id, | cx | {
                let widget = cx.with_vm(|vm| {
                    WidgetRef::script_from_value(vm, template_value)
                });
                (widget, template)
            });
            return entry.0.clone()
        }
        WidgetRef::empty() 
    }
    
    
    pub fn item_counted(&mut self, cx: &mut Cx, template: LiveId) -> WidgetRef {
        let entry_id = self.new_counted_id();
        if let Some(template_value) = self.templates.get(&template).copied() {
            let entry = self.items.as_mut().unwrap().get_or_insert(cx, entry_id, | cx | {
                let widget = cx.with_vm(|vm| {
                    WidgetRef::script_from_value(vm, template_value)
                });
                (widget, template)
            });
            return entry.0.clone()
        }
        WidgetRef::empty() 
    }
    
    pub fn existing_item(&mut self, entry_id: LiveId) -> WidgetRef {
        if let Some(item) = self.items.as_mut().unwrap().get(&entry_id){
            item.0.clone()
        }
        else{
            WidgetRef::empty()
        }
    }
        
    pub fn clear_items(&mut self){
        self.items.as_mut().unwrap().clear();
    }
        

    pub fn item_with_scope(&mut self, cx: &mut Cx, scope: &mut Scope, entry_id: LiveId, template: LiveId) -> Option<WidgetRef> {
        if let Some(template_value) = self.templates.get(&template).copied() {
            let entry = self.items.as_mut().unwrap().get_or_insert(cx, entry_id, | cx | {
                let widget = cx.with_vm(|vm| {
                    WidgetRef::script_from_value_scoped(vm, scope, template_value)
                });
                (widget, template)
            });
            return Some(entry.0.clone())
        }
        None 
    }
     
    pub fn draw_text(&mut self, cx:&mut Cx2d, text:&str){
        if let Some(DrawState::Drawing) = self.draw_state.get(){
            
            if (text == " " || text == "") && self.first_thing_on_a_line{
                return
            }
            let text = if self.first_thing_on_a_line{
                text.trim_start().trim_end_matches("\n")
            }
            else{
                text.trim_end_matches("\n")
            };
            
            let dt = if self.fixed.value() > 0{
                &mut self.draw_fixed
            }
            else if self.bold.value() > 0{
                if self.italic.value() > 0{
                    &mut self.draw_bold_italic
                }
                else{
                    &mut self.draw_bold
                }
            }
            else if self.italic.value() > 0{
                    &mut self.draw_italic
            }
            else{
                &mut self.draw_normal
            };
            let font_size = self.font_sizes.last().unwrap_or(&self.font_size);
            let font_color = self.font_colors.last().unwrap_or(&self.font_color);
            dt.text_style.font_size = *font_size as _;
            dt.color = *font_color;
           
            let areas_tracker = &mut self.areas_tracker;
            if self.inline_code.value() > 0{
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::InlineCode;
                if !self.first_thing_on_a_line{
                    let rect = TextFlow::walk_margin(cx, self.inline_code_margin.left);
                    areas_tracker.track_rect(cx, rect);
                }
                dt.draw_walk_resumable_with(cx, text, |cx, mut rect, _|{
                    rect.pos -= self.inline_code_padding.left_top();
                    rect.size += self.inline_code_padding.size();
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
                let rect = TextFlow::walk_margin(cx, self.inline_code_margin.right);
                areas_tracker.track_rect(cx, rect);
            }
            else if self.strikethrough.value() > 0{
                let db = &mut self.draw_block;
                db.line_color = *font_color;
                db.block_type = FlowBlockType::Strikethrough;
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _|{
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
            }
            else if self.underline.value() > 0{
                let db = &mut self.draw_block;
                db.line_color = *font_color;
                db.block_type = FlowBlockType::Underline;
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _|{
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
            }
            else{
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _|{
                    areas_tracker.track_rect(cx, rect);
                });
            }
        }
        self.first_thing_on_a_line = false;
        
    }
    
    pub fn walk_margin(cx:&mut Cx2d, margin:f64)->Rect{
        cx.walk_turtle(Walk{
            width: Size::Fixed(margin),
            height: Size::Fixed(0.0),
            ..Default::default()
        })
    }
    
    pub fn draw_link(&mut self, cx:&mut Cx2d, template:LiveId, data:impl ActionTrait + PartialEq, label:&str){
        let entry_id = self.new_counted_id();
        self.item_with(cx, entry_id, template, |cx, item, tf|{
            item.set_text(cx, label);
            item.set_action_data(data);
            item.draw_all(cx, &mut Scope::with_data(tf));
        })
    }
}

#[derive(Debug, Clone, Default)]
pub enum TextFlowLinkAction {
    Clicked {
        key_modifiers: KeyModifiers,
    },
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget, Animator)]
struct TextFlowLink {
    #[source] source: ScriptObjectRef,
    #[apply_default] animator: Animator,
    
    #[redraw] #[area] area: Area,
    
    #[live(true)] click_on_down: bool,
    #[rust] drawn_areas: SmallVec<[Area; 2]>,
    #[live(true)] grab_key_focus: bool,
    #[live] margin: Inset,
    #[live] hovered: f32,
    #[live] down: f32,
    
    /// The default font color for the link when not hovered on or down.
    #[live] color: Option<Vec4f>,
    /// The font color used when the link is hovered on.
    #[live] color_hover: Option<Vec4f>,
    /// The font color used when the link is down.
    #[live] color_down: Option<Vec4f>,
    
    #[live] pub text: ArcStringMut,
        
    #[action_data] #[rust] action_data: WidgetActionData,
}

impl Widget for TextFlowLink {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            if let Some(tf) = scope.data.get_mut::<TextFlow>() {
                tf.redraw(cx);
            } else {
                self.drawn_areas.iter().for_each(|area| area.redraw(cx));
            }
        }
        
        for area in self.drawn_areas.clone().into_iter() {
            match event.hits(cx, area) {
                Hit::FingerDown(fe) if fe.is_primary_hit() => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.area());
                    }
                    self.animator_play(cx, ids!(hover.down));
                    if self.click_on_down{
                        cx.widget_action_with_data(
                            &self.action_data,
                            self.widget_uid(),
                            &scope.path,
                            TextFlowLinkAction::Clicked {
                                key_modifiers: fe.modifiers,
                            },
                        );
                    }
                }
                Hit::FingerHoverIn(_) => {
                    cx.set_cursor(MouseCursor::Hand);
                    self.animator_play(cx, ids!(hover.on));
                }
                Hit::FingerHoverOut(_) => {
                    self.animator_play(cx, ids!(hover.off));
                }
                Hit::FingerUp(fe) if fe.is_primary_hit() => {
                    if fe.is_over {
                        if !self.click_on_down{
                            cx.widget_action_with_data(
                                &self.action_data,
                                self.widget_uid(),
                                &scope.path,
                                TextFlowLinkAction::Clicked {
                                    key_modifiers: fe.modifiers,
                                },
                            );
                        }
                        
                        if fe.device.has_hovers() {
                            self.animator_play(cx, ids!(hover.on));
                        } else {
                            self.animator_play(cx, ids!(hover.off));
                        }
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
                }
                _ => (),
            }
        }
    }
        
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let Some(tf) = scope.data.get_mut::<TextFlow>() else {
            return DrawStep::done();
        };
        
        // Here: the text flow has already began drawing, so we just need to draw the text.
        tf.underline.push();
        tf.areas_tracker.push_tracker();
        let mut pushed_color = false;
        if self.hovered > 0.0 {
            if let Some(color) = self.color_hover {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        } else if self.down > 0.0 {
            if let Some(color) = self.color_down {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        } else {
            if let Some(color) = self.color {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        }
        TextFlow::walk_margin(cx, self.margin.left);
        tf.draw_text(cx, self.text.as_ref());
        TextFlow::walk_margin(cx, self.margin.right);
                                
        if pushed_color {
            tf.font_colors.pop();
        }
        tf.underline.pop();
        
        let (start, end) = tf.areas_tracker.pop_tracker();
        
        if self.drawn_areas.len() == end-start{
            for i in 0..end-start{
                self.drawn_areas[i] = cx.update_area_refs( self.drawn_areas[i], 
                tf.areas_tracker.areas[i+start]);
            }
        }
        else{
            self.drawn_areas = SmallVec::from(
            &tf.areas_tracker.areas[start..end]
            );
        }
            
        DrawStep::done()
    }
        
    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, cx:&mut Cx, v: &str) {
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }    
}
