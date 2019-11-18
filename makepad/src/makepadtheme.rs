use render::*;
use crate::filetree::*;
use crate::loglist::*;

pub fn set_makepad_theme_values(cx: &mut Cx){

    
    FileTree::theme(cx);
    LogList::theme(cx);
}

pub fn set_dark_makepad_theme(cx: &mut Cx) {
    set_makepad_theme_values(cx);
}