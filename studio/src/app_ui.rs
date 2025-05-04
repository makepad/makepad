use crate::makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::widgets::*;
    use link::shaders::*;
    use makepad_widgets::designer_theme::*;
    use makepad_studio::studio_editor::StudioCodeEditor;
    use makepad_studio::ai_chat::ai_chat_view::AiChatView;
    use makepad_studio::studio_file_tree::StudioFileTree;
    use makepad_studio::run_view::RunView;
    use makepad_studio::log_list::LogList;
    use makepad_studio::run_list::RunList;
    use makepad_studio::profiler::Profiler;
    use makepad_studio::search::Search;
    use makepad_studio::snapshot::Snapshot;
    
    ICO_SEARCH = dep("crate://self/resources/icons/Icon_Search.svg")

    Logo = <Button> {
        draw_icon: {
            svg_file: dep("crate://self/resources/logo_makepad.svg"),
            fn get_color(self) -> vec4 {
                return (THEME_COLOR_D_1)
            }
        }
        text:""
        icon_walk: {width: 250.0, height: Fit}
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
    }

    IconTab = <TabFlat> {
        closeable: false
        icon_walk: { width: 15., height: 15. }
    }

    Vr = <View> {
        width: Fit, height: 27.,
        flow: Right,
        spacing: 0.,
        margin: <THEME_MSPACE_V_2> {}
        <View> {
            width: (THEME_BEVELING * 2.0), height: Fill
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_OUTSET_2) }
        }
        <View> {
            width: (THEME_BEVELING), height: Fill,
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_OUTSET_1) }
        }
    }

    DockSettings = <View> {
        align: { x: 0., y: 0. }
        spacing: (THEME_SPACE_2)
        <Filler> {}
        <P> {
            width: Fit,
            text: "",
            margin: 0.,
            padding: <THEME_MSPACE_1> {}
        }
        <CheckBoxCustom> {
            text:""
            // text:"Apps"
            draw_bg: { check_type: None }
            icon_walk: {width: 11., margin: {top: 1.75, right: 3. }}
            padding: { right: 0, left: 0.}
            draw_icon: {
                color: (THEME_COLOR_D_2),
                color_active: (THEME_COLOR_LABEL_OUTER_ACTIVE),
                svg_file: dep("crate://self/resources/icons/icon_tab_app.svg"),
            }
        }
        <CheckBoxCustom> {
            text:""
            // text:"Designer"
            draw_bg: { check_type: None }
            icon_walk: {width: 12., margin: {top: 1.0 }}
            padding: { right: 0, left: 0.}
            draw_icon: {
                color: (THEME_COLOR_D_2),
                color_active: (THEME_COLOR_LABEL_OUTER_ACTIVE),
                svg_file: dep("crate://self/resources/icons/icon_designer.svg"),
            }
        }
        <CheckBoxCustom> {
            text:""
            // text:"Editor"
            width: 13.
            draw_bg: { check_type: None }
            icon_walk: {width: 6., margin: {top: 0.5, left: 3. }}
            padding: { right: 0., left: 0.}
            draw_icon: {
                color: (THEME_COLOR_D_2),
                color_active: (THEME_COLOR_LABEL_OUTER_ACTIVE),
                svg_file: dep("crate://self/resources/icons/icon_editor.svg"),
            }
        }
        <CheckBoxCustom> {
            text:""
            // text:"Scene"
            draw_bg: { check_type: None }
            icon_walk: {width: 11.5, margin: {top: 1.5 } }
            padding: { right: 5., left: 0.}
            draw_icon: {
                color: (THEME_COLOR_D_2),
                color_active: (THEME_COLOR_LABEL_OUTER_ACTIVE),
                svg_file: dep("crate://self/resources/icons/icon_outliner.svg"),
            }
        }
    }

    pub AppUI =  <Window> {
        margin: 5.
        caption_bar = { margin: {left: -100}, visible: true, caption_label = {label = {text: "Makepad"}} 
        <View>{
            width: Fit,
            padding:{top:0,right:7}
            preset_1 = <Button>{text:"A"}
            preset_2 = <Button>{text:"C"}
            preset_3 = <Button>{text:"D"}
            preset_4 = <Button>{text:"P"}
            }
        },
        window: { inner_size: vec2(1600, 900), /*dpi_override:3.0 */},
        show_bg: true,
        draw_bg: { fn pixel(self) -> vec4 { return (THEME_COLOR_BG_APP) } }
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
        body = {
            padding:5
            dock = <DockFlat> {
                width: Fill, height: Fill,
                tab_bar:{
                    OutlineFirstTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 10.
                            margin: { top: 5. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_1)
                            svg_file: dep("crate://self/resources/icons/icon_outliner.svg"),
                        }
                    }
                    EditFirstTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 5.
                            margin: { top: 5. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_6)
                            svg_file: dep("crate://self/resources/icons/icon_editor.svg"),
                        }
                    }
                    AiFirstTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 8.
                            margin: { top: 5.5 }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_6)
                            svg_file: dep("crate://self/resources/icons/icon_auto.svg"),
                        }
                    }
                    DesignFirstTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 11.
                            margin: { top: 4. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_3)
                            svg_file: dep("crate://self/resources/icons/icon_designer.svg"),
                        }
                    }
                    FilesTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 8.5,
                            margin: { top: 4. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_2)
                            svg_file: dep("crate://self/resources/icons/icon_file.svg"),
                        }
                    }
                    RunFirstTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 11.,
                            margin: { top: 6. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_4)
                            svg_file: dep("crate://self/resources/icons/icon_tab_app.svg"),
                        }
                    }
                    RunListTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 7.
                            margin: { top: 5. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_5)
                            svg_file: dep("crate://self/resources/icons/icon_run.svg"),
                        }
                    }
                    SnapshotTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 7.
                            margin: { top: 5. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_5)
                            svg_file: dep("crate://self/resources/icons/icon_run.svg"),
                        }
                    }
                    LogTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk:{
                            width: 9.5
                            margin: { top: 7. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_2)
                            svg_file: dep("crate://self/resources/icons/icon_log.svg"),
                        }
                    }
                    ProfilerTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 9.
                            margin: { top: 4. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_7)
                            svg_file: dep("crate://self/resources/icons/icon_profiler.svg"),
                        }
                    }
                    SearchTab = <IconTab> {
                        spacing: (THEME_SPACE_2)
                        icon_walk: {
                            width: 10.5,
                            margin: { top: 4. }
                        }
                        draw_icon: {
                            color: (STUDIO_PALETTE_3)
                            svg_file: dep("crate://self/resources/icons/icon_search.svg"),
                        }
                    }
                }
                root = Splitter {
                    axis: Horizontal,
                    align: FromA(250.0),
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
                    b: split3
                }
                split3 = Splitter {
                    axis: Horizontal,
                    align: FromA(20),
                    a: split4,
                    b: run_tabs
                }
                split4 = Splitter {
                    axis: Horizontal,
                    align: Weighted(0.5),
                    a: outline_tabs,
                    b: design_tabs
                }
                /*
                split3 = Splitter {
                    axis: Horizontal,
                    align: Weighted(0.5),
                    a: design_tabs,
                    b: run_tabs
                }*/
    
                file_tree_tabs = Tabs {
                    tabs: [file_tree_tab, run_list_tab, search, snapshot_tab],
                    selected: 0
                }
    
                edit_tabs = Tabs {
                    tabs: [edit_first],
                    selected: 0
                }
                
                design_tabs = Tabs {
                    tabs: [design_first],
                    selected: 0
                }
                
                outline_tabs = Tabs {
                    tabs: [outline_first],
                    selected: 0
                }
                
                log_tabs = Tabs {
                    tabs: [log_list_tab, profiler],
                    selected: 0
                }
    
                run_tabs = Tabs {
                    tabs: [run_first,ai_first],
                    selected: 0
                }
                /*
                design_tabs = Tabs {
                    tabs: [design_first],
                    selected: 0
                }*/
    
                file_tree_tab = Tab {
                    name: "Files",
                    template: FilesTab,
                    kind: StudioFileTree
                }
    
                search = Tab {
                    name: "Search"
                    template: SearchTab,
                    kind: Search
                }
    
                run_first = Tab {
                    name: ""
                    template: RunFirstTab,
                    kind: RunFirst
                }
    
                design_first = Tab {
                    name: ""
                    template: DesignFirstTab,
                    kind: DesignFirst
                }
    
                edit_first = Tab {
                    name: ""
                    template: EditFirstTab,
                    kind: EditFirst
                }
                ai_first = Tab {
                    name: ""
                    template: AiFirstTab,
                    kind: AiFirst
                }
                outline_first = Tab {
                    name: ""
                    template: OutlineFirstTab,
                    kind: OutlineFirst
                }
    
                run_list_tab = Tab {
                    name: "Run"
                    template: RunListTab,
                    kind: RunList
                }
                snapshot_tab = Tab {
                    name: "Snapshot"
                    template: SnapshotTab,
                    kind: Snapshot
                }
                log_list_tab = Tab {
                    name: "Log",
                    template: LogTab,
                    kind: LogList
                }
    
                profiler = Tab {
                    name: "Profiler",
                    template: ProfilerTab,
                    kind: Profiler
                }
    
                CodeEditor = <View> {
                    flow: Down,
                    <DockToolbar> {
                        content = {
                            height: Fit, width: Fill,
                            spacing: (THEME_SPACE_1)
                            flow: Right,
                            margin: {left: (THEME_SPACE_1), right: (THEME_SPACE_1) },
    
                            <ButtonFlatter> { width: Fit, text: "File"}
                            <ButtonFlatter> { width: Fit, text: "Edit"}
                            <ButtonFlatter> { width: Fit, text: "Search"}
                            <ButtonFlatter> { width: Fit, text: "Debug"}
                            <Filler> {}
                            <LinkLabel> { width: Fit, text: "Docs", url: "https://publish.obsidian.md/makepad-docs"}
                        }
                    }
                    editor = <StudioCodeEditor> {} 
                }
                
                AiChat = <AiChatView> {
                    flow: Down,
                }
                
                EditFirst = <RectView> {
                    draw_bg: {color: (THEME_COLOR_BG_CONTAINER)}
                    <View> {
                        width: Fill, height: Fill,
                        align: { x: 0., y: 0. }
                        flow: Down
                        <DockToolbar> { content = <DockSettings> {} }
                        <View> {
                            width: Fill, height: Fill,
                            align: { x: 0.5, y: 0.5 }
                            <Logo> {}
                        }
                        
                        // <H3> {
                        //     width: Fit,
                        //     text: "Welcome to \nMakepad \n\n欢迎来到\nMakepad"
                        //     margin: {left: 185}
                        // }
                    }
                }
                OutlineFirst = <RectView> {
                    draw_bg: {color: (THEME_COLOR_BG_CONTAINER)}
                    <View> {
                        width: Fill, height: Fill,
                        align: { x: 0.5, y: 0.5 }
                        flow: Down
                        <DockToolbar> { content = <DockSettings> {} }
                        <View> {
                            width: Fill, height: Fill,
                            align: { x: 0.5, y: 0.5 }
                            <Logo> {}
                        }
                    }
                }
                DesignFirst = <RectView> {
                    draw_bg: {color: (THEME_COLOR_BG_CONTAINER)}
                    <View> {
                        width: Fill, height: Fill
                        flow: Down
                        <DockToolbar> { content = <DockSettings> {} }
                        <View> {
                            width: Fill, height: Fill,
                            align: { x: 0.5, y: 0.5 }
                            <Logo> {}
                        }
                    }
                }
                AiFirst = <RectView> {
                    draw_bg: {color: (THEME_COLOR_BG_CONTAINER)}
                    <View> {
                        width: Fill, height: Fill
                        flow: Down
                        <DockToolbar> { content = <DockSettings> {} }
                        <View> {
                            width: Fill, height: Fill,
                            align: { x: 0.5, y: 0.5 }
                            <Logo> {}
                        }
                    }
                }
                RunFirst = <RectView> {
                    draw_bg: {color: (THEME_COLOR_BG_CONTAINER)}
                    <View> {
                        width: Fill, height: Fill,
                        flow: Down
                        <DockToolbar> { content = <DockSettings> {} }
                        <View> {
                            width: Fill, height: Fill,
                            align: { x: 0.5, y: 0.5 }
                            <Logo> {}
                        }
                    }
                }
                RunList = <View> {
                    flow: Down,
                    margin: 0.,
                    padding: 0.,
                    <DockToolbar> {
                        content = {
                            <Pbold> {
                                width: Fit,
                                text: "Types",
                                margin: 0.,
                                padding: <THEME_MSPACE_1> {}
                            }
                            <Toggle> { text: "Release", }
                            <Toggle> { text: "Debug"}
                        }
                    }
                    <RunList> {}
                }
                Snapshot = <Snapshot> {}
                Search = <Search> {}
                RunView = <RunView> {}
                StudioFileTree = <View> {
                    flow: Down,
                    <DockToolbar> {
                        content = {
                            <TextInput> {
                                width: Fill,
                                empty_text: "Filter",
                            }
                        }
                    }
                    file_tree = <StudioFileTree> {}
                }
                LogList = <View> {
                    flow: Down,
                    <DockToolbar> {
                        content = {
                            align: { x: 0., y: 0.5 }
                            <View> {
                                width: Fit
                                flow: Right,
                                spacing: (THEME_SPACE_1)
                                <CheckBoxCustom> {
                                    margin: {left: (THEME_SPACE_1)}
                                    text:"Error"
                                    align: { y: 0.5 }
                                    draw_bg: { check_type: None }
                                    spacing: (THEME_SPACE_1),
                                    icon_walk: {width: 7.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_2),
                                        color_active: (STUDIO_PALETTE_4),
                                        svg_file: dep("crate://self/resources/icons/icon_log_bullet.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    text:"Warning"
                                    align: { y: 0.5 }
                                    draw_bg: { check_type: None }
                                    spacing: (THEME_SPACE_1),
                                    icon_walk: {width: 7.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_2),
                                        color_active: (STUDIO_PALETTE_1),
                                        svg_file: dep("crate://self/resources/icons/icon_log_bullet.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    text:"Log"
                                    align: { y: 0.5 }
                                    draw_bg: { check_type: None }
                                    spacing: (THEME_SPACE_1),
                                    icon_walk: {width: 7.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_2),
                                        color_active: (THEME_COLOR_U_5),
                                        svg_file: dep("crate://self/resources/icons/icon_log_bullet.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    text:"Wait"
                                    align: { y: 0.5 }
                                    draw_bg: { check_type: None }
                                    spacing: (THEME_SPACE_1),
                                    icon_walk: {width: 7.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_2),
                                        color_active: (STUDIO_PALETTE_2),
                                        svg_file: dep("crate://self/resources/icons/icon_log_bullet.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    text:"Panic"
                                    align: { y: 0.5 }
                                    draw_bg: { check_type: None }
                                    spacing: (THEME_SPACE_1),
                                    icon_walk: {width: 7.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_2),
                                        color_active: (STUDIO_PALETTE_5),
                                        svg_file: dep("crate://self/resources/icons/icon_log_bullet.svg"),
                                    }
                                }
                            }
                            // <Vr> {}
                            <Filler> {}
                            <TextInput> {
                                width: 200.
                                empty_text: "Filter",
                            }
                        }
                    }
                    log_list = <LogList> {}
                }
                Profiler = <Profiler> {
                    flow: Down,
                }
            }
        }
    }
}