use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        text_input::{TextInput, TextInputAction}
    }
};

live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    DrawSlider = {{DrawSlider}} {}
    
    pub SliderBase = {{Slider}} {}
    
    pub Slider = <SliderBase> {
        min: 0.0, max: 1.0,
        step: 0.0,
        label_align: { y: 0.0 }
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        precision: 2,
        height: Fit
        
        draw_slider: {
            instance hover: float
            instance focus: float
            instance drag: float
            instance label_size: 0.0
            
            fn pixel(self) -> vec4 {
                let slider_height = 3;
                let nub_size = mix(3, 5, self.hover);
                let nubbg_size = mix(0, 13, self.hover)
                
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                
                let slider_bg_color = mix(mix(THEME_COLOR_AMOUNT_TRACK_DEFAULT, THEME_COLOR_AMOUNT_TRACK_HOVER, self.hover), THEME_COLOR_AMOUNT_TRACK_ACTIVE, self.focus);
                let slider_color = mix(
                    mix(THEME_COLOR_AMOUNT_DEFAULT, THEME_COLOR_AMOUNT_HOVER, self.hover),
                THEME_COLOR_AMOUNT_ACTIVE,
                self.focus);
                    
                let nub_color = (THEME_COLOR_SLIDER_NUB_DEFAULT);
                let nubbg_color = mix(THEME_COLOR_SLIDER_NUB_HOVER, THEME_COLOR_SLIDER_NUB_ACTIVE, self.drag);
                    
                match self.slider_type {
                    SliderType::Horizontal => {
                        sdf.rect(0, self.rect_size.y - slider_height * 1.25, self.rect_size.x, slider_height)
                        sdf.fill(slider_bg_color);
                            
                        sdf.rect(0, self.rect_size.y - slider_height * 0.5, self.rect_size.x, slider_height)
                        sdf.fill(THEME_COLOR_BEVEL_LIGHT);
                            
                        sdf.rect(
                            0,
                            self.rect_size.y - slider_height * 1.25,
                            self.slide_pos * (self.rect_size.x - nub_size) + nub_size,
                            slider_height
                        )
                        sdf.fill(slider_color);
                            
                        let nubbg_x = self.slide_pos * (self.rect_size.x - nub_size) - nubbg_size * 0.5 + 0.5 * nub_size;
                        sdf.rect(
                            nubbg_x,
                            self.rect_size.y - slider_height * 1.25,
                            nubbg_size,
                            slider_height
                        )
                        sdf.fill(nubbg_color);
                            
                        // the nub
                        let nub_x = self.slide_pos * (self.rect_size.x - nub_size);
                        sdf.rect(
                            nub_x,
                            self.rect_size.y - slider_height * 1.25,
                            nub_size,
                            slider_height
                        )
                        sdf.fill(nub_color);
                    }
                    SliderType::Vertical => {
                            
                    }
                    SliderType::Rotary => {
                            
                    }
                }
                return sdf.result
            }
        }
            
        draw_text: {
            color: (THEME_COLOR_TEXT_DEFAULT),
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }
            
        label_walk: { width: Fill, height: Fit }
            
        text_input: <TextInput> {
            width: Fit, padding: 0.,
            empty_message: "0",
            is_numeric_only: true,
                
            label_align: {y: 0.},
            margin: { bottom: (THEME_SPACE_2), left: (THEME_SPACE_2) }
            draw_bg: {
                instance radius: 1.0
                instance border_width: 0.0
                instance border_color: (#f00) // TODO: This appears not to do anything.
                instance inset: vec4(0.0, 0.0, 0.0, 0.0)
                instance focus: 0.0,
                color: (THEME_COLOR_D_HIDDEN)
                instance color_selected: (THEME_COLOR_D_HIDDEN)
                    
                fn get_color(self) -> vec4 {
                    return mix(self.color, self.color_selected, self.focus)
                }
                    
                fn get_border_color(self) -> vec4 {
                    return self.border_color
                }
                    
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box(
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                        max(1.0, self.radius)
                    )
                    sdf.fill_keep(self.get_color())
                    if self.border_width > 0.0 {
                        sdf.stroke(self.get_border_color(), self.border_width)
                    }
                    return sdf.result;
                }
            },
        }
            
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: OutQuad
                    apply: {
                        draw_slider: {hover: 0.0}
                        // text_input: { draw_bg: { hover: 0.0}}
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: {hover: 1.0}
                        // text_input: { draw_bg: { hover: 1.0}}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_slider: {drag: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {draw_slider: {drag: 1.0}}
                }
            }
        }
    }
        
    pub SliderBig = <Slider> {
        height: 36
        text: "CutOff1"
        // draw_text: {text_style: <H2_TEXT_BOLD> {}, color: (COLOR_UP_5)}
        text_input: {
            // cursor_margin_bottom: (THEME_SPACE_1),
            // cursor_margin_top: (THEME_SPACE_1),
            // select_pad_edges: (THEME_SPACE_1),
            // cursor_size: (THEME_SPACE_1),
            empty_message: "0",
            is_numeric_only: true,
            draw_bg: {
                color: (THEME_COLOR_D_HIDDEN)
            },
        }
        draw_slider: {
            instance line_color: (THEME_COLOR_AMOUNT_DEFAULT_BIG),
            instance bipolar: 0.0,
            uniform label_size: 0.0,

            fn pixel(self) -> vec4 {
                let nub_size = 3
                    
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let top = 20.0;
                    
                sdf.box(1.0, top, self.rect_size.x - 2, self.rect_size.y - top - 2, 1);
                sdf.fill_keep(
                    mix(
                        mix((THEME_COLOR_INSET_PIT_TOP), (THEME_COLOR_INSET_PIT_BOTTOM) * 0.1, pow(self.pos.y, 1.0)),
                        mix((THEME_COLOR_INSET_PIT_TOP_HOVER) * 1.75, (THEME_COLOR_BEVEL_LIGHT) * 0.1, pow(self.pos.y, 1.0)),
                        self.drag
                    )
                ) // Control backdrop gradient
                    
                sdf.stroke(mix(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_SHADOW * 1.25, self.drag), THEME_COLOR_BEVEL_LIGHT, pow(self.pos.y, 10.0)), 1.0) // Control outline
                let in_side = 5.0;
                let in_top = 5.0; // Ridge: vertical position
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(mix(THEME_COLOR_AMOUNT_TRACK_DEFAULT, THEME_COLOR_AMOUNT_TRACK_ACTIVE, self.drag)); // Ridge color
                let in_top = 7.0;
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 1.5);
                sdf.fill(THEME_COLOR_BEVEL_LIGHT); // Ridge: Rim light catcher
                    
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 9);
                sdf.move_to(mix(in_side + 3.5, self.rect_size.x * 0.5, self.bipolar), top + in_top);
                    
                sdf.line_to(nub_x + in_side + nub_size * 0.5, top + in_top);
                sdf.stroke_keep(mix((THEME_COLOR_U_HIDDEN), self.line_color, self.drag), 1.5)
                sdf.stroke(
                    mix(mix(self.line_color * 0.85, self.line_color, self.hover), THEME_COLOR_AMOUNT_ACTIVE, self.drag),
                    1.5
                )
                    
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 3) - 3;
                sdf.box(nub_x + in_side, top + 1.0, 11, 11, 1.)
                    
                sdf.fill_keep(mix(
                    mix(
                        mix(THEME_COLOR_SLIDER_BIG_NUB_TOP, THEME_COLOR_SLIDER_BIG_NUB_TOP_HOVER, self.hover),
                        mix(THEME_COLOR_SLIDER_BIG_NUB_BOTTOM, THEME_COLOR_SLIDER_BIG_NUB_BOTTOM_HOVER, self.hover),
                        self.pos.y
                    ),
                    mix(THEME_COLOR_SLIDER_BIG_NUB_BOTTOM, THEME_COLOR_SLIDER_BIG_NUB_TOP, pow(self.pos.y, 1.5)),
                    self.drag
                ))
                
                sdf.stroke(
                    mix(
                        mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_LIGHT * 1.2, self.hover),
                        THEME_COLOR_BLACK,
                        pow(self.pos.y, 1.)
                    ),
                    1.
                ); // Nub outline gradient
                
                
                return sdf.result
            }
        }
    }

    pub SliderCompact = <Slider> {
        height: 18.,
        text: "CutOff1",
        // draw_text: {text_style: <H2_TEXT_BOLD> {}, color: (COLOR_UP_5)}

        text_input: {
            empty_message: "0",
            is_numeric_only: true,
            margin: { right: 7.5, top: 1. } 

            draw_text: {
                fn get_color(self) -> vec4 {
                    return
                    mix(
                        mix(
                            mix(THEME_COLOR_U_5, THEME_COLOR_WHITE, self.hover),
                            THEME_COLOR_WHITE,
                            self.focus
                        ),
                        mix(THEME_COLOR_U_5, THEME_COLOR_WHITE, self.hover),
                        self.is_empty
                    )
                }
            }
        }

        draw_slider: {
            uniform peak: 3.0;
            instance bipolar: 0.0;
            uniform color_a: (THEME_COLOR_D_1);
            uniform color_b: (THEME_COLOR_D_4);
            uniform label_size: 75.0;
            offset_left: 75.0;

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let nub_size = 5.;
                let offset_top = 8.5;
                let padding = 5.0;
                let nub_x = self.slide_pos * (self.rect_size.x - 75.0 - (nub_size + padding) * 2.0);

                // Background
                sdf.box(self.offset_left, 0.0, self.rect_size.x - self.offset_left, self.rect_size.y, 5.);
                sdf.fill_keep(
                    mix(
                        mix((THEME_COLOR_D_2), (THEME_COLOR_D_HIDDEN), pow(self.pos.y, 1.0)),
                        mix((THEME_COLOR_D_2), (THEME_COLOR_BEVEL_LIGHT) * 0.1, pow(self.pos.y, 1.0)),
                        self.drag
                    )
                )

                sdf.stroke(mix(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_SHADOW * 1.25, self.drag), THEME_COLOR_BEVEL_LIGHT, pow(self.pos.y, 2.0)), 1.0)

                let offset_l2 = self.offset_left + nub_size + padding;

                // Amount bar
                sdf.move_to(mix(offset_l2, self.rect_size.x, self.bipolar), offset_top);
                sdf.line_to(offset_l2 + nub_x, offset_top);
                sdf.stroke(
                    mix(mix(
                        mix(self.color_a, self.color_b, pow(self.pos.x, self.peak)),
                        mix(self.color_a, self.color_b, pow(self.pos.x, self.peak)), self.hover),
                        mix(self.color_a, self.color_b, pow(self.pos.x, self.peak)),
                        self.drag),
                    6.5
                )

                // Nub
                sdf.circle(offset_l2 + nub_x, self.rect_size.y * 0.45, mix(3., nub_size, self.hover));
                sdf.fill_keep(mix(
                    mix(THEME_COLOR_U_2, THEME_COLOR_U_3, self.hover),
                    THEME_COLOR_U_4,
                    self.drag
                ))
                
                return sdf.result
            }
        }
    }

}

