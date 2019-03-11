use std::mem;

use render::*;
use crate::splitter::*;
use crate::tabcontrol::*;

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

#[derive(Clone)]
pub struct Dock<TItem>
where TItem: Clone
{
    pub dock_items: DockItem<TItem>,
    pub splitters: Elements<Splitter, usize>,
    pub tab_controls: Elements<TabControl, usize>
}

struct DockStackLevel<'a, TItem>
where TItem: Clone
{
    counter:usize,
    uid:usize,
    item:&'a mut DockItem<TItem>
}

pub struct DockWalker<'a, TItem>
where TItem: Clone
{
    walk_uid:usize,
    splitters:&'a mut Elements<Splitter, usize>,
    tab_controls:&'a mut Elements<TabControl, usize>,
    stack:Vec<DockStackLevel<'a, TItem>>,
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
                        let tab_control = self.tab_controls.get(cx, stack_top.uid);
                        
                        // ok so this one returns 'DragTab(x,y)
                        tab_control.handle_tab_control(cx, event);

                        return Some(unsafe{mem::transmute(&mut tabs[*current].item)});
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
                        let split = self.splitters.get(cx, stack_top.uid);
                        match split.handle_splitter(cx, event){
                            SplitterEvent::Moving{new_pos}=>{
                                *pos = new_pos;
                            },
                            _=>()
                        };
                        // update state in our splitter level
                        Some(DockStackLevel{counter:0, uid:0, item:unsafe{mem::transmute(first.as_mut())}})
                    }
                    else if stack_top.counter == 1{
                        stack_top.counter +=1;
                        Some(DockStackLevel{counter:0, uid:0, item:unsafe{mem::transmute(last.as_mut())}})
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
                        let tab_control = self.tab_controls.get(cx, stack_top.uid);
                        tab_control.begin_tabs(cx);
                        for tab in tabs.iter(){
                            tab_control.draw_tab(cx, &tab.title, false);
                        }
                        tab_control.end_tabs(cx);
                        tab_control.begin_tab_page(cx);
                        return Some(unsafe{mem::transmute(&mut tabs[*current].item)});
                    }
                    else{
                        let tab_control = self.tab_controls.get(cx, stack_top.uid);
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
                        let split = self.splitters.get(cx, stack_top.uid);
                        split.set_splitter_state(align.clone(), *pos, axis.clone());
                        split.begin_splitter(cx);
                        Some(DockStackLevel{counter:0, uid:0, item:unsafe{mem::transmute(first.as_mut())}})
                    }
                    else if stack_top.counter == 1{
                        stack_top.counter +=1 ;

                        let split = self.splitters.get(cx, stack_top.uid);
                        split.mid_splitter(cx);
                        Some(DockStackLevel{counter:0, uid:0, item:unsafe{mem::transmute(last.as_mut())}})
                    }
                    else{
                        let split = self.splitters.get(cx, stack_top.uid);
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

impl<TItem> Dock<TItem>
where TItem: Clone
{
    pub fn walker<'a>(&'a mut self)->DockWalker<'a, TItem>{
        let mut stack = Vec::new();
        stack.push(DockStackLevel{counter:0, uid:0, item:&mut self.dock_items});
        DockWalker{
            walk_uid:0,
            splitters:&mut self.splitters,
            tab_controls:&mut self.tab_controls,
            stack:stack
        }
    }
}
