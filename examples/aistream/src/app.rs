

Todo = {done:false}
Todos = [
    Todo{text:"Design Splash"}
    Todo{text:"Implement it"}
]

Ui = View{
    PortalList{
        Button{}
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
                    on_click: ||{
                        delete Todos[item]
                    }
                }
            }
        }
    }
    
    View{
        flow: Right
        _new_item: TextInput{}
        Button{
            text: "Add"
            on_click: ||{
                Todos += Todo{
                    text: up._new_item.text
                }
                up._new_item.text = ""
            }
        }
        Button{
            text: "Clear completed"
            on_click: ||{
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
