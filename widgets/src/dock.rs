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
pub struct Dock<TItem, TSplitter, TTabControl>
where TItem: Clone, 
      TSplitter: Clone + ElementLife + SplitterLike,
      TTabControl: Clone + ElementLife + TabControlLike
{
    pub dock_items: DockItem<TItem>,
    pub splitters: Elements<TSplitter, usize>,
    pub tab_controls: Elements<TTabControl, usize>
}

struct DockStackLevel<'a, TItem>
where TItem: Clone
{
    counter:usize,
    uid:usize,
    item:&'a mut DockItem<TItem>
}

pub struct DockWalker<'a, TItem, TSplitter, TTabControl>
where TItem: Clone, 
      TSplitter: Clone + ElementLife + SplitterLike,
      TTabControl: Clone + ElementLife + TabControlLike
{
    walk_uid:usize,
    splitters:&'a mut Elements<TSplitter, usize>,
    tab_controls:&'a mut Elements<TTabControl, usize>,
    stack:Vec<DockStackLevel<'a, TItem>>,
}



impl<'a, TItem, TSplitter, TTabbed> DockWalker<'a, TItem, TSplitter, TTabbed>
where TItem: Clone, 
      TSplitter: Clone + ElementLife + SplitterLike,
      TTabbed: Clone + ElementLife + TabControlLike
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
                        return Some(unsafe{mem::transmute(&mut tabs[*current].item)});
                    }
                    else{
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

impl<TItem, TSplitter, TTabbed> Dock<TItem, TSplitter, TTabbed>
where TItem: Clone,
      TSplitter: Clone + ElementLife + SplitterLike,
      TTabbed: Clone + ElementLife + TabControlLike
{
    pub fn walker<'a>(&'a mut self)->DockWalker<'a, TItem, TSplitter, TTabbed>{
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
