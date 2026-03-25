use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let PaneToolbar = RectView {
        width: Fill
        height: 36.0
        flow: Right
        align: Align {x: 0.0 y: 0.5}
        padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        spacing: theme.space_2
        draw_bg +: {
            color: theme.color_bg_container
        }
    }

    let FileTreePane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            file_tree_filter := TextInputFlat {
                width: Fill
                empty_text: "Filter"
            }
        }
        file_tree := DesktopFileTree {}
    }

    let CodeEditorPane = View {
        width: Fill
        height: Fill
        flow: Down
        code_editor := DesktopCodeEditor {}
    }

    let EditorFirstPane = RectView {
        draw_bg +: {
            color: theme.color_bg_container
        }
    }

    let RunListPane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            run_stop_all := ButtonFlat {text: "Stop All"}
        }
        run_list := DesktopRunList {}
    }

    let RunningAppPane = View {
        width: Fill
        height: Fill
        flow: Down
        run_view := DesktopRunView {}
    }

    let RunFirstPane = RectView {
        draw_bg +: {
            color: theme.color_bg_container
        }
        View {
            width: Fill
            height: Fill
            align: Align {x: 0.5 y: 0.5}
            placeholder := Label {
                text: "Click play in Run to launch"
                draw_text.color: theme.color_label_outer
            }
        }
    }

    let LogPane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            log_tail_toggle := Toggle {
                text: "Tail"
                active: true
            }
            Filler {}
            log_filter := TextInputFlat {
                width: 200.0
                empty_text: "Filter"
            }
            clear_log_filter := ButtonFlatter {
                text: "x"
                padding: Inset {left: 4.0 right: 4.0 top: 0.0 bottom: 0.0}
            }
            clear_log := ButtonFlatter {
                text: "Clear"
            }
            log_open_profiler := ButtonFlatterIcon {
                width: 24.0
                height: 24.0
                icon_walk: Walk {width: 14.0 height: 14.0}
                draw_icon +: {
                    color: theme.color_label_outer
                    svg: crate_resource("self://resources/icons/icon_profiler.svg")
                }
            }
        }
        log_view := DesktopLogView {}
    }

    let ProfilerPane = View {
        width: Fill
        height: Fill
        flow: Down
        profiler_view := DesktopProfilerView {}
    }

    let LogFirstPane = LogPane {}

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

    let TerminalShellPane = View {
        width: Fill
        height: Fill
        terminal_tabs := DockTabs {
            tabs: [@terminal_first @terminal_add]
            selected: 0
            closable: true
        }
    }

    let CaptionSidebarToggle = ButtonFlatterIcon {
        width: 38.0
        height: 30.0
        icon_walk: Walk {width: 17.0 height: 17.0}
        draw_bg +: {
            color: #x4C4C4C
            color_hover: #x5C5C5C
            color_down: #x3F3F3F
            border_radius: 5.0
        }
        draw_icon +: {
            color: #xD6D6D6
            svg: crate_resource("self://resources/icons/icon_sidebar_toggle.svg")
        }
    }

    let CaptionPanelToggle = ButtonFlatterIcon {
        width: 38.0
        height: 30.0
        icon_walk: Walk {width: 17.0 height: 17.0}
        draw_bg +: {
            color: #x4C4C4C
            color_hover: #x5C5C5C
            color_down: #x3F3F3F
            border_radius: 5.0
        }
        draw_icon +: {
            color: #xD6D6D6
            svg: crate_resource("self://resources/icons/icon_panel_toggle.svg")
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
        caption_bar := SolidView {
            visible: true
            height: 38.0
            flow: Right
            align: Align {x: 0.0 y: 0.5}
            draw_bg.color: theme.color_bg_app

            left_controls := View {
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                margin: Inset {left: 88.0 right: 0.0 top: 0.0 bottom: 0.0}

                sidebar_toggle := CaptionSidebarToggle {}
            }

            caption_label := View {
                width: Fill
                height: Fill
                align: Center
                label := Label {text: "Makepad"}
            }

            right_caption_tools := View {
                width: Fit
                height: Fit
                flow: Right
                spacing: theme.space_2
                margin: Inset {left: 0.0 right: 112.0 top: 0.0 bottom: 0.0}

                bottom_panel_toggle := CaptionPanelToggle {}
                voice_wave := VoiceWave {
                    width: Fit
                    height: Fit
                }
            }

            windows_buttons := View {
                visible: false
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                min := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsMin width: 46 height: 29}
                max := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsMax width: 46 height: 29}
                close := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsClose width: 46 height: 29}
            }

            web_fullscreen := View {
                visible: false
                width: Fit
                height: Fit
                align: Align {x: 0.0 y: 0.5}
                margin: Inset {left: 0.0 right: 8.0 top: 0.0 bottom: 0.0}
                fullscreen := DesktopButton {draw_bg.button_type: DesktopButtonType.Fullscreen width: 50 height: 36}
            }
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
            padding: Inset {left: 10.0 right: 10.0 top: 2.0 bottom: 10.0}

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
                            b: @bottom_panel_tabs
                        }

                        editor_split := DockSplitter {
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.Weighted(0.62)
                            a: @editor_tabs
                            b: @run_tabs
                        }

                        bottom_panel_tabs := DockTabs {
                            tabs: [@log_first @bottom_terminal_tab]
                            selected: 0
                            closable: false
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
                            kind: @EditorFirstPane
                        }

                        run_first := DockTab {
                            name: ""
                            template: @RunFirstTab
                            kind: @RunFirstPane
                        }

                        log_first := DockTab {
                            name: "Logs"
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

                        bottom_terminal_tab := DockTab {
                            name: "Terminal"
                            template: @TerminalTab
                            kind: @TerminalShellPane
                        }

                        FileTreePane := FileTreePane {}
                        RunListPane := RunListPane {}
                        CodeEditorPane := CodeEditorPane {}
                        EditorFirstPane := EditorFirstPane {}
                        RunningAppPane := RunningAppPane {}
                        RunFirstPane := RunFirstPane {}
                        LogFirstPane := LogFirstPane {}
                        LogPane := LogPane {}
                        ProfilerPane := ProfilerPane {}
                        TerminalFirstPane := TerminalFirstPane {}
                        TerminalShellPane := TerminalShellPane {}
                        TerminalPane := TerminalPane {}
                        TerminalAddPane := View {}
                    }
                }
            }
        }
    }
}
