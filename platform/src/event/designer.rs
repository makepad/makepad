use crate::{area::Area, cx::Cx, event::Event, Inset, Vec2d};

#[derive(Clone, Debug)]
pub struct DesignerPickEvent {
    pub abs: Vec2d,
}

pub enum HitDesigner {
    DesignerPick(DesignerPickEvent),
    Nothing,
}

impl Event {
    pub fn hit_designer(&self, cx: &mut Cx, area: Area) -> HitDesigner {
        match self {
            Event::DesignerPick(e) => {
                let rect = area.clipped_rect(&cx);
                if Inset::rect_contains_with_inset(e.abs, &rect, &None) {
                    return HitDesigner::DesignerPick(e.clone());
                }
            }
            _ => {}
        }
        HitDesigner::Nothing
    }
}
