use crate::makepad_widgets::*;

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_studio::studio_editor::StudioEditor;
    import makepad_studio::studio_file_tree::StudioFileTree;
    import makepad_studio::run_view::RunView;
    import makepad_studio::log_list::LogList;
    import makepad_studio::run_list::RunList;
    import makepad_studio::profiler::Profiler;
    
    ICO_SEARCH = dep("crate://self/resources/icons/Icon_Search.svg")

    Logo = <Button> {
        draw_icon: {
            svg_file: dep("crate://self/resources/logo_makepad.svg"),
            fn get_color(self) -> vec4 {
                return #xffffff
            }
        }
        icon_walk: {width: 300.0, height: Fit}
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        margin: {top: 20.0, right: 0.0, bottom: 30.0, left: 0.0}
        padding: 0.0
        text: ""
    }
    
    AppUI =  <Window> {
        caption_bar = { margin: {left: -100}, visible: true, caption_label = {label = {text: "Makepad Studio"}} },
        window: {inner_size: vec2(1600, 900)},
        window_menu = {
            main = Main {items: [app, file, edit, selection, view, run, window, help]}
                
            app = Sub {name: "Makepad Studio", items: [about, line, settings, line, quit]}
            about = Item {name: "About Makepad Studio", enabled: false}
            settings = Item {name: "Settings", enabled: false}
            quit = Item {name: "Quit Makepad Studio", key: KeyQ}
                
            file = Sub {name: "File", items: [new_file, new_window, line, save_as, line, rename, line, close_editor, close_window]}
            new_file = Item {name: "New File", enabled: false, shift: true, key: KeyN}
            new_window = Item {name: "New Window", enabled: false, shift: true, key: KeyN}
            save_as = Item {name: "Save As", enabled: false}
            rename = Item {name: "Rename", enabled: false}
            close_editor = Item {name: "Close Editor", enabled: false}
            close_window = Item {name: "Close Window", enabled: false}
                
            edit = Sub {name: "Edit", items: [undo, redo, line, cut, copy, paste, line, find, replace, line, find_in_files, replace_in_files]}
            undo = Item {name: "Undo", enabled: false}
            redo = Item {name: "Redo", enabled: false}
            cut = Item {name: "Cut", enabled: false}
            copy = Item {name: "Copy", enabled: false}
            paste = Item {name: "Paste", enabled: false}
            find = Item {name: "Find", enabled: false}
            replace = Item {name: "Replace", enabled: false}
            find_in_files = Item {name: "Find in Files", enabled: false}
            replace_in_files = Item {name: "Replace in Files", enabled: false}
                
            selection = Sub {name: "Selection", items: [select_all]}
            select_all = Item {name: "Select All", enabled: false}
                
            view = Sub {name: "View", items: [select_all]}
            zoom_in = Item {name: "Zoom In", enabled: false}
            zoom_out = Item {name: "Zoom Out", enabled: false}
            select_all = Item {name: "Enter Full Screen", enabled: false}
                
            run = Sub {name: "Run", items: [run_program]}
            run_program = Item {name: "Run Program", enabled: false}
                
            window = Sub {name: "Window", items: [minimize, zoom, line, all_to_front]}
            minimize = Item {name: "Minimize", enabled: false}
            zoom = Item {name: "Zoom", enabled: false}
            all_to_front = Item {name: "Bring All to Front", enabled: false}
                
            help = Sub {name: "Help", items: [about]}
                
            line = Line,
        }
        body = {dock = <Dock> {
            height: Fill,
            width: Fill
                
            root = Splitter {
                axis: Horizontal,
                align: FromA(230.0),
                a: file_tree_tabs,
                b: split1
            }
                
            split1 = Splitter {
                axis: Vertical,
                align: FromB(200.0),
                a: split2,
                b: log_tabs
            }
                
            split2 = Splitter {
                axis: Horizontal,
                align: Weighted(0.5),
                a: edit_tabs,
                b: run_tabs
            }
                
            file_tree_tabs = Tabs {
                tabs: [file_tree, search, run_list],
                selected: 2
            }
                
            edit_tabs = Tabs {
                tabs: [edit_first],
                selected: 0
            }
                
            log_tabs = Tabs {
                tabs: [log_list, profiler],
                selected: 1
            }
                
            run_tabs = Tabs {
                tabs: [run_first],
                selected: 0
            }
                
            file_tree = Tab {
                name: "Explore",
                closable: false,
                kind: StudioFileTree
            }
                
            search = Tab {
                name: "Search"
                closable: false,
                kind: Search
            }
                
            run_first = Tab {
                name: "View"
                closable: false,
                kind: RunFirst
            }
                
            edit_first = Tab {
                name: "Edit"
                closable: false,
                kind: EditFirst
            }
                
            run_list = Tab {
                name: "Run"
                closable: false,
                kind: RunList
            }
                
            file1 = Tab {
                name: "app.rs",
                closable: true,
                kind: StudioEditor
            }
                
            log_list = Tab {
                name: "Log",
                closable: false,
                kind: LogList
            }
            
            profiler = Tab {
                name: "Profiler",
                closable: false,
                kind: Profiler
            }
                
            StudioEditor = <StudioEditor> {}
            EditFirst = <RectView> {
                draw_bg: {color: #052329}
                <View> {
                    width: Fill,
                    height: Fill
                    align: {
                        x: 0.5,
                        y: 0.5
                    }
                    flow: Down
                    <Logo> {}
                    <Label> {
                        text: "Welcome to \nMakepad \n\n欢迎来到\nMakepad"
                        width: Fit,
                        margin: {left: 200}
                        draw_text: {
                            text_style: {
                                font_size: 20.0,
                                height_factor: 1.0,
                                font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
                            },
                        }
                    }
                }
            }
            RunFirst = <RectView> {
                draw_bg: {color: #4}
                <View> {
                    width: Fill,
                    height: Fill
                    align: {
                        x: 0.5,
                        y: 0.5
                    }
                    flow: Down
                        <Logo> {
                        draw_icon: {
                            fn get_color(self) -> vec4 {
                                return #7
                            }
                        }
                    }
                }
            }
            RunList = <RunList> {
            }
            Search = <RectView> {
                draw_bg: {color: #x28}
                //  margin:{left: 0, top: 0}
                <View> {
                    margin:10
                    flow: Down
                    <View> 
                    {
                        flow: Right
                        height: Fit
                        <TextInput>{
                            draw_bg: {
                                fn pixel(self) -> vec4 {
                                    return #x00000044
                                }
                            }
                            width: Fill,
                            empty_message:"Search here"                           
                        }
                        //panic = <IconButton> {draw_icon: {svg_file: (ICO_PANIC)} icon_walk: {width: Fit, height: 17.0}, margin: {left: 5.0, right: -10.0}}
                    // <Button> {
                    //     text:"Search"
                            
                    //     draw_icon: {
                    //         svg_file: (ICO_SEARCH)
                    //         fn get_color(self) -> vec4 {
                    //             return #8;
                    //         }
                    //     } 
                    //     icon_walk:  {
                    //         margin: {left: 10}
                    //         width: Fit,
                    //         // height: 12.0
                    //     } 
                    // }
                }
                    <Label>
                    {
                        text: "this does not work yet."
                    }
                }
            }
            RunView = <RunView> {}
            StudioFileTree = <StudioFileTree> {}
            LogList = <LogList> {}
            Profiler = <Profiler> {}
        }}
    }
}
