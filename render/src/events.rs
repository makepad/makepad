use {
    makepad_live_compiler::*,
    makepad_platform::{
        events::*,
        area::Area,
    },
    crate::{
        cx::Cx,
        area::AreaImpl,
        turtle::Margin,
    },
};

#[derive(Clone, Debug, Default)]
pub struct HitOpt {
    pub use_multi_touch: bool,
    pub margin: Option<Margin>,
}

pub trait EventImpl{
    fn is_next_frame(&self, cx: &mut Cx, next_frame: NextFrame) -> Option<NextFrameEvent>;
    fn hits(&mut self, cx: &mut Cx, area: Area, opt: HitOpt) -> Event;
    fn drag_hits(&mut self, cx: &mut Cx, area: Area, opt: HitOpt) -> Event;
}

pub fn rect_contains_with_margin(rect:&Rect, pos: Vec2, margin: &Option<Margin>) -> bool {
    if let Some(margin) = margin {
        return
        pos.x >= rect.pos.x - margin.l
            && pos.x <= rect.pos.x + rect.size.x + margin.r
            && pos.y >= rect.pos.y - margin.t
            && pos.y <= rect.pos.y + rect.size.y + margin.b;
    }
    else {
        return rect.contains(pos);
    }
}

impl EventImpl for Event {
    
    fn is_next_frame(&self, cx: &mut Cx, next_frame: NextFrame) -> Option<NextFrameEvent> {
        match self {
            Event::NextFrame(fe) => {
                if cx._next_frames.contains(&next_frame) {
                    return Some(fe.clone())
                }
            }
            _ => ()
        }
        None
    }
    /*
    pub fn is_animate(&self, cx:&mut Cx, animator: &Animator)->Option<AnimateEvent>{
         match self {
            Event::Animate(ae) => {
                if cx.playing_animator_ids.get(&animator.animator_id).is_some(){
                    return Some(ae.clone())
                }
            }
            _=>()
        }
        None
    }*/
    
    
    
