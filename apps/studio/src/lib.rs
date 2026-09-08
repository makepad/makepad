//! makepad_studio: the Studio 3 shell's UI registrations and `Cx`-free state.
//!
//! `script_mod` registers the Studio widgets (`StudioSettings`, the dock
//! tab bodies) on top of the stock widget set. The binary in `main.rs` builds
//! the window from them; the WM's module host can register the same set into
//! its own isolate later.

pub use makepad_widgets;
use makepad_widgets::*;

pub mod activity;
pub mod agent_session;
pub mod ai;
pub mod appearance;
pub mod atlas;
pub mod canvas_draw;
mod canvas_input;
pub mod disk;
pub mod disk_graph;
pub mod document;
pub mod document_worker;
pub mod history_list;
pub mod iteration;
pub mod iteration_git;
pub mod iteration_host;
pub mod iteration_host_view;
pub mod iteration_tools;
pub mod iteration_view;
pub mod iteration_worker;
pub mod mcp;
pub mod presentation;
pub mod project_tree;
pub mod state;
pub mod surface;
pub mod surface_pump;
pub mod usage;
pub mod usage_codex;
pub mod usage_history_view;
pub mod usage_stall;
pub mod workspace;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let SectionTitle = Label{
        padding: 0
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: 12.0}
        }
    }
    let Hint = Label{
        padding: 0
        width: Fill
        draw_text +: {
            color: theme.color_text_disabled
            text_style: theme.font_regular{font_size: 9.5}
        }
    }
    let Card = RoundedView{
        width: Fill height: Fit
        flow: Down spacing: theme.space_2 padding: theme.space_3
        draw_bg +: {
            color: theme.color_bg_container
            border_radius: theme.container_corner_radius
        }
    }
    let ProjIcon = RadioButtonTab{
        width: 30 height: 30 padding: 0 margin: 0 spacing: 0 align: Center
        text: ""
        icon_walk: Walk{width: 17 height: 17 margin: 0}
        label_walk: Walk{width: 0 height: 0 margin: 0}
        draw_icon +: {color: theme.color_text}
        draw_bg +: {
            border_radius: 4.0
            color_active: mix(theme.color_bg_app, theme.color_focus, 0.2)
            border_color_active: theme.color_focus
        }
    }
    let Row = View{
        width: Fill height: Fit
        flow: Right spacing: theme.space_2
        align: Align{y: 0.5}
    }
    let RowLabel = Label{
        padding: 0
        width: 120
        draw_text +: {color: theme.color_text}
    }

    /** Appearance, Architecture code density, and the saved state location. */
    mod.widgets.StudioSettings = ScrollYView{
        width: Fill height: Fill
        flow: Down spacing: theme.space_3 padding: theme.space_3
        show_bg: true
        draw_bg +: {color: theme.color_bg_app}

        SectionTitle{text: "Appearance"}
        appearance := Card{
            Row{
                RowLabel{text: "Style"}
                style_picker := DropDown{
                    width: 220
                    labels: [
                        "Follow host OS"
                        "Omarchy"
                        "macOS"
                        "Windows"
                        "Windows 2000"
                        "NeXTSTEP"
                        "iOS"
                        "Android"
                    ]
                    selected_item: 0
                }
            }
            Row{
                RowLabel{text: "Dark"}
                dark_toggle := CheckBox{text: "Use the dark variant"}
            }
            style_note := Hint{text: ""}
        }

        SectionTitle{text: "Architecture"}
        architecture := Card{
            Row{
                RowLabel{text: "Code indentation"}
                architecture_indent := DropDown{
                    width: 220 labels: ["1 cell" "2 cells" "3 cells" "4 cells" "5 cells" "6 cells" "7 cells" "8 cells"]
                    selected_item: 1
                }
            }
            Hint{text: "Cells per tab or four spaces in the map."}
        }

        SectionTitle{text: "Storage"}
        storage := Card{
            Row{
                RowLabel{text: "State"}
                state_dir_label := Label{padding: 0 draw_text +: {color: theme.color_text}}
            }
            Hint{text: "Your settings and tab layout are saved here between sessions."}
        }
    }

    /** One terminal tab: the terminal library widget, full size. */
    mod.widgets.StudioTerminalTab = View{
        width: Fill height: Fill flow: Down
        View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
            terminal_session_status := Hint{width: Fill text: "Connecting persistent terminal…"}
            Tip{text: "Running agent"
                connect_terminal := Button{width: 24 height: 22 text: ">_" padding: 0}
            }
        }
        term := MpTerm{}
    }

    mod.widgets.StudioDisk = View{
        width: Fill height: Fill
        flow: Down spacing: 0
        show_bg: true
        draw_bg +: {color: theme.color_bg_app}
        View{
            width: Fill height: Fit flow: Right spacing: theme.space_2
            padding: Inset{left: 10 right: 10 top: 6 bottom: 6}
            align: Align{y: 0.5}
            disk_scan_status := Label{padding: 0 text: "Waiting for first sample" draw_text +: {color: theme.color_text_disabled}}
            View{width: Fill height: 1}
            refresh_disk := Button{text: "Refresh" height: 28}
            disk_path := DropDown{width: 220 labels: []}
            preview_cleanup := Button{text: "Preview cleanup" height: 28}
        }
        cleanup_note := Hint{padding: Inset{left: 10 right: 10} text: "Select a measured path to see its cleanup constraints."}
        cwd_map := DiskMap{width: Fill height: Fill}
    }
    mod.widgets.StudioActivity = ScrollYView{
        width: Fill height: Fill flow: Down padding: 14
        activity_report := Label{width: Fill height: Fit text: "Observing workspace activity…" draw_text +: {wrap: Words color: theme.color_text}}
    }
    mod.widgets.StudioUsage = ScrollYView{
        width: Fill height: Fill flow: Down spacing: 14 padding: 14
        show_bg: true draw_bg +: {color: theme.color_bg_app}
        Row{
            SectionTitle{width: Fill text: "AI usage"}
        }
        usage_poll_status := Hint{text: "Checking provider limits…"}
        Card{
            SectionTitle{text: "Astra"}
            codex_usage_report := Label{
                width: Fill height: Fit padding: 0 text: "Waiting for first query"
                draw_text +: {color: theme.color_text wrap: Words}
            }
        }
        Card{
            SectionTitle{text: "Fable"}
            claude_usage_report := Label{
                width: Fill height: Fit padding: 0 text: "Waiting for first query"
                draw_text +: {color: theme.color_text wrap: Words}
            }
        }
        Hint{text: "Limits are account-wide. Background queries share your installed CLI sign-in. Missing or stale data is shown explicitly."}
    }
    mod.widgets.StudioDesign = ScrollYView{
        width: Fill height: Fill flow: Down spacing: 10 padding: 14
        design_detail := Label{width: Fill height: Fit text: "" draw_text +: {wrap: Words color: theme.color_text}}
        design_path := Label{width: Fill height: Fit text: "" draw_text +: {wrap: Words color: theme.color_text_disabled}}
    }

}
