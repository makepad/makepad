use render::*;
use crate::scrollview::*;

#[derive(Clone, Default)]
pub struct List {
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

pub enum ListItemEvent {
    ItemAnimate(AnimateEvent),
    ItemAnimEnded,
    ItemSelect,
    ItemDeselect,
    ItemCleanup,
    ItemOver,
    ItemOut
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

impl List {
    pub fn set_list_len<F>(&mut self, cx: &mut Cx, len:usize, mut cb:F)
    where F: FnMut(&mut Cx, usize)->Anim
    {
        if self.list_items.len() < len{
            for i in self.list_items.len()..len{
                self.list_items.push(ListItem{
                    animator:Animator::new(cb(cx, i)),
                    is_selected:false
                })
            }
        }
        else{
            self.list_items.truncate(len);
        }
    }
    
    pub fn handle_list_scroll_bars(&mut self, cx:&mut Cx, event:&mut Event, view:&mut ScrollView)
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
    
    pub fn begin_list(&mut self, cx: &mut Cx, view:&mut ScrollView, row_height:f32)->Result<(),()>
    {
        view.begin_view(cx, Layout {
            direction: Direction::Down,
            ..Layout::default()
        })?;

        self.set_visible_range_and_scroll(cx, view, row_height);
        
        Ok(())
    }
    
    
    pub fn walk_turtle_to_end(&mut self, cx: &mut Cx, row_height:f32){
        let left = (self.list_items.len() - self.end_item) as f32 * row_height;
        cx.walk_turtle(Bounds::Fill, Bounds::Fix(left), Margin::zero(), None);
    }
    
    
    pub fn end_list(&mut self, cx: &mut Cx, view:&mut ScrollView){
        view.end_view(cx);
        if let Some(set_scroll_pos) = self.set_scroll_pos{
            view.set_scroll_pos(cx, set_scroll_pos);
        }
    }
    
