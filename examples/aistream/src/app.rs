
use makepad_widgets::*;

// The new ai_stream DSL which can be executed streaming
ai_stream!{
    // A Todo list example
        
    // In AiStream we have a global namespace of Name = value
    // We also don't import any UI components from the makepad component library,
    // the namespace is always global and has forward referencing.
        
    // define the Todo base object
    Todo = {done:false}
        
    // Todo{ } does prototypical inheritance from Todo and adds fields
    Todos = [Todo{text:"Item1"}, Todo{text:"Item1"},Todo{text:"itemr"}]
    
    // the scripting language is very similar to Javascript
    // and entirely dynamically typed

    // global functions are defined like so 
    fn globalfn(){
    }
         
    // Normal makepad DSL but without the < > around UI components
    Ui = View{
        list = PortalList{
            // callbacks can be defined as such (like a JS method on an object)
            // on draw item is called for each item, item is a usize
            // just like in JS methods dont have a this/self argument
            // the script syntax is entirely untyped and highly similar to javascript
            on_draw_item(item){
                                                
                // store the items UI structure on 'self'
                .[item] = View{
                    // whilst this code is being generated
                    // the UI can already update as each UI element pops into view
                    input = TextInput{
                        // read the text value from the DATA array
                        text: Todos[item].text
                        on_change(value){
                            Todos[item].text = value
                            // use of the starting- . operator like swift
                            .text = value
                        }
                    }
                    done = CheckBox{
                        check: DATA[item].done
                        on_change(value){
                            TODOS[item].done = value
                        }
                    }
                    delete = Button{
                        text: "delete"
                        on_click(){
                            // in AiStream you can't remove items from an array
                            // but you can mark them with the 'deleted' tombstone value
                            Todos[item] = deleted
                        }
                    }
                }
            }
        }
        // Creating new todo items
        extra = View{
            flow: Right
            // UI components are given a name/id with the assignment = operator
            new_item = TextInput{}
            add = Button{
                text: Add
                on_click(){
                    // += on an array adds a new item
                    Todos += Todo{text: ..new_item.text}
                    ..new_item.text = ""
                }
            }
        }
    }
    
    // Aistream can append to existing Ui structures like so
    Ui.extra += Button{
    }
    
    // AiStream can 'stream' generate items such as new data
    Todos += Todo{text:"Whilst this text is being generated the UI can update"}
}