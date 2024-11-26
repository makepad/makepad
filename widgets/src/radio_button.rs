use crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        View,
        Image,
    };

live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    use crate::view_ui::CachedRoundedView;
     
    DrawRadioButton = {{DrawRadioButton}} {}
    pub RadioButtonBase = {{RadioButton}} {}
    pub RadioButtonGroupBase = {{RadioButtonGroup }} {}
    
    pub RadioButton = <RadioButtonBase> {
        // TODO: adda  focus states 
        width: Fit, height: 16.,
        align: { x: 0.0, y: 0.5 }
        
        icon_walk: { margin: { left: 20. } }
        
        label_walk: {
            width: Fit, height: Fit,
            margin: { left: 20. }
        }
        label_align: { y: 0.0 }
        
        draw_radio: {
            uniform size: 7.0;
            // uniform color_active: (THEME_COLOR_U_2)
            // uniform color_inactive: (THEME_COLOR_D_4)
            
            // instance pressed: 0.0
            uniform border_radius: (THEME_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_CTRL_DEFAULT)
            instance bodybottom: (THEME_COLOR_CTRL_ACTIVE)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.radio_type {
                    RadioType::Round => {
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.circle(left, c.y, sz);
                        sdf.fill_keep(mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM, pow(self.pos.y, 1.)))
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pos.y), (THEME_BEVELING))
                        let isz = sz * 0.5;
                        sdf.circle(left, c.y, isz);
                        sdf.fill(
                            mix(
                                mix(
                                    THEME_COLOR_U_HIDDEN,
                                    THEME_COLOR_CTRL_HOVER,
                                    self.hover
                                ),
                                THEME_COLOR_TEXT_ACTIVE,
                                self.selected
                            )
                        );
                        
                    }
                    RadioType::Tab => {
                        let grad_top = 5.0;
                        let grad_bot = 1.0;
                        let body = mix(
                            mix(self.bodytop, (THEME_COLOR_CTRL_HOVER), self.hover),
                            self.bodybottom,
                            self.selected
                        );
                        let body_transp = vec4(body.xyz, 0.0);
                        let top_gradient = mix(body_transp, mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_SHADOW, self.selected), max(0.0, grad_top - sdf.pos.y) / grad_top);
                        let bot_gradient = mix(
                            mix(body_transp, THEME_COLOR_BEVEL_LIGHT, self.selected),
                            top_gradient,
                            clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                        );
                        
                        // the little drop shadow at the bottom
                        let shift_inward = 0. * 1.75;
                        sdf.move_to(shift_inward, self.rect_size.y);
                        sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y);
                        sdf.stroke(
                            mix(THEME_COLOR_BEVEL_SHADOW,
                                THEME_COLOR_U_HIDDEN,
                                self.selected
                            ), THEME_BEVELING * 2.
                        );
                            
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x - 2.0,
                            self.rect_size.y - 2.0,
                            1.
                        )
                        sdf.fill_keep(body)
                            
                        sdf.stroke(bot_gradient, THEME_BEVELING * 1.5)
                    }
                }
                return sdf.result
            }
        }
            
        draw_text: {
            instance hover: 0.0
            instance selected: 0.0
                
            uniform color_unselected: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_unselected_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_selected: (THEME_COLOR_TEXT_SELECTED)
                
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color_unselected,
                        self.color_unselected,
                        // self.color_unselected_hover,
                        self.hover
                    ),
                    self.color_unselected,
                    // self.color_selected,
                    self.selected
                )
            }
        }
            
        draw_icon: {
            instance hover: 0.0
            instance selected: 0.0
            uniform color: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_active: (THEME_COLOR_TEXT_ACTIVE)
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        mix(self.color, #f, 0.4),
                        self.hover
                    ),
                    mix(
                        self.color_active,
                        mix(self.color_active, #f, 0.75),
                        self.hover
                    ),
                    self.selected
                )
            }
        }
            
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_radio: {hover: 0.0}
                        draw_text: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_radio: {hover: 1.0}
                        draw_text: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_radio: {selected: 0.0}
                        draw_icon: {selected: 0.0}
                        draw_text: {selected: 0.0}
                        draw_icon: {selected: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {selected: 1.0}
                        draw_icon: {selected: 1.0}
                        draw_text: {selected: 1.0}
                        draw_icon: {selected: 1.0}
                    }
                }
            }
        }
    }
        
    pub RadioButtonCustom = <RadioButton> {
        height: Fit,
        draw_radio: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }
        margin: { left: -17.5 }
        label_walk: {
            width: Fit, height: Fit,
            margin: { left: (THEME_SPACE_1) }
        }
    }
        
    pub RadioButtonTextual = <RadioButton> {
        height: Fit,
        draw_radio: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }
        label_walk: {
            margin: 0.,
            width: Fit, height: Fit,
        }
        draw_text: {
            instance hover: 0.0
            instance selected: 0.0
                
            uniform color_unselected: (THEME_COLOR_U_3)
            uniform color_unselected_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_selected: (THEME_COLOR_TEXT_SELECTED)
                
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color_unselected,
                        self.color_unselected_hover,
                        self.hover
                    ),
                    self.color_selected,
                    self.selected
                )
            }
        }
    }
        
    pub RadioButtonImage = <RadioButton> { }
        
    pub RadioButtonTab = <RadioButton> {
        height: Fit,
        draw_radio: { radio_type: Tab }
        padding: <THEME_MSPACE_2> { left: (THEME_SPACE_2 * -1.25)}
            
        draw_text: {
            instance hover: 0.0
            instance selected: 0.0
                
            uniform color_unselected: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_unselected_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_selected: (THEME_COLOR_TEXT_HOVER)
                
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color_unselected,
                        self.color_unselected,
                        // self.color_unselected_hover,
                        self.hover
                    ),
                    self.color_selected,
                    self.selected
                )
            }
        }
    }
    
    pub ButtonGroup = <CachedRoundedView> {
        height: Fit, width: Fit,
        spacing: 0.0,
        flow: Right
        align: { x: 0.0, y: 0.5 }
        draw_bg: {
            radius: 4.
        }
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawRadioButton {
    #[deref] draw_super: DrawQuad,
    #[live] radio_type: RadioType,
    #[live] hover: f32,
    #[live] focus: f32,
    #[live] selected: f32
}


#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum RadioType {
    #[pick] Round = shader_enum(1),
    Tab = shader_enum(2),
}

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum MediaType {
    Image,
    #[pick] Icon,
    None,
}

