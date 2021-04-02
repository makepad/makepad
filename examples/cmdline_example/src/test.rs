let live = live_load!(crate::MyFrame)

// this object now has a clone of its DSL
// so we can write shit like
live.set_str(id!(button.label), "hello world");

for user_draw in live.draw_iter(cx){
    if user_draw.id == id!(user1){
        // draw things
    }
}

for event in live.handle_iter(cx){
    match event.id{
        id!(button1)=>{
            
        },
        id!(user1)=>{
            
        }
    }
}
