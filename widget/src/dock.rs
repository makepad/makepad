use std::mem;

use render::*;
use crate::splitter::*;
use crate::tabcontrol::*;
use crate::widgettheme::*;
use serde::*;

#[derive(Clone)]
pub struct Dock<TItem>
where TItem: Clone
{
    // pub dock_items: Option<DockItem<TItem>>,
    pub splitters: Elements<usize, Splitter, Splitter>,
    pub tab_controls: Elements<usize, TabControl, TabControl>,
    
    pub drop_size: Vec2,
    pub drop_quad: Quad,
    pub drop_quad_view: View,
    //pub drop_quad_color: ColorId,
    pub _drag_move: Option<FingerMoveEvent>,
    pub _drag_end: Option<DockDragEnd<TItem>>,
    pub _close_tab: Option<DockTabIdent>,
    pub _tab_select: Option<(usize, usize)>,
    pub _tweening_quad: Option<(usize, Rect, f32)>
}

#[derive(Clone, Debug)]
pub struct DockTabIdent {
    tab_control_id: usize,
    tab_id: usize
}

#[derive(Clone)]
pub enum DockDragEnd<TItem>
where TItem: Clone {
    OldTab {fe: FingerUpEvent, ident: DockTabIdent},
    NewItems {fe: FingerUpEvent, items: Vec<DockTab<TItem>>}
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DockTab<TItem>
where TItem: Clone
{
    pub closeable: bool,
    pub title: String,
    pub item: TItem
}

#[derive(Clone, Serialize, Deserialize)]
pub enum DockItem<TItem>
where TItem: Clone
{
    Single(TItem),
    TabControl {
        current: usize,
        previous: usize,
        tabs: Vec<DockTab<TItem>>,
    },
    Splitter {
        pos: f32,
        align: SplitterAlign,
        axis: Axis,
        first: Box<DockItem<TItem>>,
        last: Box<DockItem<TItem>>
    }
}


struct DockWalkStack<'a, TItem>
where TItem: Clone
{
    counter: usize,
    uid: usize,
    item: &'a mut DockItem<TItem>
}

pub enum DockEvent {
    None,
    DockTabClosed,
    DockTabCloned {tab_control_id: usize, tab_id: usize},
    DockChanged
}

pub struct DockWalker<'a, TItem>
where TItem: Clone
{
    walk_uid: usize,
    stack: Vec<DockWalkStack<'a, TItem>>,
    // forwards for Dock
    splitters: &'a mut Elements<usize, Splitter, Splitter>,
    tab_controls: &'a mut Elements<usize, TabControl, TabControl>,
    drop_quad_view: &'a mut View,
    _drag_move: &'a mut Option<FingerMoveEvent>,
    _drag_end: &'a mut Option<DockDragEnd<TItem>>,
    _close_tab: &'a mut Option<DockTabIdent>,
    _tab_select: &'a mut Option<(usize, usize)>
}

impl<'a, TItem> DockWalker<'a, TItem>
where TItem: Clone
{
    
    pub fn walk_dock_item(&mut self) -> Option<(usize, &mut DockItem<TItem>)> {
        // lets get the current item on the stack
        let push_or_pop = if let Some(stack_top) = self.stack.last_mut() {
            // return item 'count'
            match stack_top.item {
                DockItem::Single(..) => {
                    if stack_top.counter == 0 {
                        stack_top.counter += 1;
                        return Some((self.walk_uid,unsafe {mem::transmute(&mut *stack_top.item)}));
                    }
                    else {
                        None
                    }
                },
                DockItem::TabControl {..} => {
                    if stack_top.counter == 0 {
                        let uid = self.walk_uid;
                        self.walk_uid += 1;
                        stack_top.counter += 1;
                        return Some((uid, unsafe {mem::transmute(&mut *stack_top.item)}));
                    }
                    else {
                        None
                    }
                },
                DockItem::Splitter {first, last, ..} => {
                    if stack_top.counter == 0 {
                        let uid = self.walk_uid;
                        self.walk_uid += 1;
                        stack_top.counter += 1;
                        return Some((uid, unsafe {mem::transmute(&mut *stack_top.item)}));
                    }
                    else if stack_top.counter == 1 {
                        stack_top.counter += 1;
                        Some(DockWalkStack {counter: 0, uid: 0, item: unsafe {mem::transmute(first.as_mut())}})
                    }
                    else if stack_top.counter == 2 {
                        stack_top.counter += 1;
                        Some(DockWalkStack {counter: 0, uid: 0, item: unsafe {mem::transmute(last.as_mut())}})
                    }
                    else {
                        None
                    }
                }
            }
        }
        else {
            return None;
        };
        if let Some(item) = push_or_pop {
            self.stack.push(item);
            return self.walk_dock_item();
        }
        else if self.stack.len() > 0 {
            self.stack.pop();
            return self.walk_dock_item();
        }
        return None;
    }
    
    pub fn walk_handle_dock(&mut self, cx: &mut Cx, event: &mut Event) -> Option<&mut TItem> {
        // lets get the current item on the stack
        let push_or_pop = if let Some(stack_top) = self.stack.last_mut() {
            // return item 'count'
            match stack_top.item {
                DockItem::Single(item) => {
                    if stack_top.counter == 0 {
                        stack_top.counter += 1;
                        return Some(unsafe {mem::transmute(item)});
                    }
                    else {
                        None
                    }
                },
                DockItem::TabControl {current, previous, tabs} => {
                    if stack_top.counter == 0 {
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;

                        if *current < tabs.len() {
                            return Some(unsafe {mem::transmute(&mut tabs[*current].item)});
                        }
                        None
                    }
                    else {
                        let tab_control = self.tab_controls.get(stack_top.uid);
                        let mut defocus = false;
                        if !tab_control.is_none() {
                            match tab_control.unwrap().handle_tab_control(cx, event) {
                                TabControlEvent::TabSelect {tab_id} => {
                                    *previous = *current;
                                    *current = tab_id;
                                    // someday ill fix this. Luckily entire UI redraws are millisecond span
                                    cx.redraw_child_area(Area::All);
                                    *self._tab_select = Some((stack_top.uid, tab_id));
                                    defocus = true;
                                },
                                TabControlEvent::TabDragMove {fe, ..} => {
                                    *self._drag_move = Some(fe);
                                    *self._drag_end = None;
                                    self.drop_quad_view.redraw_view_area(cx);
                                },
                                TabControlEvent::TabDragEnd {fe, tab_id} => {
                                    *self._drag_move = None;
                                    *self._drag_end = Some(DockDragEnd::OldTab {
                                        fe: fe,
                                        
                                        ident: DockTabIdent {
                                            tab_control_id: stack_top.uid,
                                            
                                            tab_id: tab_id
                                        }
                                    });
                                    self.drop_quad_view.redraw_view_area(cx);
                                },
                                TabControlEvent::TabClose {tab_id} => {
                                    *self._close_tab = Some(DockTabIdent {
                                        tab_control_id: stack_top.uid,
                                        tab_id: tab_id
                                    });
                                    // if tab_id < current, subtract current if >0
                                    if tab_id < *current && *current > 0 {
                                        *current -= 1;
                                    }
                                    cx.redraw_child_area(Area::All);
                                },
                                _ => ()
                            }
                        }
                        
                        if defocus {
                            // defocus all other tabcontrols
                            for (id, tab_control) in self.tab_controls.enumerate() {
                                if *id != stack_top.uid {
                                    tab_control.set_tab_control_focus(cx, false);
                                }
                            }
                        }

                        None
                    }
                },
                DockItem::Splitter {first, last, pos, align, ..} => {
                    if stack_top.counter == 0 {
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        let split = self.splitters.get(stack_top.uid);
                        if let Some(split) = split {
                            match split.handle_splitter(cx, event) {
                                SplitterEvent::Moving {new_pos} => {
                                    *pos = new_pos;
                                    cx.redraw_pass_of(split._split_area);
                                },
                                SplitterEvent::MovingEnd {new_align, new_pos} => {
                                    *align = new_align;
                                    *pos = new_pos;
                                    cx.redraw_pass_of(split._split_area);
                                },
                                _ => ()
                            };
                        }
                        // update state in our splitter level
                        Some(DockWalkStack {counter: 0, uid: 0, item: unsafe {mem::transmute(first.as_mut())}})
                    }
                    else if stack_top.counter == 1 {
                        stack_top.counter += 1;
                        Some(DockWalkStack {counter: 0, uid: 0, item: unsafe {mem::transmute(last.as_mut())}})
                    }
                    else {
                        None
                    }
                }
            }
        }
        else {
            return None;
        };
        if let Some(item) = push_or_pop {
            self.stack.push(item);
            return self.walk_handle_dock(cx, event);
        }
        else if self.stack.len() > 0 {
            self.stack.pop();
            return self.walk_handle_dock(cx, event);
        }
        return None;
    }

    pub fn walk_draw_dock<F>(&mut self, cx: &mut Cx, mut tab_handler:F) -> Option<&'a mut TItem> 
    where F: FnMut(&mut Cx, &mut TabControl, &DockTab<TItem>, bool) {
        // lets get the current item on the stack
        let push_or_pop = if let Some(stack_top) = self.stack.last_mut() {
            // return item 'count'
            match stack_top.item {
                DockItem::Single(item) => {
                    if stack_top.counter == 0 {
                        stack_top.counter += 1;
                        return Some(unsafe {mem::transmute(item)});
                    }
                    else {
                        None
                    }
                },
                DockItem::TabControl {current, previous:_, tabs} => {
                    if stack_top.counter == 0 {
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        let tab_control = self.tab_controls.get_draw(cx, stack_top.uid, | _cx, tmpl | tmpl.clone());
                        
                        if let Ok(_) = tab_control.begin_tabs(cx) {
                            for (id, tab) in tabs.iter().enumerate() {
                                tab_handler(cx, tab_control, tab, *current == id)
                            }
                            tab_control.end_tabs(cx);
                        }
                        
                        if let Ok(_) = tab_control.begin_tab_page(cx) {
                            if *current < tabs.len() {
                                return Some(unsafe {mem::transmute(&mut tabs[*current].item)});
                            }
                            tab_control.end_tab_page(cx);
                        }
                        None
                    }
                    else {
                        let tab_control = self.tab_controls.get_draw(cx, stack_top.uid, | _cx, tmpl | tmpl.clone());
                        tab_control.end_tab_page(cx);
                        None
                    }
                },
                DockItem::Splitter {align, pos, axis, first, last} => {
                    if stack_top.counter == 0 {
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        // begin a split
                        let split = self.splitters.get_draw(cx, stack_top.uid, | _cx, tmpl | tmpl.clone());
                        split.set_splitter_state(align.clone(), *pos, axis.clone());
                        split.begin_splitter(cx);
                        Some(DockWalkStack {counter: 0, uid: 0, item: unsafe {mem::transmute(first.as_mut())}})
                    }
                    else if stack_top.counter == 1 {
                        stack_top.counter += 1;
                        
                        let split = self.splitters.get_draw(cx, stack_top.uid, | _cx, tmpl | tmpl.clone());
                        split.mid_splitter(cx);
                        Some(DockWalkStack {counter: 0, uid: 0, item: unsafe {mem::transmute(last.as_mut())}})
                    }
                    else {
                        let split = self.splitters.get_draw(cx, stack_top.uid, | _cx, tmpl | tmpl.clone());
                        split.end_splitter(cx);
                        None
                    }
                }
            }
        }
        else {
            return None
        };
        if let Some(item) = push_or_pop {
            self.stack.push(item);
            return self.walk_draw_dock(cx, tab_handler);
        }
        else if self.stack.len() > 0 {
            self.stack.pop();
            return self.walk_draw_dock(cx, tab_handler);
        }
        None
    }
}

enum DockDropKind {
    Tab(usize),
    TabsView,
    Left,
    Top,
    Right,
    Bottom,
    Center
}

impl<TItem> Dock<TItem>
where TItem: Clone
{
    pub fn proto(cx: &mut Cx) -> Dock<TItem> {
        Dock {
            // dock_items:None,
            drop_size: Vec2 {x: 100., y: 70.},
            //drop_quad_color: Color_drop_quad::id(),
            drop_quad: Quad {
                z: 10.,
                ..Quad::proto(cx)
            },
            splitters: Elements::new(Splitter::proto(cx)),
            tab_controls: Elements::new(TabControl::proto(cx)),
            drop_quad_view: View::proto_overlay(cx),
            _close_tab: None,
            _drag_move: None,
            _drag_end: None,
            _tab_select: None,
            _tweening_quad: None
        }
    }
    
    fn recur_remove_tab(dock_walk: &mut DockItem<TItem>, control_id: usize, tab_id: usize, counter: &mut usize, clone: bool, select_previous:bool) -> Option<DockTab<TItem>>
    where TItem: Clone
    {
        match dock_walk {
            DockItem::Single(_) => {},
            DockItem::TabControl {tabs, current, previous} => {
                let id = *counter;
                *counter += 1;
                if id == control_id {
                    if tab_id >= tabs.len() {
                        return None;
                    }
                    if clone && tabs[tab_id].closeable {
                        return Some(tabs[tab_id].clone())
                    }
                    else {
                        // this select the previous tab.
                        if select_previous && *previous != *current && *previous < tabs.len() - 1{
                            *current = *previous;
                        }
                        else 
                        if *current >= 1 && *current == tabs.len() - 1 {
                            *current -= 1;
                        }
                        return Some(tabs.remove(tab_id));
                    }
                }
            },
            DockItem::Splitter {first, last, ..} => {
                *counter += 1;
                let left = Self::recur_remove_tab(first, control_id, tab_id, counter, clone, select_previous);
                if !left.is_none() {
                    return left
                }
                let right = Self::recur_remove_tab(last, control_id, tab_id, counter, clone, select_previous);
                if !right.is_none() {
                    return right
                }
            }
        }
        None
    }
    
    fn recur_collapse_empty(dock_walk: &mut DockItem<TItem>) -> bool
    where TItem: Clone
    {
        match dock_walk {
            DockItem::Single(_) => {},
            DockItem::TabControl {tabs, ..} => {
                return tabs.len() == 0
            },
            DockItem::Splitter {first, last, ..} => {
                let rem_first = Self::recur_collapse_empty(first);
                let rem_last = Self::recur_collapse_empty(last);
                if rem_first && rem_last {
                    return true;
                }
                if rem_first {
                    *dock_walk = *last.clone();
                }
                else if rem_last {
                    *dock_walk = *first.clone();
                }
            }
        }
        false
    }
    
    fn recur_split_dock(dock_walk: &mut DockItem<TItem>, items: &Vec<DockTab<TItem>>, control_id: usize, kind: &DockDropKind, counter: &mut usize)->Option<DockTabIdent>
    where TItem: Clone
    {
        match dock_walk {
            DockItem::Single(_) => {},
            DockItem::TabControl {tabs, previous:_, current} => {
                let id = *counter;
                *counter += 1;
                if id == control_id {
                    match kind {
                        DockDropKind::Tab(id) => {
                            let mut idc = *id;
                            for item in items {
                                tabs.insert(idc, item.clone());
                                idc += 1;
                            }
                            *current = idc - 1;
                            return Some(DockTabIdent{tab_control_id:control_id, tab_id:*current})
                        },
                        DockDropKind::Left => {
                            *dock_walk = DockItem::Splitter {
                                align: SplitterAlign::Weighted,
                                pos: 0.5,
                                axis: Axis::Vertical,
                                last: Box::new(dock_walk.clone()),
                                first: Box::new(DockItem::TabControl {current: 0, previous:0, tabs: items.clone()})
                            };
                            return Some(DockTabIdent{tab_control_id:control_id+1, tab_id:0})
                        },
                        DockDropKind::Right => {
                            *dock_walk = DockItem::Splitter {
                                align: SplitterAlign::Weighted,
                                pos: 0.5,
                                axis: Axis::Vertical,
                                first: Box::new(dock_walk.clone()),
                                last: Box::new(DockItem::TabControl {current: 0, previous:0, tabs: items.clone()})
                            };
                            return Some(DockTabIdent{tab_control_id:control_id+2, tab_id:0})
                        },
                        DockDropKind::Top => {
                            *dock_walk = DockItem::Splitter {
                                align: SplitterAlign::Weighted,
                                pos: 0.5,
                                axis: Axis::Horizontal,
                                last: Box::new(dock_walk.clone()),
                                first: Box::new(DockItem::TabControl {current: 0, previous:0, tabs: items.clone()})
                            };
                            return Some(DockTabIdent{tab_control_id:control_id+1, tab_id:0})
                        },
                        DockDropKind::Bottom => {
                            *dock_walk = DockItem::Splitter {
                                align: SplitterAlign::Weighted,
                                pos: 0.5,
                                axis: Axis::Horizontal,
                                first: Box::new(dock_walk.clone()),
                                last: Box::new(DockItem::TabControl {current: 0, previous:0, tabs: items.clone()})
                            };
                            return Some(DockTabIdent{tab_control_id:control_id+2, tab_id:0})
                        },
                        DockDropKind::TabsView |
                        DockDropKind::Center => {
                            *current = tabs.len() + items.len() - 1;
                            for item in items {
                                tabs.push(item.clone());
                            }
                            return Some(DockTabIdent{tab_control_id:control_id, tab_id:tabs.len()-1})
                        }
                    }
                }
            },
            DockItem::Splitter {first, last, ..} => {
                *counter += 1;
                if let Some(ret) = Self::recur_split_dock(first, items, control_id, kind, counter){
                    return Some(ret)
                }
                if let Some(ret) =  Self::recur_split_dock(last, items, control_id, kind, counter){
                    return Some(ret)
                }
            }
        }
        None
    }
    
    fn get_drop_kind(pos: Vec2, drop_size: Vec2, tvr: Rect, cdr: Rect, tab_rects: Vec<Rect>) -> (DockDropKind, Rect) {
        // this is how the drop areas look
        //    |            Tab                |
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
        
        if tvr.contains(pos.x, pos.y) {
            for (id, tr) in tab_rects.iter().enumerate() {
                if tr.contains(pos.x, pos.y) {
                    return (DockDropKind::Tab(id), *tr)
                }
            }
            return (DockDropKind::TabsView, tvr)
        }
        if pos.y < cdr.y + drop_size.y {
            return (DockDropKind::Top, Rect {
                x: cdr.x,
                y: cdr.y,
                w: cdr.w,
                h: 0.5 * cdr.h
            })
        }
        if pos.y > cdr.y + cdr.h - drop_size.y {
            return (DockDropKind::Bottom, Rect {
                x: cdr.x,
                y: cdr.y + 0.5 * cdr.h,
                w: cdr.w,
                h: 0.5 * cdr.h
            })
        }
        if pos.x < cdr.x + drop_size.x {
            return (DockDropKind::Left, Rect {
                x: cdr.x,
                y: cdr.y,
                w: 0.5 * cdr.w,
                h: cdr.h
            })
        }
        if pos.x > cdr.x + cdr.w - drop_size.x {
            return (DockDropKind::Right, Rect {
                x: cdr.x + 0.5 * cdr.w,
                y: cdr.y,
                w: 0.5 * cdr.w,
                h: cdr.h
            })
        }
        (DockDropKind::Center, cdr.clone())
    }
    
    pub fn dock_drag_out(&mut self, cx: &mut Cx) {
        self._drag_move = None;
        self.drop_quad_view.redraw_view_area(cx);
    }
    
    pub fn dock_drag_move(&mut self, cx: &mut Cx, fe: FingerMoveEvent) {
        self._drag_move = Some(fe);
        self.drop_quad_view.redraw_view_area(cx);
    }
    
    pub fn dock_drag_cancel(&mut self, cx: &mut Cx) {
        self._drag_move = None;
        self.drop_quad_view.redraw_view_area(cx);
    }
    
    pub fn dock_drag_end(&mut self, _cx: &mut Cx, fe: FingerUpEvent, new_items: Vec<DockTab<TItem>>) {
        self._drag_move = None;
        self._drag_end = Some(DockDragEnd::NewItems {
            fe: fe,
            items: new_items
        });
    }
    
    pub fn handle_dock(&mut self, cx: &mut Cx, _event: &mut Event, dock_items: &mut DockItem<TItem>) -> DockEvent {
        if let Some(close_tab) = &self._close_tab {
            Self::recur_remove_tab(dock_items, close_tab.tab_control_id, close_tab.tab_id, &mut 0, false, false);
            Self::recur_collapse_empty(dock_items);
            cx.redraw_child_area(Area::All);
            self._close_tab = None;
            return DockEvent::DockTabClosed
        }
        if let Some(drag_end) = self._drag_end.clone() {
            self._drag_end = None;
            let mut tab_clone_ident = None;
            let fe = match &drag_end {DockDragEnd::OldTab {fe, ..} => fe, DockDragEnd::NewItems {fe, ..} => fe};
            for (target_id, tab_control) in self.tab_controls.enumerate() {
                
                let cdr = tab_control.get_content_drop_rect(cx);
                let tvr = tab_control.get_tabs_view_rect(cx);
                if tvr.contains(fe.abs.x, fe.abs.y) || cdr.contains(fe.abs.x, fe.abs.y) { // we might got dropped elsewhere
                    // ok now, we ask the tab_controls rect
                    let tab_rects = tab_control.get_tab_rects(cx);
                    let (kind, _rect) = Self::get_drop_kind(fe.abs, self.drop_size, tvr, cdr, tab_rects);
                    
                    // alright our drag_end is an enum
                    // its either a previous tabs index
                    // or its a new Item
                    // we have a kind!
                    let mut do_tab_clone = false;
                    let items = match &drag_end {
                        DockDragEnd::OldTab {ident, ..} => {
                            if fe.modifiers.control || fe.modifiers.logo {
                                do_tab_clone = true;
                            }
                            let item = Self::recur_remove_tab(dock_items, ident.tab_control_id, ident.tab_id, &mut 0, do_tab_clone, true);
                            if let Some(item) = item {
                                if !item.closeable {
                                    do_tab_clone = false;
                                }
                                vec![item]
                            }
                            else {
                                vec![]
                            }
                        },
                        DockDragEnd::NewItems {items, ..} => {
                            items.clone()
                        }
                    };
                    // alright we have a kind.
                    if items.len() > 0 {
                        let new_ident = Self::recur_split_dock(
                            dock_items,
                            &items,
                            *target_id,
                            &kind,
                            &mut 0
                        );
                        if do_tab_clone{
                            tab_clone_ident = new_ident;
                        }
                    };
                }
            }
            Self::recur_collapse_empty(dock_items);
            cx.redraw_child_area(Area::All);
            //Self::recur_debug_dock(self.dock_items.as_mut().unwrap(), &mut 0, 0);
            if let Some(ident) = tab_clone_ident {
                return DockEvent::DockTabCloned {tab_control_id: ident.tab_control_id, tab_id: ident.tab_id}
            }
            return DockEvent::DockChanged
        };
        // ok we need to pull out the TItem from our dockpanel
        DockEvent::None
    }
    
    pub fn draw_dock(&mut self, cx: &mut Cx) {
        // lets draw our hover layer if need be
        if let Some(fe) = &self._drag_move {
            if let Err(()) = self.drop_quad_view.begin_view(cx, Layout::abs_origin_zero()) {
                return
            }
            let mut found_drop_zone = false;
            for (id, tab_control) in self.tab_controls.enumerate() {
                
                let cdr = tab_control.get_content_drop_rect(cx);
                let tvr = tab_control.get_tabs_view_rect(cx);
                if tvr.contains(fe.abs.x, fe.abs.y) || cdr.contains(fe.abs.x, fe.abs.y) {
                    let tab_rects = tab_control.get_tab_rects(cx);
                    let (_kind, rect) = Self::get_drop_kind(fe.abs, self.drop_size, tvr, cdr, tab_rects);
                    
                    if !self._tweening_quad.is_none() && self._tweening_quad.unwrap().0 != *id {
                        // restarts the animation by removing drop_quad
                        self._tweening_quad = None;
                    }
                    
                    // yay, i can finally do these kinds of animations!
                    let (dr, alpha) = if self._tweening_quad.is_none() {
                        self._tweening_quad = Some((*id, rect, 0.));
                        (rect, 0.)
                    }
                    else {
                        let (id, old_rc, old_alpha) = self._tweening_quad.unwrap();
                        let move_speed = 0.0;
                        let alpha_speed = 0.0;
                        let alpha = old_alpha * alpha_speed + (1. - alpha_speed);
                        let rc = Rect {
                            x: old_rc.x * move_speed + rect.x * (1. - move_speed),
                            y: old_rc.y * move_speed + rect.y * (1. - move_speed),
                            w: old_rc.w * move_speed + rect.w * (1. - move_speed),
                            h: old_rc.h * move_speed + rect.h * (1. - move_speed)
                        };
                        let dist = (rc.x - rect.x)
                            .abs()
                            .max((rc.y - rect.y).abs())
                            .max((rc.w - rect.w).abs())
                            .max((rc.h - rect.h).abs())
                            .max(100. - alpha * 100.);
                        if dist>0.5 { // keep redrawing until we are close
                            // cx.redraw_previous_areas();
                            //self.drop_quad_view.redraw_view_area(cx);
                        }
                        self._tweening_quad = Some((id, rc, alpha));
                        (rc, alpha)
                    };
                    self.drop_quad.color = Theme::color_drop_quad().base(cx);
                    self.drop_quad.color.a = alpha * 0.8;
                    found_drop_zone = true;
                    self.drop_quad.draw_quad_rel(cx, dr);
                }
            }
            if !found_drop_zone {
                self._tweening_quad = None;
            }
            self.drop_quad_view.end_view(cx);
        }
    }
    
    pub fn walker<'a>(&'a mut self, dock_items: &'a mut DockItem<TItem>) -> DockWalker<'a,
        TItem> {
        let mut stack = Vec::new();
        //if !self.dock_items.is_none(){
        stack.push(DockWalkStack {counter: 0, uid: 0, item: dock_items});
        //}
        DockWalker {
            walk_uid: 0,
            stack: stack,
            splitters: &mut self.splitters,
            tab_controls: &mut self.tab_controls,
            _drag_move: &mut self._drag_move,
            _drag_end: &mut self._drag_end,
            _close_tab: &mut self._close_tab,
            _tab_select: &mut self._tab_select,
            drop_quad_view: &mut self.drop_quad_view
        }
    }

}

/*
fn recur_debug_dock(dock_walk:&mut DockItem<TItem>, counter:&mut usize, depth:usize)
where TItem: Clone
{
let mut indent = String::new();
for i in 0..depth{indent.push_str(" ")}
match dock_walk{
DockItem::Single(item)=>{},
DockItem::TabControl{tabs,..}=>{
let id = *counter;
*counter += 1;
println!("{}TabControl {}", indent, id);
for (id,tab) in tabs.iter().enumerate(){
println!("{} Tab{} {}", indent, id, tab.title);
}
},
DockItem::Splitter{first,last,..}=>{
let id = *counter;
*counter += 1;
println!("{}Splitter {}", indent, id);
Self::recur_debug_dock(first, counter, depth + 1);
Self::recur_debug_dock(last, counter, depth + 1);
}
}
}*/