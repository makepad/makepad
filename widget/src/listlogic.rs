use render::*;
use crate::scrollview::*;

#[derive(Clone, Default)]
pub struct ListLogic {
    pub list_items: Vec<ListItem>,
    pub scroll_item_in_view: Option<usize>,
    pub set_scroll_pos: Option<Vec2>,
    pub tail_list: bool,
    pub start_item: usize,
    pub end_item: usize,
    pub end_fill: usize,
    pub selection: Vec<usize>,
    pub last_range: Option<(usize, usize)>,
}

#[derive(Clone)]
pub struct ListItem {
    pub animator: Animator,
    pub is_selected: bool
}

pub enum ListLogicEvent {
    Animate(AnimateEvent),
    AnimEnded,
    Select,
    Deselect,
    Cleanup,
    Over,
    Out
}

pub enum ListEvent {
    SelectSingle(usize),
    SelectMultiple,
    None
}

#[derive(PartialEq)]
pub enum ListSelect {
    None,
    Single(usize),
    Range(usize),
    Toggle(usize),
    All
}

impl ListSelect {
    pub fn item_index(&self) -> Option<usize> {
        match self {
            ListSelect::Single(index) => Some(*index),
            _ => None
        }
    }
}

impl ListLogic {
    pub fn set_list_len(&mut self,  len: usize)
    {
        if self.list_items.len() < len {
            for _ in self.list_items.len()..len {
                self.list_items.push(ListItem {
                    animator: Animator::default(),
                    is_selected: false
                })
            }
        }
        else {
            self.list_items.truncate(len);
        }
    }
    
    pub fn handle_list_scroll_bars(&mut self, cx: &mut Cx, event: &mut Event, view: &mut ScrollView)
    {
        if view.handle_scroll_bars(cx, event) {
            view.redraw_view_area(cx);
            match &event {
                Event::FingerScroll {..} => {
                    self.tail_list = false;
                },
                Event::FingerMove {..} => {
                    self.tail_list = false;
                },
                _ => ()
            }
        }
    }
    
    pub fn begin_list(&mut self, cx: &mut Cx, view: &mut ScrollView, row_height: f32) -> Result<(), ()>
    {
        view.begin_view(cx, Layout {
            direction: Direction::Down,
            ..Layout::default()
        }) ?;
        
        self.set_visible_range_and_scroll(cx, view, row_height);
        
        Ok(())
    }
    
    
    pub fn walk_turtle_to_end(&mut self, cx: &mut Cx, row_height: f32) {
        let left = (self.list_items.len() - self.end_item) as f32 * row_height;
        cx.walk_turtle(Walk::wh(Width::Fill, Height::Fix(left)));
    }
    
    
    pub fn end_list(&mut self, cx: &mut Cx, view: &mut ScrollView) {
        view.end_view(cx);
        if let Some(set_scroll_pos) = self.set_scroll_pos {
            view.set_scroll_pos(cx, set_scroll_pos);
        }
    }
    
    pub fn set_visible_range_and_scroll(&mut self, cx: &mut Cx, view: &mut ScrollView, row_height: f32) {
        let view_rect = cx.get_turtle_rect();
        
        // the maximum scroll position given the amount of log items
        let max_scroll_y = ((self.list_items.len() + 1) as f32 * row_height - view_rect.h).max(0.);
        
        // tail the log
        let (scroll_pos, set_scroll_pos) = if self.tail_list {
            (Vec2 {x: 0., y: max_scroll_y}, true)
        }
        else {
            let sp = view.get_scroll_pos(cx);
            
            // scroll item into view
            if let Some(scroll_item_in_view) = self.scroll_item_in_view {
                self.scroll_item_in_view = None;
                let item_y = scroll_item_in_view as f32 * row_height;
                let dy = (item_y + row_height) - (sp.y + view_rect.h);
                if item_y < sp.y {
                    (Vec2 {x: 0., y: item_y}, true)
                }
                else if dy > 0. {
                    (Vec2 {x: 0., y: sp.y + dy}, true)
                }
                else {
                    (sp, false)
                }
            }
            else {
                // clamp the scrollbar to our max list size
                if sp.y > max_scroll_y {
                    (Vec2 {x: 0., y: max_scroll_y}, false)
                }
                else {
                    (sp, false)
                }
            }
        };
        
        let start_item = (scroll_pos.y / row_height).floor() as usize;
        let end_item = ((scroll_pos.y + view_rect.h + row_height) / row_height).ceil() as usize;
        
        self.start_item = start_item.min(self.list_items.len());
        self.end_fill = end_item;
        self.end_item = end_item.min(self.list_items.len());
        
        let start_scroll = (self.start_item as f32) * row_height;
        if set_scroll_pos {
            self.set_scroll_pos = Some(scroll_pos);
        }
        else {
            self.set_scroll_pos = None;
        }
        // lets jump the turtle forward by scrollpos.y
        cx.move_turtle(0., start_scroll);
    }
    
    pub fn get_next_single_selection(&self) -> ListSelect {
        if let Some(last) = self.selection.last() {
            let next = last + 1;
            if next >= self.list_items.len() { // wrap around
                ListSelect::Single(0)
            }
            else {
                ListSelect::Single(next)
            }
        }
        else {
            ListSelect::Single(0)
        }
    }
    