#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum SliderType {
    #[pick] Horizontal = shader_enum(1),
    Vertical = shader_enum(2),
    Rotary = shader_enum(3),
}

impl LiveHook for Slider{
    fn after_new_from_doc(&mut self, _cx:&mut Cx){
        self.set_internal(self.default);
        self.update_text_input();
    }
}


#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawSlider {
    #[deref] draw_super: DrawQuad,
    #[live] offset_left: f32,
    #[live] slide_pos: f32,
    #[live] slide_posr_type: SliderType
}

#[derive(Live, Widget)]
#[designable]
pub struct Slider {
    #[area] #[redraw] #[live] draw_slider: DrawSlider,
    
    #[walk] walk: Walk,
    
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[rust] label_area: Area,
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    #[live] draw_text: DrawText,
    #[live] text: String,
    
    #[live] text_input: TextInput,
    
    #[live] precision: usize,
    
    #[live] min: f64,
    #[live] max: f64,
    #[live] step: f64,
    #[live] default: f64,
    
    #[live] bind: String,

    // Indicates if the label of the slider responds to hover events
    // The primary use case for this kind of emitted actions is for tooltips displaying
    // and it is turned on by default, since this component already consumes finger events
    #[live(true)] hover_actions_enabled: bool,
    
    #[rust] pub relative_value: f64,
    #[rust] pub dragging: Option<f64>,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum SliderAction {
    StartSlide,
    TextSlide(f64),
    Slide(f64),
    EndSlide,
    LabelHoverIn(Rect),
    LabelHoverOut,
    None
}

