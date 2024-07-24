use {
    crate::{
        DVec2,
        
    }
};

#[derive(Clone, Debug)]
pub struct DesignerPickEvent {
    pub abs: DVec2,
}
/*
impl Event{
    fn designer_hit()    
    Event::DesignerPick(e) => {
                       
        let rect = area.clipped_rect(&cx);
        if !hit_test(e.abs, &rect, &options.margin) {
            return Hit::Nothing
        }
        // lets add our area to a handled vec?
        // but how will we communicate the widget?
        return Hit::DesignerPick(e.clone())
    },
}*/