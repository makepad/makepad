let Todo = {done:false}

let todos = [
    Todo{text: "Design Splash"}
    Todo{text: "Implement it"}
]

let sum = |a,b|{
    return a+b
}

let sum = |a,b|{
    return a+b
}

text_style = {}

Ui = View{
    PortalList{
        Button{}
        on_draw_item: |item|{
            View{
                TextInput{
                    .draw_text.text_style.font_size += 1.0
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

let req = http.fetch("https://todolistservice" + 10)
let json = req.result.parse_json()
for todo in json{
    Todos += Todo{
        text: todo.text
        done: todo.done
    }
}
Todos += Todo{text: "Whilst this text is being generated the UI can update"}