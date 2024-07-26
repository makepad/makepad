use {
    crate::{
        DVec2,
        event::Event,
        cx::Cx,
        area::Area,
        Margin,
    }
};

#[derive(Clone, Debug)]
pub struct DesignerPickEvent {
    pub abs: DVec2,
}

pub enum DesignerHit{
    DesignerPick(DesignerPickEvent),
    Nothing
}

impl Event{
    pub fn hit_designer(&mut self, cx: &mut Cx, area:Area)->DesignerHit{
        match self{
            Event::DesignerPick(e) => {
                let rect = area.clipped_rect(&cx);
                if Margin::rect_contains_with_margin(e.abs, &rect, &None){
                    return DesignerHit::DesignerPick(e.clone())
                }
            }
            _=>{}
        }
        DesignerHit::Nothing
    }
}