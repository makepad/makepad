use std::mem;

use render::*;
use crate::splitter::*;
use crate::tabcontrol::*;

#[derive(Clone)]
pub struct Dock<TItem>
where TItem: Clone
{
    pub dock_items: Option<DockItem<TItem>>,
    pub splitters: Elements<Splitter, usize>,
    pub tab_controls: Elements<TabControl, usize>,

    pub drop_size:Vec2,
    pub drop_quad: Quad,
    pub drop_quad_view:View<NoScrollBar>,
    pub drop_quad_color:Vec4,
    pub _drag_move: Option<FingerMoveEvent>,
    pub _drag_end: Option<DockDragEnd>,
    pub _tweening_quad: Option<(usize,Rect,f32)>
}

impl<TItem> ElementLife for Dock<TItem>
where TItem: Clone
{
    fn construct(&mut self, cx: &mut Cx){
        self.handle_dock(cx, &mut Event::Construct);
    }

    fn destruct(&mut self, cx: &mut Cx){
        self.handle_dock(cx, &mut Event::Destruct);
    }
}

impl<TItem> Style for Dock<TItem>
where TItem: Clone
{
    fn style(cx: &mut Cx)->Dock<TItem>{
        Dock{
            dock_items:None,
            drop_size:vec2(70.,70.),
            drop_quad_color:color("#a"),
            drop_quad:Quad{
                ..Style::style(cx)
            },
            splitters:Elements::new(Splitter{
                ..Style::style(cx)
            }),
            tab_controls:Elements::new(TabControl{
                ..Style::style(cx)
            }),
            drop_quad_view:View{
                is_overlay:true,
                ..Style::style(cx)
            },
            _drag_move:None,
            _drag_end:None,
            _tweening_quad:None
        }
    }
}

#[derive(Clone)]
pub struct DockDragEnd{
    finger_up_event:FingerUpEvent,
    tab_control_id:usize,
    tab_id:usize
}

#[derive(Clone)]
pub struct DockTab<TItem>
where TItem: Clone
{
    pub title:String,
    pub item:TItem
}

#[derive(Clone)]
pub enum DockItem<TItem>
where TItem: Clone
{
    Single(TItem),
    TabControl{
        current:usize,
        tabs:Vec<DockTab<TItem>>,
    },
    Splitter{
        align:SplitterAlign,
        pos:f32,
        axis:Axis,
        first:Box<DockItem<TItem>>, 
        last:Box<DockItem<TItem>>
    }
}

struct DockWalkStack<'a, TItem>
where TItem: Clone
{
    counter:usize,
    uid:usize,
    item:&'a mut DockItem<TItem>
}

pub enum DockEvent{
    None,
    DockChanged
}

pub struct DockWalker<'a, TItem>
where TItem: Clone
{
    walk_uid:usize,
    stack:Vec<DockWalkStack<'a, TItem>>,
    // forwards for Dock
    splitters:&'a mut Elements<Splitter, usize>,
    tab_controls:&'a mut Elements<TabControl, usize>,
    drop_quad_view:&'a mut View<NoScrollBar>,
    _drag_move:&'a mut Option<FingerMoveEvent>,
    _drag_end:&'a mut Option<DockDragEnd>
}

