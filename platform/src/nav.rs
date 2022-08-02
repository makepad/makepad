#![allow(dead_code)]
use crate::area::Area;

#[derive(Debug)]
pub struct NavList{
    pub items: Vec<NavItem>
}

#[derive(Debug)]
pub enum NavOrder{
    Top(u64),
    Middle(u64),
    Bottom(u64),
}

#[derive(Debug)]
pub enum NavItem{
    Child(usize),
    Stop(NavStop)
}

#[derive(Debug)]
pub struct NavStop{
    pub role: NavRole,
    pub order: NavOrder,
    pub area: Area
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