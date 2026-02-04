use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*  // Import studio widgets registered by other modules
    use mod.draw.KeyCode  // For menu keyboard shortcuts

    let ICO_SEARCH = crate_resource("self://resources/icons/Icon_Search.svg")

    // Local DockToolbar definition (since it's not in widgets2)
    let DockToolbar = RectShadowView{
        width: Fill
        height: 38.
        flow: Down
        align: Align{ x: 0. y: 0. }
        margin: Inset{ top: -1. }
        padding: theme.mspace_2
        spacing: 0.
        draw_bg +: {
            border_size: 0.0
            border_color: theme.color_bevel_outset_1
            shadow_color: theme.color_shadow
            shadow_radius: 7.5
            shadow_offset: vec2(0.0, 0.0)
            color: theme.color_fg_app
        }
        $content: View {
            height: Fill
            width: Fill
            flow: Right
            margin: 0.
            padding: 0.
            align: Align{ x: 0. y: 0. }
            spacing: theme.space_3
        }
    }

    let Logo = Button{
        draw_icon +: {
            svg: crate_resource("self://resources/logo_makepad.svg")
            color: theme.color_d_1
        }
        text: ""
        icon_walk: Walk{width: 250.0 height: Fit}
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }
    }

    let IconTab = TabFlat{
        closeable: false
        spacing: theme.space_1
        icon_walk: Walk{width: Fit height: 18.}
    }

    let Vr = View{
        width: Fit height: 27.
        flow: Flow.Right
        spacing: 0.
        margin: theme.mspace_v_2
        View{
            width: theme.beveling * 2.0 height: Fill
            show_bg: true
            draw_bg +: {color: theme.color_bevel_outset_2}
        }
        View{
            width: theme.beveling height: Fill
            show_bg: true
            draw_bg +: {color: theme.color_bevel_outset_1}
        }
    }

    let DockSettings = View{
        align: Align{x: 0. y: 0.}
        spacing: theme.space_2
        Filler{}
        P{
            width: Fit
            text: ""
            margin: 0.
            padding: theme.mspace_1
        }
        CheckBoxCustom{
            text: ""
            icon_walk: Walk{width: 11. margin: Inset{top: 1.75 right: 3.}}
            padding: Inset{right: 0 left: 0.}
            draw_icon +: {
                color: theme.color_label_outer
                color_active: theme.color_label_outer_active
                svg: crate_resource("self://resources/icons/icon_tab_app.svg")
            }
        }
        CheckBoxCustom{
            text: ""
            icon_walk: Walk{width: 12. margin: Inset{top: 1.0}}
            padding: Inset{right: 0 left: 0.}
            draw_icon +: {
                color: theme.color_label_outer
                color_active: theme.color_label_outer_active
                svg: crate_resource("self://resources/icons/icon_designer.svg")
            }
        }
        CheckBoxCustom{
            text: ""
            width: 13.
            icon_walk: Walk{width: 6. margin: Inset{top: 0.5 left: 3.}}
            padding: Inset{right: 0. left: 0.}
            draw_icon +: {
                color: theme.color_label_outer
                color_active: theme.color_label_outer_active
                svg: crate_resource("self://resources/icons/icon_editor.svg")
            }
        }
        CheckBoxCustom{
            text: ""
            icon_walk: Walk{width: 11.5 margin: Inset{top: 1.5}}
            padding: Inset{right: 5. left: 0.}
            draw_icon +: {
                color: theme.color_label_outer
                color_active: theme.color_label_outer_active
                svg: crate_resource("self://resources/icons/icon_outliner.svg")
            }
        }
    }

    // Studio palette colors
    let STUDIO_PALETTE_1 = #B2FF64
    let STUDIO_PALETTE_2 = #80FFBF
    let STUDIO_PALETTE_3 = #80BFFF
    let STUDIO_PALETTE_4 = #BF80FF
    let STUDIO_PALETTE_5 = #FF80BF
    let STUDIO_PALETTE_6 = #FFB368
    let STUDIO_PALETTE_7 = #FFD864

    let OutlineFirstTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_1
            svg: crate_resource("self://resources/icons/icon_outliner.svg")
        }
    }

    let EditFirstTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_6
            svg: crate_resource("self://resources/icons/icon_editor.svg")
        }
    }

    let AiFirstTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_6
            svg: crate_resource("self://resources/icons/icon_auto.svg")
        }
    }

    let DesignFirstTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_3
            svg: crate_resource("self://resources/icons/icon_designer.svg")
        }
    }

    let FilesTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_2
            svg: crate_resource("self://resources/icons/icon_file.svg")
        }
    }

    let RunFirstTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_4
            svg: crate_resource("self://resources/icons/icon_tab_app.svg")
        }
    }

    let RunListTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_5
            svg: crate_resource("self://resources/icons/icon_run.svg")
        }
    }

    let SnapshotTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_5
            svg: crate_resource("self://resources/icons/icon_run.svg")
        }
    }

    let LogTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_2
            svg: crate_resource("self://resources/icons/icon_log.svg")
        }
    }

    let ProfilerTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_7
            svg: crate_resource("self://resources/icons/icon_profiler.svg")
        }
    }

    let SearchTab = IconTab{
        draw_icon +: {
            color: STUDIO_PALETTE_3
            svg: crate_resource("self://resources/icons/icon_search.svg")
        }
    }

    // Content templates for dock
    let CodeEditorContent = View{
        flow: Flow.Down
        DockToolbar{
            $content +: {
                height: Fit width: Fill
                spacing: theme.space_1
                flow: Flow.Right
                margin: Inset{left: theme.space_1 right: theme.space_1}

                ButtonFlatter{width: Fit text: "File"}
                ButtonFlatter{width: Fit text: "Edit"}
                ButtonFlatter{width: Fit text: "Search"}
                ButtonFlatter{width: Fit text: "Debug"}
                Filler{}
                LinkLabel{width: Fit text: "Docs" url: "https://publish.obsidian.md/makepad-docs"}
            }
        }
        $editor: StudioCodeEditor{}
    }

    let AiChatContent = AiChatView{
        flow: Flow.Down
    }

    let EditFirstContent = RectView{
        draw_bg +: {color: theme.color_bg_container}
        View{
            width: Fill height: Fill
            align: Align{x: 0. y: 0.}
            flow: Flow.Down
            DockToolbar{$content: DockSettings{}}
            View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                Logo{}
            }
        }
    }

    let OutlineFirstContent = RectView{
        draw_bg +: {color: theme.color_bg_container}
        View{
            width: Fill height: Fill
            align: Align{x: 0.5 y: 0.5}
            flow: Flow.Down
            DockToolbar{$content: DockSettings{}}
            View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                Logo{}
            }
        }
    }

    let DesignFirstContent = RectView{
        draw_bg +: {color: theme.color_bg_container}
        View{
            width: Fill height: Fill
            flow: Flow.Down
            DockToolbar{$content: DockSettings{}}
            View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                Logo{}
            }
        }
    }

    let AiFirstContent = RectView{
        draw_bg +: {color: theme.color_bg_container}
        View{
            width: Fill height: Fill
            flow: Flow.Down
            DockToolbar{$content: DockSettings{}}
            View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                Logo{}
            }
        }
    }

    let RunFirstContent = RectView{
        draw_bg +: {color: theme.color_bg_container}
        View{
            width: Fill height: Fill
            flow: Flow.Down
            DockToolbar{
                $content +: {
                    Pbold{
                        width: Fit
                        text: "Run"
                        margin: 0.
                        padding: theme.mspace_1
                    }
                }
            }
            View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                Logo{}
            }
        }
    }

    let RunListContent = View{
        flow: Flow.Down
        margin: 0.
        padding: 0.
        DockToolbar{
            $content +: {
                $stop_all: ButtonFlat{text: "Stop All"}
            }
        }
        $run_list: RunList{}
    }

    let SnapshotContent = Snapshot{}
    let SearchContent = Search{}
    let RunViewContent = RunView{}

    let StudioFileTreeContent = View{
        flow: Flow.Down
        DockToolbar{
            $content +: {
                TextInputFlat{
                    width: Fill
                    empty_text: "Filter"
                }
            }
        }
        $file_tree: StudioFileTree{}
    }

    let LogListContent = View{
        flow: Flow.Down
        DockToolbar{
            $content +: {
                align: Align{x: 0. y: 0.5}
                View{
                    width: Fit
                    flow: Flow.Right
                    CheckBoxCustom{
                        text: "Error"
                        align: Align{y: 0.5}

                        icon_walk: Walk{width: 7.}
                        draw_icon +: {
                            color: theme.color_label_outer
                            color_active: STUDIO_PALETTE_4
                            svg: crate_resource("self://resources/icons/icon_log_bullet.svg")
                        }
                    }
                    CheckBoxCustom{
                        text: "Warning"
                        align: Align{y: 0.5}

                        icon_walk: Walk{width: 7.}
                        draw_icon +: {
                            color: theme.color_label_outer
                            color_active: STUDIO_PALETTE_1
                            svg: crate_resource("self://resources/icons/icon_log_bullet.svg")
                        }
                    }
                    CheckBoxCustom{
                        text: "Log"
                        align: Align{y: 0.5}

                        icon_walk: Walk{width: 7.}
                        draw_icon +: {
                            color: theme.color_label_outer
                            color_active: theme.color_u_5
                            svg: crate_resource("self://resources/icons/icon_log_bullet.svg")
                        }
                    }
                    CheckBoxCustom{
                        text: "Wait"
                        align: Align{y: 0.5}

                        icon_walk: Walk{width: 7.}
                        draw_icon +: {
                            color: theme.color_label_outer
                            color_active: STUDIO_PALETTE_2
                            svg: crate_resource("self://resources/icons/icon_log_bullet.svg")
                        }
                    }
                    CheckBoxCustom{
                        text: "Panic"
                        align: Align{y: 0.5}

                        icon_walk: Walk{width: 7.}
                        draw_icon +: {
                            color: theme.color_label_outer
                            color_active: STUDIO_PALETTE_5
                            svg: crate_resource("self://resources/icons/icon_log_bullet.svg")
                        }
                    }
                }
                Filler{}
                TextInputFlat{
                    width: 200.
                    empty_text: "Filter"
                }
            }
        }
        $log_list: LogList{}
    }

    let ProfilerContent = Profiler{
        flow: Flow.Down
    }

    mod.widgets.AppUI = Window{
        margin: 5.
        $caption_bar +: {
            margin: Inset{top: 2 left: -190}
            visible: true
            $caption_label +: {$label +: {text: "Makepad"}}
        }
        window.inner_size: vec2(2600 1900)
        show_bg: true
        draw_bg +: {
            pixel: fn() {
                return theme.color_bg_app
            }
        }
        $window_menu +: {
            $main: MenuItem.Main{items: [@app @file @edit @selection @view @run @window @help]}

            $app: MenuItem.Sub{name: "Makepad Studio" items: [@about @line @settings @line @quit]}
            $about: MenuItem.Item{name: "About Makepad Studio" enabled: false}
            $settings: MenuItem.Item{name: "Settings" enabled: false}
            $quit: MenuItem.Item{name: "Quit Makepad Studio" key: KeyCode.KeyQ}

            $file: MenuItem.Sub{name: "File" items: [@new_file @new_window @line @save_as @line @rename @line @close_editor @close_window]}
            $new_file: MenuItem.Item{name: "New File" enabled: false shift: true key: KeyCode.KeyN}
            $new_window: MenuItem.Item{name: "New Window" enabled: false shift: true key: KeyCode.KeyN}
            $save_as: MenuItem.Item{name: "Save As" enabled: false}
            $rename: MenuItem.Item{name: "Rename" enabled: false}
            $close_editor: MenuItem.Item{name: "Close Editor" enabled: false}
            $close_window: MenuItem.Item{name: "Close Window" enabled: false}

            $edit: MenuItem.Sub{name: "Edit" items: [@undo @redo @line @cut @copy @paste @line @find @replace @line @find_in_files @replace_in_files]}
            $undo: MenuItem.Item{name: "Undo" enabled: false}
            $redo: MenuItem.Item{name: "Redo" enabled: false}
            $cut: MenuItem.Item{name: "Cut" enabled: false}
            $copy: MenuItem.Item{name: "Copy" enabled: false}
            $paste: MenuItem.Item{name: "Paste" enabled: false}
            $find: MenuItem.Item{name: "Find" enabled: false}
            $replace: MenuItem.Item{name: "Replace" enabled: false}
            $find_in_files: MenuItem.Item{name: "Find in Files" enabled: false}
            $replace_in_files: MenuItem.Item{name: "Replace in Files" enabled: false}

            $selection: MenuItem.Sub{name: "Selection" items: [@select_all]}
            $select_all: MenuItem.Item{name: "Select All" enabled: false}

            $view: MenuItem.Sub{name: "View" items: [@zoom_in @zoom_out @fullscreen]}
            $zoom_in: MenuItem.Item{name: "Zoom In" enabled: false}
            $zoom_out: MenuItem.Item{name: "Zoom Out" enabled: false}
            $fullscreen: MenuItem.Item{name: "Enter Full Screen" enabled: false}

            $run: MenuItem.Sub{name: "Run" items: [@run_program]}
            $run_program: MenuItem.Item{name: "Run Program" enabled: false}

            $window: MenuItem.Sub{name: "Window" items: [@minimize @zoom @line @all_to_front]}
            $minimize: MenuItem.Item{name: "Minimize" enabled: false}
            $zoom: MenuItem.Item{name: "Zoom" enabled: false}
            $all_to_front: MenuItem.Item{name: "Bring All to Front" enabled: false}

            $help: MenuItem.Sub{name: "Help" items: [@about]}

            $line: MenuItem.Line
        }
        $body +: {
            padding: 5
            $dock: DockFlat{
                width: Fill height: Fill

                tab_bar +: {
                    $OutlineFirstTab: OutlineFirstTab{}
                    $EditFirstTab: EditFirstTab{}
                    $AiFirstTab: AiFirstTab{}
                    $AiChatTab: AiFirstTab{}
                    $DesignFirstTab: DesignFirstTab{}
                    $FilesTab: FilesTab{}
                    $RunFirstTab: RunFirstTab{}
                    $RunListTab: RunListTab{}
                    $SnapshotTab: SnapshotTab{}
                    $LogTab: LogTab{}
                    $ProfilerTab: ProfilerTab{}
                    $SearchTab: SearchTab{}
                }

                $root: DockSplitter{
                    axis: SplitterAxis.Horizontal
                    align: SplitterAlign.FromA(250.0)
                    a: $file_tree_tabs
                    b: $split1
                }

                $split1: DockSplitter{
                    axis: SplitterAxis.Vertical
                    align: SplitterAlign.FromB(200.0)
                    a: $edit_tabs
                    b: $log_tabs
                }

                $file_tree_tabs: DockTabs{
                    tabs: [$file_tree_tab $run_list_tab]
                    selected: 0
                }

                $edit_tabs: DockTabs{
                    tabs: [$run_first $design_first $outline_first $ai_first $edit_first]
                    selected: 0
                }

                $log_tabs: DockTabs{
                    tabs: [$log_list_tab]
                    selected: 0
                }

                $file_tree_tab: DockTab{
                    name: "Files"
                    template: $FilesTab
                    kind: $StudioFileTree
                }

                $edit_first: DockTab{
                    name: ""
                    template: $EditFirstTab
                    kind: $EditFirst
                }

                $log_list_tab: DockTab{
                    name: "Log"
                    template: $LogTab
                    kind: $LogList
                }

                $search: DockTab{
                    name: "Search"
                    template: $SearchTab
                    kind: $Search
                }

                $run_first: DockTab{
                    name: ""
                    template: $RunFirstTab
                    kind: $RunFirst
                }

                $design_first: DockTab{
                    name: ""
                    template: $DesignFirstTab
                    kind: $DesignFirst
                }

                $ai_first: DockTab{
                    name: ""
                    template: $AiFirstTab
                    kind: $AiFirst
                }

                $ai_chat_tab: DockTab{
                    name: "AI Chat"
                    template: $AiChatTab
                    kind: $AiChat
                }

                $outline_first: DockTab{
                    name: ""
                    template: $OutlineFirstTab
                    kind: $OutlineFirst
                }

                $run_list_tab: DockTab{
                    name: "Run"
                    template: $RunListTab
                    kind: $RunList
                }

                $snapshot_tab: DockTab{
                    name: "Snapshot"
                    template: $SnapshotTab
                    kind: $Snapshot
                }

                $profiler: DockTab{
                    name: "Profiler"
                    template: $ProfilerTab
                    kind: $Profiler
                }

                // Content templates (kind)
                $CodeEditor: CodeEditorContent{}
                $AiChat: AiChatContent{}
                $EditFirst: EditFirstContent{}
                $OutlineFirst: OutlineFirstContent{}
                $DesignFirst: DesignFirstContent{}
                $AiFirst: AiFirstContent{}
                $RunFirst: RunFirstContent{}
                $RunList: RunListContent{}
                $Snapshot: SnapshotContent{}
                $Search: SearchContent{}
                $RunView: RunViewContent{}
                $StudioFileTree: StudioFileTreeContent{}
                $LogList: LogListContent{}
                $Profiler: ProfilerContent{}
            }
        }
    }
}