    pub fn set_visible_range_and_scroll(&mut self, cx: &mut Cx, view:&mut ScrollView, row_height:f32){
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
        if set_scroll_pos{
            self.set_scroll_pos = Some(scroll_pos);
        }
        else{
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
    
    pub fn get_prev_single_selection(&self)->ListSelect{
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
    
    pub fn handle_selection<F>(&mut self, cx: &mut Cx, event: &mut Event, select:ListSelect, mut cb:F) -> ListEvent
    where F: FnMut(&mut Cx, ListItemEvent, &mut ListItem, usize)
    {
        let mut select = select;
        
        for counter in self.start_item..self.end_item {
            if counter >= self.list_items.len() {
                break;
            }
            let item = &mut self.list_items[counter];
            match event.hits(cx, item.animator.area, HitOpt::default()) {
                Event::Animate(ae) => {
                    cb(cx, ListItemEvent::ItemAnimate(ae), item, counter)
                },
                Event::AnimEnded(_) => {
                    cb(cx, ListItemEvent::ItemAnimEnded, item, counter)
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
                            cb(cx, ListItemEvent::ItemOver, item, counter);
                        },
                        HoverState::Out => {
                            cb(cx, ListItemEvent::ItemOut, item, counter);
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
                    cb(cx, ListItemEvent::ItemDeselect, dm, counter);
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
                                cb(cx, ListItemEvent::ItemDeselect, dm, select_index);
                            }
                        }
                        self.selection.truncate(0);
                        for i in start..= end {
                            let dm = &mut self.list_items[i];
                            dm.is_selected = true;
                            cb(cx, ListItemEvent::ItemSelect, dm, i);
                            self.selection.push(i);
                        }
                        
                    }
                }
            },
            ListSelect::Toggle(select_index) => {
                let dm = &mut self.list_items[select_index];
                if dm.is_selected {
                    dm.is_selected = false;
                    cb(cx, ListItemEvent::ItemDeselect, dm, select_index);
                    if let Some(pos) = self.selection.iter().position( | v | *v == select_index) {
                        self.selection.remove(pos);
                    }
                }
                else {
                    self.selection.push(select_index);
                    dm.is_selected = true;
                    cb(cx, ListItemEvent::ItemOver, dm, select_index);
                }
            },
            ListSelect::All => {
                self.selection.truncate(0);
                for i in 0..self.list_items.len() {
                    self.selection.push(i);
                    let dm = &mut self.list_items[i];
                    dm.is_selected = true;
                    cb(cx, ListItemEvent::ItemOver, dm, i);
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
                        cb(cx, ListItemEvent::ItemCleanup, dm, *counter);
                    }
                }
                self.selection.truncate(0);
                self.selection.push(select_index);
                let dm = &mut self.list_items[select_index];
                dm.is_selected = true;
                cb(cx, ListItemEvent::ItemOver, dm, select_index);
                
                return ListEvent::SelectSingle(select_index)
            },
            _ => ()
        }
        match select{
            ListSelect::Range(_) | ListSelect::Toggle(_) | ListSelect::All=>{
                ListEvent::SelectMultiple
            },
            _=> ListEvent::None
        }
       
    }
    /*
    pub fn draw_log_list(&mut self, cx: &mut Cx, bm: &BuildManager) {
        if let Err(_) = self.style.view.begin_view(cx, Layout {
            direction: Direction::Down,
            ..Layout::default()
        }) {
            return
        }
        
        let view_rect = cx.get_turtle_rect();
        
        
        // the maximum scroll position given the amount of log items
        let max_scroll_y = ((self.log_items.len() + 1) as f32 * self.style.row_height - view_rect.h).max(0.);
        
        // tail the log
        let (scroll_pos, move_scroll_pos) = if self.tail_log {
            (Vec2 {x: 0., y: max_scroll_y}, true)
        }
        else {
            let sp = self.style.view.get_scroll_pos(cx);
            
            // scroll item into view
            if let Some(scroll_item_in_view) = self.scroll_item_in_view {
                self.scroll_item_in_view = None;
                let item_y = scroll_item_in_view as f32 * self.style.row_height;
                println!("{}", item_y);
                let dy = (item_y + self.style.row_height) - (sp.y + view_rect.h);
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
        
        // we need to find the first item to draw
        self.start_item = (scroll_pos.y / self.style.row_height).floor() as usize;
        let start_scroll = (self.start_item as f32) * self.style.row_height;
        
        // lets jump the turtle forward by scrollpos.y
        cx.move_turtle(0., start_scroll);
        
        let item_layout = self.style.get_line_layout();
        
        let mut counter = 0;
        for i in self.start_item..self.log_items.len() {
            
            let walk = cx.get_rel_turtle_walk();
            if walk.y - start_scroll > view_rect.h + self.style.row_height {
                // this is a virtual viewport, so bail if we are below the view
                let left = (self.log_items.len() - i) as f32 * self.style.row_height;
                cx.walk_turtle(Bounds::Fill, Bounds::Fix(left), Margin::zero(), None);
                break
            }
            
            self.style.draw_log_item(cx, &mut self.log_items[i]);
            
            counter += 1;
            self.end_item = i;
        }
        
        self.style.draw_status_line(cx, counter, &bm);
        counter += 1;
        
        // draw filler nodes
        let view_total = cx.get_turtle_bounds();
        let mut y = view_total.y;
        while y < view_rect.h {
            self.style.item_bg.color = if counter & 1 == 0 {self.style.bg_even} else {self.style.bg_odd};
            self.style.item_bg.draw_quad_walk(cx, Bounds::Fill, Bounds::Fix(self.style.row_height), Margin::zero());
            cx.set_turtle_bounds(view_total); // do this so the scrollbar doesnt show up
            y += self.style.row_height;
            counter += 1;
        }
        self.style.view.end_view(cx);
        if move_scroll_pos {
            self.style.view.set_scroll_pos(cx, scroll_pos);
        }
    }*/
}
