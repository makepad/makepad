#![allow(dead_code)]
use crate::area::Area;

pub struct NavList{
    pub items: Vec<NavItem>
}

pub enum NavOrder{
    Top(u64),
    Middle(u64),
    Bottom(u64),
}

pub enum NavItem{
    Child(usize),
    Stop(NavStop)
}

pub struct NavStop{
    pub role: NavRole,
    pub order: NavOrder,
    pub area: Area
}

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