    pub fn get_prev_single_selection(&self) -> ListSelect {
        if let Some(first) = self.selection.last() {
            if *first == 0 { // wrap around
                ListSelect::Single(self.list_items.len().max(1) - 1)
            }
            else {
                ListSelect::Single(first - 1)
            }
        }
        else {
            ListSelect::Single(0)
        }
    }
    
    pub fn handle_list_logic<F>(&mut self, cx: &mut Cx, event: &mut Event, select: ListSelect, mut cb: F) -> ListEvent
    where F: FnMut(&mut Cx, ListLogicEvent, &mut ListItem, usize)
    {
        let mut select = select;
        
        for counter in self.start_item..self.end_item {
            if counter >= self.list_items.len() {
                break;
            }
            let item = &mut self.list_items[counter];
            match event.hits(cx, item.animator.area, HitOpt::default()) {
                Event::Animate(ae) => {
                    cb(cx, ListLogicEvent::Animate(ae), item, counter)
                },
                Event::AnimEnded(_) => {
                    cb(cx, ListLogicEvent::AnimEnded, item, counter)
                },
                Event::FingerDown(fe) => {
                    cx.set_down_mouse_cursor(MouseCursor::Hand);
                    if fe.modifiers.logo || fe.modifiers.control {
                        select = ListSelect::Toggle(counter)
                    }
                    else if fe.modifiers.shift {
                        select = ListSelect::Range(counter)
                    }
                    else {
                        select = ListSelect::Single(counter)
                    }
                },
                Event::FingerUp(_fe) => {
                },
                Event::FingerMove(_fe) => {
                },
                Event::FingerHover(fe) => {
                    cx.set_hover_mouse_cursor(MouseCursor::Hand);
                    match fe.hover_state {
                        HoverState::In => {
                            cb(cx, ListLogicEvent::Over, item, counter);
                        },
                        HoverState::Out => {
                            cb(cx, ListLogicEvent::Out, item, counter);
                        },
                        _ => ()
                    }
                },
                _ => ()
            }
        };
        // clean up outside of window
        if let Some(last_range) = self.last_range {
            for counter in last_range.0..last_range.1 {
                if counter >= self.list_items.len() {
                    break;
                }
                if counter < self.start_item || counter >= self.end_item {
                    let dm = &mut self.list_items[counter];
                    cb(cx, ListLogicEvent::Deselect, dm, counter);
                }
            }
        }
        self.last_range = Some((self.start_item, self.end_item));
        
        match select {
            ListSelect::Range(select_index) => {
                if let Some(first) = self.selection.first() {
                    if let Some(last) = self.selection.last() {
                        
                        let (start, end) = if select_index < *first {
                            (select_index, *last)
                        }
                        else if select_index > *last {
                            (*first, select_index)
                        }
                        else {
                            (select_index, select_index)
                        };
                        
                        for counter in &self.selection {
                            if *counter >= self.list_items.len() || *counter >= start && *counter <= end {
                                continue;
                            }
                            let dm = &mut self.list_items[*counter];
                            if *counter != select_index {
                                dm.is_selected = false;
                                cb(cx, ListLogicEvent::Deselect, dm, select_index);
                            }
                        }
                        self.selection.truncate(0);
                        for i in start..= end {
                            let dm = &mut self.list_items[i];
                            dm.is_selected = true;
                            cb(cx, ListLogicEvent::Select, dm, i);
                            self.selection.push(i);
                        }
                        
                    }
                }
            },
            ListSelect::Toggle(select_index) => {
                let dm = &mut self.list_items[select_index];
                if dm.is_selected {
                    dm.is_selected = false;
                    cb(cx, ListLogicEvent::Deselect, dm, select_index);
                    if let Some(pos) = self.selection.iter().position( | v | *v == select_index) {
                        self.selection.remove(pos);
                    }
                }
                else {
                    self.selection.push(select_index);
                    dm.is_selected = true;
                    cb(cx, ListLogicEvent::Over, dm, select_index);
                }
            },
            ListSelect::All => {
                self.selection.truncate(0);
                for i in 0..self.list_items.len() {
                    self.selection.push(i);
                    let dm = &mut self.list_items[i];
                    dm.is_selected = true;
                    cb(cx, ListLogicEvent::Over, dm, i);
                }
            },
            ListSelect::Single(select_index) => {
                for counter in &self.selection {
                    if *counter >= self.list_items.len() {
                        continue;
                    }
                    let dm = &mut self.list_items[*counter];
                    if *counter != select_index {
                        dm.is_selected = false;
                        cb(cx, ListLogicEvent::Cleanup, dm, *counter);
                    }
                }
                self.selection.truncate(0);
                self.selection.push(select_index);
                let dm = &mut self.list_items[select_index];
                dm.is_selected = true;
                cb(cx, ListLogicEvent::Over, dm, select_index);
                
                return ListEvent::SelectSingle(select_index)
            },
            _ => ()
        }
        match select {
            ListSelect::Range(_) | ListSelect::Toggle(_) | ListSelect::All => {
                ListEvent::SelectMultiple
            },
            _ => ListEvent::None
        }
        
    }
    
}