impl<'a, TItem> DockWalker<'a, TItem>
where TItem: Clone
{
    pub fn walk_handle_dock(&mut self, cx: &mut Cx, event: &mut Event)->Option<&mut TItem>{
        // lets get the current item on the stack
        let push_or_pop = if let Some(stack_top) = self.stack.last_mut(){
            // return item 'count'
            match stack_top.item{
                DockItem::Single(item)=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        return Some(unsafe{mem::transmute(item)});
                    }
                    else{
                        None
                    }
                },
                DockItem::TabControl{current, tabs}=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        let tab_control = self.tab_controls.get(stack_top.uid);
                        if !tab_control.is_none(){
                            match tab_control.unwrap().handle_tab_control(cx, event){
                                TabControlEvent::TabDragMove{fe, ..}=>{
                                *self._drag_move = Some(fe);
                                *self._drag_end = None;
                                self.drop_quad_view.redraw_view_area(cx);
                                },
                                TabControlEvent::TabDragEnd{fe,tab_id}=>{
                                    *self._drag_move = None;
                                    *self._drag_end = Some(DockDragEnd{
                                        finger_up_event:fe, 
                                        tab_control_id:stack_top.uid, 
                                        tab_id:tab_id
                                    });
                                    self.drop_quad_view.redraw_view_area(cx);
                                },
                                _=>()
                            }
                        }
                        if *current < tabs.len(){
                            return Some(unsafe{mem::transmute(&mut tabs[*current].item)});
                        }
                        None
                    }
                    else{
                        None
                    }
                },
                DockItem::Splitter{first, last, pos, ..}=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        let split = self.splitters.get(stack_top.uid);
                        if !split.is_none(){
                            match split.unwrap().handle_splitter(cx, event){
                                SplitterEvent::Moving{new_pos}=>{
                                    *pos = new_pos;
                                },
                                _=>()
                            };
                        }
                        // update state in our splitter level
                        Some(DockWalkStack{counter:0, uid:0, item:unsafe{mem::transmute(first.as_mut())}})
                    }
                    else if stack_top.counter == 1{
                        stack_top.counter +=1;
                        Some(DockWalkStack{counter:0, uid:0, item:unsafe{mem::transmute(last.as_mut())}})
                    }
                    else{
                        None
                    }
                }
            }
        }
        else{
            return None;
        };
        if let Some(item) = push_or_pop{
            self.stack.push(item);
            return self.walk_handle_dock(cx, event);
        }
        else if self.stack.len() > 0{
            self.stack.pop();
            return self.walk_handle_dock(cx, event);
        }
        return None;
    }

    pub fn walk_draw_dock(&mut self, cx: &mut Cx)->Option<&'a mut TItem>{
        // lets get the current item on the stack
         let push_or_pop = if let Some(stack_top) = self.stack.last_mut(){
           
            // return item 'count'
            match stack_top.item{
                DockItem::Single(item)=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        return Some(unsafe{mem::transmute(item)});
                    }
                    else{
                        None
                    }
                },
                DockItem::TabControl{current, tabs}=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        let tab_control = self.tab_controls.get_draw(cx, stack_top.uid);
                        tab_control.begin_tabs(cx);
                        for tab in tabs.iter(){
                            tab_control.draw_tab(cx, &tab.title, false);
                        }
                        tab_control.end_tabs(cx);
                        tab_control.begin_tab_page(cx);
                        if *current < tabs.len(){
                            return Some(unsafe{mem::transmute(&mut tabs[*current].item)});
                        }
                        tab_control.end_tab_page(cx);
                        None
                    }
                    else{
                        let tab_control = self.tab_controls.get_draw(cx, stack_top.uid);
                        tab_control.end_tab_page(cx);
                        None
                    }
                },
                DockItem::Splitter{align, pos, axis, first, last}=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        // begin a split
                        let split = self.splitters.get_draw(cx, stack_top.uid);
                        split.set_splitter_state(align.clone(), *pos, axis.clone());
                        split.begin_splitter(cx);
                        Some(DockWalkStack{counter:0, uid:0, item:unsafe{mem::transmute(first.as_mut())}})
                    }
                    else if stack_top.counter == 1{
                        stack_top.counter +=1 ;

                        let split = self.splitters.get_draw(cx, stack_top.uid);
                        split.mid_splitter(cx);
                        Some(DockWalkStack{counter:0, uid:0, item:unsafe{mem::transmute(last.as_mut())}})
                    }
                    else{
                        let split = self.splitters.get_draw(cx, stack_top.uid);
                        split.end_splitter(cx);
                        None
                    }
                }
            }
        }
        else{
            return None
        };
        if let Some(item) = push_or_pop{
            self.stack.push(item);
            return self.walk_draw_dock(cx);
        }
        else if self.stack.len() > 0{
            self.stack.pop();
            return self.walk_draw_dock(cx);
        }
        None
    }
}

enum DockDropKind{
    Left,
    Top,
    Right,
    Bottom,
    Center
}