#[derive(Live, LiveHook, Widget)]
pub struct RadioButtonGroup {
    #[deref] frame: View
}

#[derive(Live, LiveHook, Widget)]
pub struct RadioButton {
    #[redraw] #[live] draw_radio: DrawRadioButton,
    #[live] draw_icon: DrawIcon,
    #[live] draw_text: DrawText,

    #[live] value: LiveValue,

    #[live] media: MediaType,
    
    #[live] icon_walk: Walk,
    #[walk] walk: Walk,

    #[live] image: Image,

    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    #[live] text: ArcStringMut,
    
    #[live] bind: String,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum RadioButtonAction {
    Clicked,
    None
}


impl RadioButtonGroup {
    pub fn draw_walk(&mut self, _cx: &mut Cx2d, _walk: Walk) {}
}

impl RadioButton {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_radio.begin(cx, walk, self.layout);
        match self.media {
            MediaType::Image => {
                let image_walk = self.image.walk(cx);
                let _ = self.image.draw_walk(cx, image_walk);
            }
            MediaType::Icon => {
                self.draw_icon.draw_walk(cx, self.icon_walk);
            }
            MediaType::None => {}
        }
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
        self.draw_radio.end(cx);
    }
        
}

impl Widget for RadioButtonGroup {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        //let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
              
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
    
}

impl Widget for RadioButton {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
                
        match event.hits(cx, self.draw_radio.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(selected.off)) {
                    self.animator_play(cx, id!(selected.on));
                    cx.widget_action(uid, &scope.path, RadioButtonAction::Clicked);
                }
            },
            Hit::FingerUp(_fe) => {
                                
            }
            Hit::FingerMove(_fe) => {
                                
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
    
    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }
            
    fn set_text(&mut self, v: &str) {
        self.text.as_mut_empty().push_str(v);
    }
}

impl RadioButtonRef{
    fn unselect(&self, cx:&mut Cx){
        if let Some(mut inner) = self.borrow_mut(){
            inner.animator_play(cx, id!(selected.off));
        }
    }

    pub fn select(&self, cx: &mut Cx, scope: &mut Scope){
        if let Some(mut inner) = self.borrow_mut(){
            if inner.animator_in_state(cx, id!(selected.off)) {
                inner.animator_play(cx, id!(selected.on));
                cx.widget_action(inner.widget_uid(), &scope.path, RadioButtonAction::Clicked);
            }
        }
    }
}

impl RadioButtonSet{
    
    pub fn selected(&self, cx: &mut Cx, actions: &Actions)->Option<usize>{
        for action in actions{
            if let Some(action) = action.as_widget_action(){
                match action.cast(){
                    RadioButtonAction::Clicked => if let Some(index) = self.0.iter().position(|v| action.widget_uid == v.widget_uid()){
                        for (i, item) in self.0.iter().enumerate(){
                            if i != index{
                                RadioButtonRef(item.clone()).unselect(cx);
                            }
                        }
                        return Some(index);
                    }
                    _ => ()
                }
            }
        }
        None
    }
    
    pub fn selected_to_visible(&self, cx: &mut Cx, ui:&WidgetRef, actions: &Actions, paths:&[&[LiveId]] ) {
        // find a widget action that is in our radiogroup
        if let Some(index) = self.selected(cx, actions){
            // ok now we set visible
            for (i,path) in paths.iter().enumerate(){
                let widget = ui.widget(path);
                widget.apply_over(cx, live!{visible:(i == index)});
                widget.redraw(cx);
            }
        }
    }
}
