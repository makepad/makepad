use std::mem;

use shui::*;
use crate::splitter::*;

#[derive(Clone)]
pub enum DockItem<TItem>
where TItem: Clone
{
    Single(TItem),
    Tabbed{
        current_tab:usize,
        tabs:Vec<TItem>,
    },
    Split{
        split_mode:SplitterMode,
        split_pos:f32,
        axis:Axis,
        left:Box<DockItem<TItem>>, 
        right:Box<DockItem<TItem>>
    }
}

#[derive(Clone)]
pub struct Dock<TItem, TSplitter>
where TItem: Clone, 
      TSplitter: Clone + ElementLife + SplitterLike
{
    pub item_root: DockItem<TItem>,
    pub splitters: Elements<TSplitter, usize>
}

struct DockStackLevel<'a, TItem>
where TItem: Clone
{
    counter:usize,
    uid:usize,
    item:&'a mut DockItem<TItem>
}

pub struct DockWalker<'a, TItem, TSplitter>
where TItem: Clone, 
      TSplitter: Clone + ElementLife + SplitterLike
{
    walk_uid:usize,
    splitters:&'a mut Elements<TSplitter, usize>,
    stack:Vec<DockStackLevel<'a, TItem>>,
}



impl<'a, TItem, TSplitter> DockWalker<'a, TItem, TSplitter>
where TItem: Clone, 
      TSplitter: Clone + ElementLife + SplitterLike
{
    pub fn handle_walk(&'a mut self, cx: &mut Cx, event: &mut Event)->Option<&'a mut TItem>{
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
                DockItem::Tabbed{current_tab, tabs}=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        return Some(unsafe{mem::transmute(&mut tabs[*current_tab])});
                    }
                    else{
                        None
                    }
                },
                DockItem::Split{left, right, ..}=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        let split = self.splitters.get(cx, stack_top.uid);
                        split.handle_splitter(cx, event);
                        // update state in our splitter level
                        Some(DockStackLevel{counter:0, uid:0, item:left})
                    }
                    else if stack_top.counter == 1{
                        stack_top.counter +=1;
                        Some(DockStackLevel{counter:0, uid:0, item:right})
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
            return self.handle_walk(cx, event);
        }
        else if self.stack.len() > 0{
            self.stack.pop();
            return self.handle_walk(cx, event);
        }
        return None;
    }

    pub fn draw_walk(&'a mut self, cx: &mut Cx)->Option<&'a mut TItem>{
        // lets get the current item on the stack
         let push_or_pop = if let Some(stack_top) = self.stack.last_mut(){
            // return item 'count'
            match stack_top.item{
                DockItem::Single(item)=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        return Some(item);
                    }
                    else{
                        None
                    }
                },
                DockItem::Tabbed{current_tab, tabs}=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        return Some(&mut tabs[*current_tab]);
                    }
                    else{
                        None
                    }
                },
                DockItem::Split{split_mode, split_pos, axis,  left, right}=>{
                    if stack_top.counter == 0{
                        stack_top.counter += 1;
                        stack_top.uid = self.walk_uid;
                        self.walk_uid += 1;
                        // begin a split
                        let split = self.splitters.get(cx, stack_top.uid);
                        
                        split.begin_splitter(cx, split_mode.clone(), *split_pos, axis.clone());
                        Some(DockStackLevel{counter:0, uid:0, item:left})
                    }
                    else if stack_top.counter == 1{
                        stack_top.counter +=1 ;

                        let split = self.splitters.get(cx, stack_top.uid);
                        split.mid_splitter(cx);
                        Some(DockStackLevel{counter:0, uid:0, item:right})
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
            return self.draw_walk(cx);
        }
        else if self.stack.len() > 0{
            self.stack.pop();
            return self.draw_walk(cx);
        }
        None
    }
}

impl<TItem, TSplitter> Dock<TItem, TSplitter>
where TItem: Clone,
      TSplitter: Clone + ElementLife + SplitterLike
{
    pub fn walker<'a>(&'a mut self)->DockWalker<'a, TItem, TSplitter>{
        let mut stack = Vec::new();
        stack.push(DockStackLevel{counter:0, uid:0, item:&mut self.item_root});
        DockWalker{
            walk_uid:0,
            splitters:&mut self.splitters,
            stack:stack
        }
    }
}
