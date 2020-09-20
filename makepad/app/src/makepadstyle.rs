use makepad_render::*;

use crate::filetree::*;
use crate::loglist::*;
use crate::homepage::*;
use crate::codeicon::*;
use crate::searchresults::*;
use crate::itemdisplay::*;
use crate::livemacro::*;
use crate::colorpicker::*;
use crate::floatslider::*;
use crate::shaderview::*;

pub fn set_makepad_style(cx: &mut Cx) {
    CodeIcon::style(cx);
    HomePage::style(cx);
    FileTree::style(cx);
    LogList::style(cx);
    SearchResults::style(cx);
    ItemDisplay::style(cx);
    ColorPicker::style(cx);
    FloatSlider::style(cx);
    LiveMacrosView::style(cx);
    ShaderView::style(cx);
}
