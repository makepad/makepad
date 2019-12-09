use makepad_render::*;
use crate::filetree::*;
use crate::loglist::*;
use crate::homepage::*;
use widget::*;

pub fn set_makepad_style(cx: &mut Cx, opt:&StyleOptions) {
    HomePage::style(cx, opt);
    FileTree::style(cx, opt);
    LogList::style(cx, opt);
}