    fn hits(&mut self, cx: &mut Cx, area: Area, opt: HitOpt) -> Event {
        match self {
            Event::KeyFocus(kf) => {
                if area == kf.prev {
                    return Event::KeyFocusLost(kf.clone())
                }
                else if area == kf.focus {
                    return Event::KeyFocus(kf.clone())
                }
            },
            Event::KeyDown(_) => {
                if area == cx.key_focus {
                    return self.clone();
                }
            },
            Event::KeyUp(_) => {
                if area == cx.key_focus {
                    return self.clone();
                }
            },
            Event::TextInput(_) => {
                if area == cx.key_focus {
                    return self.clone();
                }
            },
            Event::TextCopy(_) => {
                if area == cx.key_focus {
                    return Event::TextCopy(
                        TextCopyEvent {response: None}
                    );
                }
            },
            Event::FingerScroll(fe) => {
                let rect = area.get_rect(&cx);
                if rect_contains_with_margin(&rect, fe.abs, &opt.margin) {
                    //fe.handled = true;
                    return Event::FingerScroll(FingerScrollEvent {
                        rel: fe.abs - rect.pos,
                        rect: rect,
                        ..fe.clone()
                    })
                }
            },
            Event::FingerHover(fe) => {
                let rect = area.get_rect(&cx);
                
                if cx.fingers[fe.digit]._over_last == area {
                    let mut any_down = false;
                    for finger in &cx.fingers {
                        if finger.captured == area {
                            any_down = true;
                            break;
                        }
                    }
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &opt.margin) {
                        fe.handled = true;
                        if let HoverState::Out = fe.hover_state {
                            //    cx.finger_over_last_area = Area::Empty;
                        }
                        else {
                            cx.fingers[fe.digit].over_last = area;
                        }
                        return Event::FingerHover(FingerHoverEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            ..fe.clone()
                        })
                    }
                    else {
                        //self.was_over_last_call = false;
                        return Event::FingerHover(FingerHoverEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            hover_state: HoverState::Out,
                            ..fe.clone()
                        })
                    }
                }
                else {
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &opt.margin) {
                        let mut any_down = false;
                        for finger in &cx.fingers {
                            if finger.captured == area {
                                any_down = true;
                                break;
                            }
                        }
                        cx.fingers[fe.digit].over_last = area;
                        fe.handled = true;
                        //self.was_over_last_call = true;
                        return Event::FingerHover(FingerHoverEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            hover_state: HoverState::In,
                            ..fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe) => {
                // check wether our digit is captured, otherwise don't send
                if cx.fingers[fe.digit].captured == area {
                    let abs_start = cx.fingers[fe.digit].down_abs_start;
                    let rel_start = cx.fingers[fe.digit].down_rel_start;
                    let rect = area.get_rect(&cx);
                    return Event::FingerMove(FingerMoveEvent {
                        abs_start: abs_start,
                        rel: area.abs_to_rel(cx, fe.abs),
                        rel_start: rel_start,
                        rect: rect,
                        is_over: rect_contains_with_margin(&rect, fe.abs, &opt.margin),
                        ..fe.clone()
                    })
                }
            },
            Event::FingerDown(fe) => {
                if !fe.handled {
                    let rect = area.get_rect(&cx);
                    if rect_contains_with_margin(&rect, fe.abs, &opt.margin) {
                        // scan if any of the fingers already captured this area
                        if !opt.use_multi_touch {
                            for finger in &cx.fingers {
                                if finger.captured == area {
                                    return Event::None;
                                }
                            }
                        }
                        cx.fingers[fe.digit].captured = area;
                        let rel = area.abs_to_rel(cx, fe.abs);
                        cx.fingers[fe.digit].down_abs_start = fe.abs;
                        cx.fingers[fe.digit].down_rel_start = rel;
                        fe.handled = true;
                        return Event::FingerDown(FingerDownEvent {
                            rel: rel,
                            rect: rect,
                            ..fe.clone()
                        })
                    }
                }
            },
            Event::FingerUp(fe) => {
                if cx.fingers[fe.digit].captured == area {
                    cx.fingers[fe.digit].captured = Area::Empty;
                    let abs_start = cx.fingers[fe.digit].down_abs_start;
                    let rel_start = cx.fingers[fe.digit].down_rel_start;
                    let rect = area.get_rect(&cx);
                    return Event::FingerUp(FingerUpEvent {
                        is_over: rect.contains(fe.abs),
                        abs_start: abs_start,
                        rel_start: rel_start,
                        rel: area.abs_to_rel(cx, fe.abs),
                        rect: rect,
                        ..fe.clone()
                    })
                }
            },
            _ => ()
        };
        return Event::None;
    }
    
    fn drag_hits(&mut self, cx: &mut Cx, area: Area, opt: HitOpt) -> Event {
        match self {
            Event::FingerDrag(event) => {
                let rect = area.get_rect(cx);
                if area == cx.drag_area {
                    if !event.handled && rect_contains_with_margin(&rect, event.abs, &opt.margin) {
                        cx.new_drag_area = area;
                        event.handled = true;
                        Event::FingerDrag(FingerDragEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            ..event.clone()
                        })
                    } else {
                        Event::FingerDrag(FingerDragEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            state: DragState::Out,
                            ..event.clone()
                        })
                    }
                } else {
                    if !event.handled && rect_contains_with_margin(&rect, event.abs, &opt.margin) {
                        cx.new_drag_area = area;
                        event.handled = true;
                        Event::FingerDrag(FingerDragEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            state: DragState::In,
                            ..event.clone()
                        })
                    } else {
                        Event::None
                    }
                }
            }
            Event::FingerDrop(event) => {
                let rect = area.get_rect(cx);
                if !event.handled && rect_contains_with_margin(&rect, event.abs, &opt.margin) {
                    cx.new_drag_area = Area::default();
                    event.handled = true;
                    Event::FingerDrop(FingerDropEvent {
                        rel: area.abs_to_rel(cx, event.abs),
                        rect,
                        ..event.clone()
                    })
                } else {
                    Event::None
                }
            }
            _ => Event::None,
        }
    }
    
}

