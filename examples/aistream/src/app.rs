
on_draw_item: |item:string, item:string|{
    let x:item = 
}


Todos = [
    Todo{text:"Design Splash"}
    Todo{text:"Implement it"}
]

// decide the variable resolution algorithm.
// we have 3 sources
// one is the let scope chain
// one is the object tree
// one is function arguments
    // ONLY when explicit. implicit ones are done using args object
// how do these things relate.


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
