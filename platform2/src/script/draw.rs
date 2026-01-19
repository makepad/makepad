
use crate::*;
use makepad_script::*;
use makepad_script::id;
use crate::draw_list::DrawCallUniforms;
use crate::draw_list::DrawListUniforms;
use crate::draw_pass::DrawPassUniforms;

pub fn define_draw_module(vm:&mut ScriptVm){
    let draw = vm.new_module(id!(draw));
    script_api!(vm, draw, DrawCallUniforms);
    script_api!(vm, draw, DrawListUniforms);
    script_api!(vm, draw, DrawPassUniforms);
    // alright so we need a 'struct' for geometry_quad
    
    // alright render. lets put some basics in here
    // we need the draw_call_uniforms
    // draw_pass
    // and draw_list    
}
