//! makepad_studio: the Studio 3 shell's UI registrations and `Cx`-free state.
//!
//! `script_mod` registers the Studio widgets (`StudioSettings`, the dock
//! tab bodies) on top of the stock widget set. The binary in `main.rs` builds
//! the window from them; the WM's module host can register the same set into
//! its own isolate later.

pub use makepad_widgets;
use makepad_widgets::*;

pub mod activity;
pub mod ai;
pub mod appearance;
pub mod canvas;
mod canvas_input;
pub mod disk;
pub mod disk_graph;
pub mod document;
pub mod document_worker;
pub mod project_tree;
pub mod state;
pub mod usage;
pub mod usage_codex;
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

    /** The Settings tab body. Only rows that work today: the appearance
     * picker (standalone; the WM owns it when hosted) and where state lives. */
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

        SectionTitle{text: "Storage"}
        storage := Card{
            Row{
                RowLabel{text: "State"}
                state_dir_label := Label{padding: 0 draw_text +: {color: theme.color_text}}
            }
            Hint{text: "Your appearance and tab layout are saved here between sessions."}
        }
    }

    /** One terminal tab: the terminal library widget, full size. */
    mod.widgets.StudioTerminalTab = View{
        width: Fill height: Fill
        term := MpTerm{}
    }

    mod.widgets.StudioDisk = ScrollYView{
        width: Fill height: Fill
        flow: Down spacing: theme.space_3 padding: theme.space_3
        show_bg: true
        draw_bg +: {color: theme.color_bg_app}
        SectionTitle{text: "Disk & workspaces"}
        Row{
            refresh_disk := Button{text: "Refresh inventory"}
            disk_scan_status := Label{padding: 0 text: "Waiting for first sample"}
        }
        Hint{text: "Inspect growing build directories and leftover workspaces before reclaiming space. The corner graph shows volume use and recent changes."}
        Row{
            disk_path := DropDown{width: Fill labels: []}
            preview_cleanup := Button{text: "Preview cleanup"}
        }
        cleanup_note := Hint{text: "Select a measured path to see its cleanup constraints."}
        disk_report := Label{
            width: Fill height: Fit padding: 0
            draw_text +: {color: theme.color_text wrap: Words}
            text: "Inspecting disk usage…"
        }
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
            refresh_usage := Button{text: "Refresh"}
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
