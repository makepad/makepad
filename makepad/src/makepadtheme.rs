use render::*;
use crate::filetree::*;
use crate::loglist::*;
use crate::homepage::*;
use widget::*;

pub fn set_makepad_theme(cx: &mut Cx, opt:&ThemeOptions) {
    HomePage::theme(cx, opt);
    FileTree::theme(cx, opt);
    LogList::theme(cx, opt);
}