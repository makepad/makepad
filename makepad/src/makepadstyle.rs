use render::*;
use crate::filetree::*;
use crate::loglist::*;
use crate::homepage::*;
use crate::codeicon::*;
use widget::*;

pub fn set_makepad_style(cx: &mut Cx, opt:&StyleOptions) {
    CodeIcon::style(cx, opt);
    HomePage::style(cx, opt);
    FileTree::style(cx, opt);
    LogList::style(cx, opt);
}