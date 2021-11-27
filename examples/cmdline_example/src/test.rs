OK Decisions to make:

use {
    crate::{
        id::GenId,
        id_map::GenIdMap,
        tab::{self, Tab},
    },
    makepad_render::*
    makepad_widget::*
};

+ visually sparse, easy to add items
- sparseness makes it harder to read (imho) because i have to scan 'up' the tree

OR

use crate::id::GenId;
use crate::id::GenIdMap;
use crate::tab::Tab;
use makepad_render::*;
use makepad_widget::*;

+ easy to IDE modify, can actually be less lines
- visually dense but no need to look up the tree maintaining context

------------------------------------------------------------------------

Action and button::Action
+ short, but longer with module-prefix
+ might be idiomatic rust more
- typeconfusion, makepad jump to def no work could be compensated with IDE support
- have to context switch when figuring out 'what type is Action here now?' like Error
- really uglier to read for the type-inferencing general code:

OR

ButtonAction
+ easier to read everywhere, can be included without module namespaces
- collision risk still exists, just heavily reduced
- less idiomatic Rusty
- longer to type for local use, shorter to type for outside use
- and possibly more work to rename depending on useage

Most used case:

for item in self.frame.handle_frame(cx, event) {
    if let ButtonAction::Pressed = item.action.cast() {
        println!("Clicked on button {}", item.id);
    }
}

OR

for item in self.frame.handle_frame(cx, event) {
    if let button::Action::Pressed = item.action.cast() {
        println!("Clicked on button {}", item.id);
    }
}

Related:

do we flatten all external exports on a module on lib to hide internal structure?
or do we not.
pro: less memorising submodule structure to fetch types
con: this module-namespacing idea doesnt match

------------------------------------------------------------------------
Usage of short typenames in general:

IdMap, GenId, View, Token, etc all create large collision potential
what to do

------------------------------------------------------------------------
usage of property naming. 

pub button_logic: ButtonLogic;

state: CodeEditorState

self.button_logic.handle_button_logic(...)
self.logic.handle_button_logic(...)
self.logic.handle_event(...)

handle_button(cx, event);
draw_button(cx, "Hello")
handle_event(cx, event);
draw(cx, "Hello")

self.btn_bla.draw(cx, "Hello");
self.btn_bla.draw_button(cx, "Hello");

FrameComponent{
    draw_dyn
    apply_draw_dyn
    handle_event_dyn
}

+ can really see the types without type info of IDE or hover-to-explore

OR

pub logic: button_logic::Logic;
self.logic.handle(...)

+ shorter to read 
+ but i have to overlay mentally 'what logic' and 'what handle' in my head per file 
so its context-senitive to read taking (me atleast) much more energy

------------------------------------------------------------------------

handle_button_event(possiblecustomargs)
draw_button(possiblecustomargs)

+ easy to see that these fns are not a trait
+ easy to see without IDE type info support that these fns are hard typed
+ you can read code more like a language, 'handle the button' not handle() handle what
- more work to type and visually bulkier/busier

OR

handle_event()
draw()
+ short
- can't see where the fn is from so breaks brain narrative
- not obvious these functions arent a trait interface

------------------------------------------------------------------------

The closure callback for Action. It makes the calling convention really busy/ugly
Pro: could nest things more easily.
Will decide later.

------------------------------------------------------------------------

what is as_ref for newtypes? is that the way to do it?

------------------------------------------------------------------------

remove all ::*, make macros fully namespaced
do we agree on the naming of our core crates? 
makepad_render::*? with directory render/

------------------------------------------------------------------------

