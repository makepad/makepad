// a window menu implementation
use render::*;

#[derive(Clone)]
pub struct WindowMenu {
    pub view: View,
    pub item_draw: MenuItemDraw,
}

#[derive(Clone)]
pub struct MenuItemDraw {
    pub text: Text,
    pub item_bg: Quad,
    pub row_height: f32,
    pub name_color: Color,
    pub bg_color: Color,
    pub bg_over_color: Color,
    pub bg_selected_color: Color,
}

impl MenuItemDraw {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            text: Text {
                wrapping: Wrapping::Word,
                ..Text::style(cx)
            },
            item_bg: Quad::style(cx),
            row_height: 20.0,
            name_color: color("white"),
            bg_color: cx.color("bg_selected"),
            bg_over_color: cx.color("bg_odd"),
            bg_selected_color: cx.color("bg_selected_over"),
        }
    }
    
    pub fn get_default_anim(&self, cx: &Cx) -> Anim {
        Anim::new(Play::Chain {duration: 0.01}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![
                (1.0, self.bg_color)
            ])
        ])
    }
    
    pub fn get_default_anim_cut(&self, cx: &Cx) -> Anim {
        Anim::new(Play::Cut {duration: 0.01}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![
                (0.0, self.bg_color)
            ])
        ])
    }
    
    pub fn get_over_anim(&self, cx: &Cx) -> Anim {
        Anim::new(Play::Cut {duration: 0.02}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![
                (0., self.bg_over_color),
            ])
        ])
    }
    
    pub fn get_line_layout(&self) -> Layout {
        Layout {
            width: Bounds::Fill,
            height: Bounds::Fix(self.row_height),
            padding: Padding {l: 2., t: 3., b: 2., r: 0.},
            line_wrap: LineWrap::None,
            ..Default::default()
        }
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
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            item_draw: MenuItemDraw::style(cx),
            view: View::style(cx),
        }
    }
    
    pub fn handle_window_menu(&mut self, _cx: &mut Cx, _event: &mut Event, _menu: &Menu) -> WindowMenuEvent {
        WindowMenuEvent::None
    }
    
    pub fn draw_window_menu(&mut self, _cx: &mut Cx, _menu: &Menu) {
    }
}
