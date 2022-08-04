#![allow(dead_code)]
use crate::area::Area;
use crate::draw_list::DrawListId;
use crate::draw_2d::turtle::Margin;
use crate::cx::Cx;
use crate::makepad_error_log::*;

#[derive(Debug)]
pub struct NavList{
    pub items: Vec<NavItem>
}

#[derive(Debug)]
pub enum NavOrder{
    Default,
    Top(u64),
    Middle(u64),
    Bottom(u64),
}

#[derive(Debug)]
pub struct NavStop{
    pub role: NavRole,
    pub order: NavOrder,
    pub margin: Margin,
    pub area: Area
}


#[derive(Debug)]
pub enum NavItem{
    Child(DrawListId),
    Stop(NavStop)
}

#[derive(Debug)]
pub enum NavRole{
    ScrollBar,
    Slider,
    SpinButton,
    Switch,
    Tab,
    TabPanel,
    TreeItem,
    TextInput,
    Label,
    ComboBox,
    Menu,
    MenuBar,
    TabList,
    Tree,
    TreeGrid
}

impl Cx{
    pub fn iterate_nav_stops<F>(&self, root:DrawListId, mut callback:F)->Option<Area> where F:FnMut(&Cx, &NavStop)->Option<Area>{
        fn iterate_nav_stops<F>(cx:&Cx, draw_list_id:DrawListId, callback:&mut F)->Option<Area> where F:FnMut(&Cx, &NavStop)->Option<Area>{
            
            for i in 0..cx.draw_lists[draw_list_id].nav_items.len(){
                let nav_item = &cx.draw_lists[draw_list_id].nav_items[i];
                match nav_item{
                    NavItem::Child(draw_list_id)=>{
                        if let Some(area) = iterate_nav_stops(cx, *draw_list_id, callback){
                            return Some(area)
                        }
                    }
                    NavItem::Stop(stop)=>if let Some(area) = callback(cx, stop){
                        return Some(area)
                    }
                }
            }
            None
        }
        iterate_nav_stops(self, root, &mut callback)
    }
}