impl Slider {
    
    fn to_external(&self) -> f64 {
        let val = self.relative_value * (self.max - self.min);
        if self.step != 0.0{
            return (val / self.step).floor()* self.step + self.min
        }
        else{
            val  + self.min
        }
    }
    
    fn set_internal(&mut self, external: f64) -> bool {
        let old = self.relative_value;
        self.relative_value = (external - self.min) / (self.max - self.min);
        old != self.relative_value
    }
    
    pub fn update_text_input(&mut self) {
        let e = self.to_external();
        self.text_input.text = match self.precision{
            0=>format!("{:.0}",e),
            1=>format!("{:.1}",e),
            2=>format!("{:.2}",e),
            3=>format!("{:.3}",e),
            4=>format!("{:.4}",e),
            5=>format!("{:.5}",e),
            6=>format!("{:.6}",e),
            7=>format!("{:.7}",e),
            _=>format!("{}",e)
        };
        self.text_input.select_all();
    }
    
    pub fn update_text_input_and_redraw(&mut self, cx: &mut Cx) {
        self.update_text_input();
        self.text_input.redraw(cx);
    }
    
    pub fn draw_walk_slider(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_slider.slide_pos = self.relative_value as f32;
        self.draw_slider.begin(cx, walk, self.layout);
        
        if let Some(mut dw) = cx.defer_walk(self.label_walk) {
            //, (self.value*100.0) as usize);
            let walk = self.text_input.walk(cx);
            let mut scope = Scope::default();
            let _ = self.text_input.draw_walk(cx, &mut scope, walk);

            let label_walk = dw.resolve(cx);
            cx.begin_turtle(label_walk, Layout::default());
            self.draw_text.draw_walk(cx, label_walk, self.label_align, &self.text);
            cx.end_turtle_with_area(&mut self.label_area);
        }
        
        self.draw_slider.end(cx);
    }

