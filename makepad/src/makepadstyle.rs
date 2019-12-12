use makepad_render::*;
use makepad_widget::*;
use crate::filetree::*;
use crate::loglist::*;
use crate::homepage::*;
use crate::codeicon::*;

pub fn set_makepad_style(cx: &mut Cx, opt:&StyleOptions) {
    CodeIcon::style(cx, opt);
    HomePage::style(cx, opt);
    FileTree::style(cx, opt);
    LogList::style(cx, opt);
}
