#![allow(dead_code)]
use crate::area::Area;
use crate::draw_list::DrawListId;
use crate::draw_2d::turtle::Margin;

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
pub enum NavItem{
    Child(DrawListId),
    Stop{
        role: NavRole,
        order: NavOrder,
        margin: Margin,
        area: Area
    }
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