    pub fn value(&self) -> f64 {
        self.to_external()
    }

    pub fn set_value(&mut self, v: f64) {
        let prev_value = self.value();
        self.set_internal(v);
        if v != prev_value {
            self.update_text_input();
        }
    }
}

impl WidgetDesign for Slider{
    
}

impl Widget for Slider {

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        
        // alright lets match our designer against the slider backgdrop
        match event.hit_designer(cx, self.draw_slider.area()){
            HitDesigner::DesignerPick(_e)=>{
                cx.widget_action(uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _=>()
        }
        
        for action in cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope)) {
            match action.as_widget_action().cast() {
                TextInputAction::KeyFocus => {
                    self.animator_play(cx, id!(focus.on));
                }
                TextInputAction::KeyFocusLost => {
                    self.animator_play(cx, id!(focus.off));
                }
                TextInputAction::Return(value) => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.set_internal(v.max(self.min).min(self.max));
                    }
                    self.update_text_input_and_redraw(cx);
                    cx.widget_action(uid, &scope.path, SliderAction::TextSlide(self.to_external()));
                }
                TextInputAction::Escape => {
                    self.update_text_input_and_redraw(cx);
                }
                _ => ()
            }
        };

        if self.hover_actions_enabled {
            match event.hits_with_capture_overload(cx, self.label_area, true) {
                Hit::FingerHoverIn(fh) => {
                    cx.widget_action(uid, &scope.path, SliderAction::LabelHoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, &scope.path, SliderAction::LabelHoverOut);
                },
                _ => ()
            }
        }

        match event.hits(cx, self.draw_slider.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                // cx.set_key_focus(self.slider.area());
                self.text_input.is_read_only = true;
                self.text_input.set_key_focus(cx);
                self.text_input.select_all();
                self.text_input.redraw(cx);
                                
                self.animator_play(cx, id!(drag.on));
                self.dragging = Some(self.relative_value);
                cx.widget_action(uid, &scope.path, SliderAction::StartSlide);
            },
            Hit::FingerUp(fe) => {
                self.text_input.is_read_only = false;
                // if the finger hasn't moved further than X we jump to edit-all on the text thing
                self.text_input.force_new_edit_group();
                self.animator_play(cx, id!(drag.off));
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
                self.dragging = None;
                cx.widget_action(uid, &scope.path, SliderAction::EndSlide);
            }
            Hit::FingerMove(fe) => {
                let rel = fe.abs - fe.abs_start;
                if let Some(start_pos) = self.dragging {
                    println!("The value of my_variable is: {}", self.draw_slider.offset_left);
                    self.relative_value = (start_pos + rel.x / (fe.rect.size.x - self.draw_slider.offset_left as f64)).max(0.0).min(1.0);
                    self.set_internal(self.to_external());
                    self.draw_slider.redraw(cx);
                    self.update_text_input_and_redraw(cx);
                    cx.widget_action(uid, &scope.path, SliderAction::Slide(self.to_external()));
                }
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk_slider(cx, walk);
        DrawStep::done()
    }
    
    fn widget_to_data(&self, _cx: &mut Cx, actions:&Actions, nodes: &mut LiveNodeVec, path: &[LiveId])->bool{
        match actions.find_widget_action_cast(self.widget_uid()) {
            SliderAction::TextSlide(v) | SliderAction::Slide(v) => {
                nodes.write_field_value(path, LiveValue::Float64(v as f64));
                true
            }
            _ => false
        }
    }
    
    fn data_to_widget(&mut self, cx: &mut Cx, nodes:&[LiveNode], path: &[LiveId]){
        if let Some(value) = nodes.read_field_value(path) {
            if let Some(value) = value.as_float() {
                if self.set_internal(value) {
                    self.redraw(cx)
                }
                self.update_text_input_and_redraw(cx);
            }
        }
    }
    
    fn text(&self) -> String {
        format!("{}", self.to_external())
    }
        
    fn set_text(&mut self, v: &str) {
        if let Ok(v) = v.parse::<f64>(){
            self.set_internal(v);
            self.update_text_input()
        }
    }
        
}

impl SliderRef{
    pub fn value(&self)->Option<f64> {
        if let Some(inner) = self.borrow(){
            return Some(inner.value())
        }

        return None
    }

    pub fn set_value(&self, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(v)
        }
    }
    
    pub fn slided(&self, actions:&Actions)->Option<f64>{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::TextSlide(v) | SliderAction::Slide(v) => {
                    return Some(v)
                }
                _=>()
            }
        }
        None
    }

    pub fn label_hover_in(&self, actions:&Actions)->Option<Rect>{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::LabelHoverIn(rect) => Some(rect),
                _=> None
            }
        } else {
            None
        }
    }

    pub fn label_hover_out(&self, actions:&Actions)->bool{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::LabelHoverOut => true,
                _=> false
            }
        } else {
            false
        }
    }
}