impl<TItem> Dock<TItem>
where TItem: Clone
{
    fn get_drop_kind(pos:Vec2, drop_size:Vec2, rect:Rect)->(DockDropKind, Rect){
        // this is how the drop areas look
        //    |-------------------------------|
        //    |      |     Top        |       |
        //    |      |----------------|       |
        //    |      |                |       |
        //    |      |                |       |
        //    | Left |    Center      | Right |
        //    |      |                |       |
        //    |      |                |       |
        //    |      |----------------|       |
        //    |      |    Bottom      |       |
        //    ---------------------------------
        if pos.x < rect.x + drop_size.x{
            return (DockDropKind::Left, Rect{x:rect.x, y:rect.y, w:0.5 * rect.w, h:rect.h})
        }
        if pos.x > rect.x + rect.w - drop_size.x{
            return (DockDropKind::Right, Rect{x:rect.x + 0.5 * rect.w, y:rect.y, w:0.5 * rect.w, h:rect.h})
        }
        if pos.y < rect.y + drop_size.y{
            return (DockDropKind::Top, Rect{x:rect.x, y:rect.y, w:rect.w, h:0.5*rect.h})
        }
        if pos.y > rect.y + rect.h - drop_size.y{
            return (DockDropKind::Bottom, Rect{x:rect.x, y:rect.y + 0.5 * rect.h, w:rect.w, h:0.5*rect.h})
        }
        (DockDropKind::Center, rect.clone())
    }

    fn recur_remove_tab(dock_walk:&mut DockItem<TItem>, control_id:usize, tab_id:usize, counter:&mut usize)->Option<DockTab<TItem>>
    where TItem: Clone
    {
        match dock_walk{
            DockItem::Single(_)=>{},
            DockItem::TabControl{tabs,..}=>{
                let id = *counter;
                *counter += 1;
                if id == control_id{
                    return Some(tabs.remove(tab_id));
                }
            },
            DockItem::Splitter{first,last,..}=>{
                *counter += 1;
                let left = Self::recur_remove_tab(first, control_id, tab_id, counter);
                if !left.is_none(){
                    return left
                }
                let right = Self::recur_remove_tab(last, control_id, tab_id, counter);
                if !right.is_none(){
                    return right
                }
            }
        }
        None
    }   

   fn recur_collapse_empty(dock_walk:&mut DockItem<TItem>)->bool
   where TItem: Clone
   {
        match dock_walk{
            DockItem::Single(_)=>{},
            DockItem::TabControl{tabs,..}=>{
                return tabs.len() == 0
            },
            DockItem::Splitter{first,last,..}=>{
                let rem_first = Self::recur_collapse_empty(first);
                let rem_last = Self::recur_collapse_empty(last);
                if rem_first && rem_last{
                    return true;
                }
                if rem_first{
                    *dock_walk = *last.clone();
                }
                else if rem_last{
                    *dock_walk = *first.clone();
                }
            }
        }
        false
    }   

    fn recur_split_dock(dock_walk:&mut DockItem<TItem>, item:&DockTab<TItem>, control_id:usize, kind:&DockDropKind, counter:&mut usize)
    where TItem: Clone
    {
        match dock_walk{
            DockItem::Single(_)=>{},
            DockItem::TabControl{tabs,..}=>{
                let id = *counter;
                *counter += 1;
                if id == control_id{
                    match kind{
                        DockDropKind::Left=>{
                            *dock_walk = DockItem::Splitter{
                                align:SplitterAlign::Weighted, pos:0.5,
                                axis:Axis::Vertical,
                                last:Box::new(dock_walk.clone()),
                                first:Box::new(DockItem::TabControl{current:0,tabs:vec![item.clone()]})
                            };
                        },
                        DockDropKind::Right=>{
                            *dock_walk = DockItem::Splitter{
                                align:SplitterAlign::Weighted, pos:0.5,
                                axis:Axis::Vertical,
                                first:Box::new(dock_walk.clone()),
                                last:Box::new(DockItem::TabControl{current:0,tabs:vec![item.clone()]})
                            };
                        },                        
                        DockDropKind::Top=>{
                           *dock_walk = DockItem::Splitter{
                                align:SplitterAlign::Weighted, pos:0.5,
                                axis:Axis::Horizontal,
                                last:Box::new(dock_walk.clone()),
                                first:Box::new(DockItem::TabControl{current:0,tabs:vec![item.clone()]})
                            };
                        },
                        DockDropKind::Bottom=>{
                           *dock_walk = DockItem::Splitter{
                                align:SplitterAlign::Weighted, pos:0.5,
                                axis:Axis::Horizontal,
                                first:Box::new(dock_walk.clone()),
                                last:Box::new(DockItem::TabControl{current:0,tabs:vec![item.clone()]})
                            };                            
                        },
                        DockDropKind::Center=>{
                            tabs.push(item.clone());
                        }
                    }
                }
            },
            DockItem::Splitter{first,last,..}=>{
                *counter += 1;
                Self::recur_split_dock(first, item, control_id, kind, counter);
                Self::recur_split_dock(last, item, control_id, kind, counter);
            }
        }
    }
/*
   fn recur_debug_dock(dock_walk:&mut DockItem<TItem>, counter:&mut usize, depth:usize)
    where TItem: Clone
    {
        let mut indent = String::new();
        for i in 0..depth{indent.push_str("  ")}
        match dock_walk{
            DockItem::Single(item)=>{},
            DockItem::TabControl{tabs,..}=>{
                let id = *counter;
                *counter += 1;
                println!("{}TabControl {}", indent, id);
                for (id,tab) in tabs.iter().enumerate(){
                    println!("{}  Tab{} {}", indent, id, tab.title);
                }
            },
            DockItem::Splitter{first,last,..}=>{
                let id = *counter;
                *counter += 1;
                println!("{}Splitter {}", indent, id);
                Self::recur_debug_dock(first, counter, depth + 1);
                Self::recur_debug_dock(last,  counter, depth + 1);
            }
        }
    }*/

    pub fn handle_dock(&mut self, cx: &mut Cx, _event:&mut Event)->DockEvent{
        if let Some(drag_end) = self._drag_end.clone(){
            self._drag_end = None;
            let fe = drag_end.finger_up_event;
            for (target_id,tab_control) in self.tab_controls.enumerate(){
                // ok now, we ask the tab_controls rect
                let cdr = tab_control.get_content_drop_rect(cx);
                // alright we need a drop area
                if cdr.contains(fe.abs.x, fe.abs.y){
                    let (kind, _rect) = Self::get_drop_kind(fe.abs, self.drop_size, cdr);
                    // we have a kind!
                    let item = Self::recur_remove_tab(self.dock_items.as_mut().unwrap(), drag_end.tab_control_id, drag_end.tab_id, &mut 0);
                    // alright we have a kind. 
                    if !item.is_none(){
                        Self::recur_split_dock(
                            self.dock_items.as_mut().unwrap(), 
                            item.as_ref().unwrap(),
                            *target_id,
                            &kind,
                            &mut 0
                        );
                    };
                }
            }
            Self::recur_collapse_empty(self.dock_items.as_mut().unwrap());
            cx.redraw_area(Area::All);
            //Self::recur_debug_dock(self.dock_items.as_mut().unwrap(), &mut 0, 0);
            return DockEvent::DockChanged
        };
        // ok we need to pull out the TItem from our dockpanel
        DockEvent::None
    }

    pub fn draw_dock(&mut self, cx: &mut Cx){
        // lets draw our hover layer if need be
        if let Some(fe) = &self._drag_move{
            self.drop_quad_view.begin_view(cx, &Layout{
                abs_start:Some(vec2(0.,0.)),
                ..Default::default()
            });

            // alright so now, what do i need to do
            // well lets for shits n giggles find all the tab areas 
            // you know, we have a list eh
            for (id,tab_control) in self.tab_controls.enumerate(){
                // ok now, we ask the tab_controls rect
                let cdr = tab_control.get_content_drop_rect(cx);
                // alright we need a drop area

                if cdr.contains(fe.abs.x, fe.abs.y){
                    let (_kind, rect) = Self::get_drop_kind(fe.abs, self.drop_size, cdr);

                    if !self._tweening_quad.is_none() && self._tweening_quad.unwrap().0 != *id{
                        // restarts the animation by removing drop_quad
                        self._tweening_quad = None;
                    }

                    // alright so we have to animate our draw rect towards it
                    let (dr, alpha) = if self._tweening_quad.is_none(){
                        self._tweening_quad = Some((*id,rect,0.25));
                        (rect,0.25)
                    }
                    else{
                        let (id, old_rc, old_alpha) = self._tweening_quad.unwrap();
                        let move_speed = 0.7;
                        let alpha_speed = 0.9;
                        let alpha = old_alpha * alpha_speed + (1.-alpha_speed);
                        let rc = Rect{
                            x:old_rc.x*move_speed + rect.x * (1.-move_speed),
                            y:old_rc.y*move_speed + rect.y * (1.-move_speed),
                            w:old_rc.w*move_speed + rect.w * (1.-move_speed),
                            h:old_rc.h*move_speed+ rect.h* (1.-move_speed)
                        };
                        let dist = (rc.x-rect.x).abs().max((rc.y-rect.y).abs()).max((rc.w-rect.w).abs()).max((rc.h-rect.h).abs()).max(100.-alpha*100.);
                        if dist>0.5{ // keep redrawing until we are close
                            self.drop_quad_view.redraw_view_area(cx);
                        }
                        self._tweening_quad = Some((id,rc,alpha));
                        (rc, alpha)
                    };
                    self.drop_quad.color = self.drop_quad_color;
                    self.drop_quad_color.w = alpha*0.8;
                    self.drop_quad.draw_quad(cx, dr.x, dr.y, dr.w, dr.h);
                }
            }
            //self.drop_quad.draw_quad()

            self.drop_quad_view.end_view(cx);
        }
    }

    pub fn walker<'a>(&'a mut self)->DockWalker<'a, TItem>{
        let mut stack = Vec::new();
        if !self.dock_items.is_none(){
            stack.push(DockWalkStack{counter:0, uid:0, item:self.dock_items.as_mut().unwrap()});
        }
        DockWalker{
            walk_uid:0,
            stack:stack,
            splitters:&mut self.splitters,
            tab_controls:&mut self.tab_controls,
            _drag_move:&mut self._drag_move,
            _drag_end:&mut self._drag_end,
            drop_quad_view:&mut self.drop_quad_view,
        }
    }
}
