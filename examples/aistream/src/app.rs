scope.import(mod.widgets)
scope.import(mod.math)
let net = mod.net;

let Todo = {done:false}

let todos = [
    Todo{text: "Design Splash"}
    Todo{text: "Implement it"}
]

let main_ui = View{
    PortalList{
        on_draw_item: |item|{
            View{
                TextInput{
                    text: todos[item].text
                }
                CheckBox{
                    check: todos[item].done
                }
                Button{
                    text: "delete"
                    on_click: ||{
                        delete todos[item]
                    }
                }
            }
        }
    }
    view: View{
        flow: Right
        $new_item: TextInput{}
        Button{
            text: "Add",
            on_click: ||{
                todos += Todo{
                    text: $new_item.text
                }
                $new_item.text = ""
            }
        }
        Button{
            text: "Clear completed",
            on_click: ||{
                todos.retain(|v| !v.done)
            }
        }
    }
}
ui.set_view(main_ui) 

let req = net.fetch("https://todolistservice" + 10)
let json = res.parse_json()
for todo in json{
    Todos += Todo{
        text: todo.text
        done: todo.done
    }
}
Todos += Todo{text: "Whilst this text is being generated the UI can update"}
