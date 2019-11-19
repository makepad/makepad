// a window menu implementation
use render::*;
use crate::widgettheme::*;

#[derive(Clone)]
pub struct WindowMenu {
    pub view: View,
    pub item_draw: MenuItemDraw,
}

#[derive(Clone)]
pub struct MenuItemDraw {
    pub text: Text,
    pub item_bg: Quad,
    //pub item_layout: LayoutId,
    //pub name_color: ColorId,
    //pub bg_color: ColorId,
    //pub bg_over_color: ColorId,
    //pub bg_selected_color: ColorId,
}

impl MenuItemDraw {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            text: Text {
                wrapping: Wrapping::Word,
                ..Text::proto(cx)
            },
            //item_layout: Layout_window_menu::id(cx),
            item_bg: Quad::proto(cx),
            //name_color: Color_text_selected_focus::id(cx),
            //bg_color: Color_bg_selected::id(cx),
            //bg_over_color: Color_bg_odd::id(cx),
            //bg_selected_color: Color_bg_selected_over::id(cx),
        }
    }
    
    pub fn get_default_anim(&self, cx: &Cx) -> Anim {
        Anim::new(Play::Chain {duration: 0.01}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (1.0,  Theme::color_bg_selected().base(cx))
            ])
        ])
    }
    
    pub fn get_default_anim_cut(&self, cx: &Cx) -> Anim {
        Anim::new(Play::Cut {duration: 0.01}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (0.0, Theme::color_bg_selected().base(cx))
            ])
        ])
    }
    
    pub fn get_over_anim(&self, cx: &Cx) -> Anim {
        Anim::new(Play::Cut {duration: 0.02}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (0., Theme::color_bg_odd().base(cx)),
            ])
        ])
    }
    
    pub fn text_style_menu_label() ->TextStyleId{uid!()}
    
    pub fn theme(cx:&mut Cx){ 
        Self::text_style_menu_label().set_base(cx, Theme::text_style_normal().base(cx));
    }   
    
    /*
    pub fn draw_log_path(&mut self, cx: &mut Cx, path: &str, row: usize) {
        self.text.color = self.path_color;
        self.text.draw_text(cx, &format!("{}:{} - ", path, row));
    }
    
    pub fn draw_log_body(&mut self, cx: &mut Cx, body: &str) {
        self.text.color = self.message_color;
        if body.len()>500 {
            self.text.draw_text(cx, &body[0..500]);
        }
        else {
            self.text.draw_text(cx, &body);
        }
    }
    
    pub fn draw_log_item(&mut self, cx: &mut Cx, list_item: &mut ListItem, log_item: &HubLogItem) {
        self.item_bg.color = list_item.animator.last_color(cx.id("bg.color"));
        let bg_inst = self.item_bg.begin_quad(cx, &self.get_line_layout());
        
        match log_item {
            HubLogItem::LocPanic(loc_msg) => {
                self.code_icon.draw_icon_walk(cx, CodeIconType::Panic);
                self.draw_log_path(cx, &loc_msg.path, loc_msg.row);
                self.draw_log_body(cx, &loc_msg.body);
                
            },
            HubLogItem::LocError(loc_msg) => {
                self.code_icon.draw_icon_walk(cx, CodeIconType::Error);
                self.draw_log_path(cx, &loc_msg.path, loc_msg.row);
                self.draw_log_body(cx, &loc_msg.body);
            },
            HubLogItem::LocWarning(loc_msg) => {
                self.code_icon.draw_icon_walk(cx, CodeIconType::Warning);
                self.draw_log_path(cx, &loc_msg.path, loc_msg.row);
                self.draw_log_body(cx, &loc_msg.body);
            },
            HubLogItem::LocMessage(loc_msg) => {
                self.draw_log_path(cx, &loc_msg.path, loc_msg.row);
                self.draw_log_body(cx, &loc_msg.body);
            },
            HubLogItem::Error(msg) => {
                self.code_icon.draw_icon_walk(cx, CodeIconType::Error);
                self.draw_log_body(cx, &msg);
            },
            HubLogItem::Warning(msg) => {
                self.code_icon.draw_icon_walk(cx, CodeIconType::Warning);
                self.draw_log_body(cx, &msg);
            },
            HubLogItem::Message(msg) => {
                self.draw_log_body(cx, &msg);
            }
        }
        
        let bg_area = self.item_bg.end_quad(cx, &bg_inst);
        list_item.animator.update_area_refs(cx, bg_area);
    }*/
}

#[derive(Clone)]
pub enum WindowMenuEvent {
    SelectItem {
    },
    None,
}

impl WindowMenu {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            item_draw: MenuItemDraw::proto(cx),
            view: View::proto(cx),
        }
    }
    
    pub fn handle_window_menu(&mut self, _cx: &mut Cx, _event: &mut Event, _menu: &Menu) -> WindowMenuEvent {
        WindowMenuEvent::None
    }
    
    pub fn draw_window_menu(&mut self, _cx: &mut Cx, _menu: &Menu) {
    }
}
