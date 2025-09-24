
Todo = {done:false}

Instructions

num.const 1
num.const 2
add
1 + 2

a.b = 1
ident a
ident b
propaccess
num.const 1
assign

ident on_draw_item
closure_with_args 1
ident item
begin_block
end_block
assign

// assign sees on_draw_item on stack
// and a closure on stack 
// then it assigns it


on_draw_item: |item|{
}


Todos = [
    Todo{text:"Design Splash"}
    Todo{text:"Implement it"}
]

Ui = View{
    PortalList{
        Button{} // niet nil/undefined/null
        on_draw_item: |item|{
            View{
                TextInput{
                    text: Todos[item].text
                }
                CheckBox{
                    check: Todos[item].done
                }
                Button{
                    text: "delete"
                    on_click: {
                        delete Todos[item]
                    }
                }
            }
        }
    }
    
    View{
        flow: Right
        _new_item = TextInput{}
        Button{
            text: "Add"
            on_click: {
                Todos += Todo{
                    text: _new_item.text
                }
                _new_item.text = ""
            }
        }
        Button{
            text: "Clear completed"
            on_click: {
                delete Todos[?@.done]
            }
        }
    }
}

let req = http.fetch("https://todolistservice" + 10)
let json = req.result.parse_json()
for todo in json{
    Todos += Todo{
        text: todo.text
        done: todo.done
    }
}
Todos += Todo{text:"Whilst this text is being generated the UI can update"}
