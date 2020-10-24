pub mod skybox;
pub mod worldview;
pub mod treeworld;
pub mod fieldworld;

use makepad_render::*;
pub fn set_worlds_style(cx:&mut Cx){
    crate::skybox::SkyBox::style(cx);
    crate::worldview::WorldView::style(cx);
    crate::treeworld::TreeWorld::style(cx);
}
