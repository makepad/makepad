use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let FileTreePane = View {
        width: Fill
        height: Fill
        flow: Down
        file_tree := DesktopFileTree {}
    }

    let CodeEditorPane = View {
        width: Fill
        height: Fill
        flow: Down
        code_editor := DesktopCodeEditor {}
    }

    let RunListPane = View {
        width: Fill
        height: Fill
        flow: Down
        run_list := DesktopRunList {}
    }

    let RunningAppPane = View {
        width: Fill
        height: Fill
        flow: Down
        run_view := DesktopRunView {}
    }

    let LogPane = View {
        width: Fill
        height: Fill
        flow: Down
        log_view := DesktopLogView {}
    }

    let LogFirstPane = RectView {
        draw_bg +: {
            color: theme.color_bg_container
        }
    }

    let TerminalPane = View {
        width: Fill
        height: Fill
        flow: Down
        terminal_view := DesktopTerminalView {}
    }

    let TerminalFirstPane = RectView {
        draw_bg +: {
            color: theme.color_bg_container
        }
        View {
            width: Fill
            height: Fill
            align: Align {x: 0.5 y: 0.5}
            placeholder := Label {
                text: "Terminal press + to add a terminal"
                draw_text.color: theme.color_label_outer
            }
        }
    }

    let STUDIO_PALETTE_1 = #B2FF64
    let STUDIO_PALETTE_2 = #80FFBF
    let STUDIO_PALETTE_3 = #80BFFF
    let STUDIO_PALETTE_4 = #BF80FF
    let STUDIO_PALETTE_5 = #FF80BF
    let STUDIO_PALETTE_6 = #FFB368

    let IconTab = TabFlat {
        closeable: false
        spacing: theme.space_1
        icon_walk: Walk {width: Fit height: 16.0}
    }

    let MountTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_3
            svg: crate_resource("self://resources/icons/icon_tab_app.svg")
        }
    }

    let FilesTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_2
            svg: crate_resource("self://resources/icons/icon_file.svg")
        }
    }

    let RunListTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_5
            svg: crate_resource("self://resources/icons/icon_run.svg")
        }
    }

    let EditorFirstTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_6
            svg: crate_resource("self://resources/icons/icon_editor.svg")
        }
    }

    let EditorTab = EditorFirstTab {closeable: true}

    let RunFirstTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_4
            svg: crate_resource("self://resources/icons/icon_tab_app.svg")
        }
    }

    let RunAppTab = RunFirstTab {closeable: true}

    let LogFirstTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_2
            svg: crate_resource("self://resources/icons/icon_log.svg")
        }
    }

    let LogTab = LogFirstTab {closeable: true}

    let TerminalTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_2
            svg: crate_resource("self://resources/icons/icon_terminal.svg")
        }
    }

    mod.widgets.AppUI = Window {
        window.inner_size: vec2(1400 900)
        caption_bar +: {
            visible: false
            height: 0.0
        }
        draw_bg +: {
            pixel: fn() {
                return theme.color_bg_app
            }
        }

        body +: {
            width: Fill
            height: Fill
            flow: Down
            spacing: 0.0
            padding: 0.0

            RoundedView {
                visible: false
                width: Fill
                height: Fit
                flow: Right
                spacing: theme.space_2
                padding: Inset {left: 10.0 right: 10.0 top: 6.0 bottom: 6.0}
                draw_bg.color: #x1B2332
                draw_bg.border_radius: 6.0

                status_label := Label {
                    width: Fit
                    text: "Starting backend..."
                    draw_text.color: #xD5E4FF
                }
                Filler {}
                current_file_label := Label {
                    width: Fit
                    text: "No file"
                    draw_text.color: #x89A0C7
                }
            }

            mount_dock := DockFlat {
                width: Fill
                height: Fill

                tab_bar +: {
                    MountTab := MountTab {}
                }

                root := DockTabs {
                    tabs: [@mount_first]
                    selected: 0
                    closable: false
                }

                mount_first := DockTab {
                    name: "makepad"
                    template: @MountTab
                    kind: @MountWorkspace
                }

                MountWorkspace := View {
                    width: Fill
                    height: Fill

                    dock := DockFlat {
                        width: Fill
                        height: Fill

                        tab_bar +: {
                            FilesTab := FilesTab {}
                            RunListTab := RunListTab {}
                            EditorFirstTab := EditorFirstTab {}
                            EditorTab := EditorTab {}
                            RunFirstTab := RunFirstTab {}
                            RunAppTab := RunAppTab {}
                            LogFirstTab := LogFirstTab {}
                            LogTab := LogTab {}
                            TerminalTab := TerminalTab {}
                        }

                        root := DockSplitter {
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.FromA(310.0)
                            a: @tree_tabs
                            b: @main_split
                        }

                        main_split := DockSplitter {
                            axis: SplitterAxis.Vertical
                            align: SplitterAlign.FromB(220.0)
                            a: @editor_split
                            b: @bottom_split
                        }

                        editor_split := DockSplitter {
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.Weighted(0.62)
                            a: @editor_tabs
                            b: @run_tabs
                        }

                        bottom_split := DockSplitter {
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.Weighted(0.5)
                            a: @log_tabs
                            b: @terminal_tabs
                        }

                        tree_tabs := DockTabs {
                            tabs: [@tree_tab @run_list_tab]
                            selected: 0
                            closable: false
                        }

                        editor_tabs := DockTabs {
                            tabs: [@editor_first]
                            selected: 0
                            closable: true
                        }

                        run_tabs := DockTabs {
                            tabs: [@run_first]
                            selected: 0
                            closable: true
                        }

                        log_tabs := DockTabs {
                            tabs: [@log_first]
                            selected: 0
                            closable: true
                        }

                        terminal_tabs := DockTabs {
                            tabs: [@terminal_first @terminal_add]
                            selected: 0
                            closable: true
                        }

                        tree_tab := DockTab {
                            name: "Files"
                            template: @FilesTab
                            kind: @FileTreePane
                        }

                        run_list_tab := DockTab {
                            name: "Run"
                            template: @RunListTab
                            kind: @RunListPane
                        }

                        editor_first := DockTab {
                            name: ""
                            template: @EditorFirstTab
                            kind: @CodeEditorPane
                        }

                        run_first := DockTab {
                            name: ""
                            template: @RunFirstTab
                            kind: @RunningAppPane
                        }

                        log_first := DockTab {
                            name: ""
                            template: @LogFirstTab
                            kind: @LogFirstPane
                        }

                        terminal_first := DockTab {
                            name: ""
                            template: @TerminalTab
                            kind: @TerminalFirstPane
                        }

                        terminal_add := DockTab {
                            name: "+"
                            template: @PermanentTab
                            kind: @TerminalAddPane
                        }

                        FileTreePane := FileTreePane {}
                        RunListPane := RunListPane {}
                        CodeEditorPane := CodeEditorPane {}
                        RunningAppPane := RunningAppPane {}
                        LogFirstPane := LogFirstPane {}
                        LogPane := LogPane {}
                        TerminalFirstPane := TerminalFirstPane {}
                        TerminalPane := TerminalPane {}
                        TerminalAddPane := View {}
                    }
                }
            }
        }
    }
}
