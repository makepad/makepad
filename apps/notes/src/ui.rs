//! Composition of the Notes redesign. All hit controls are plain Views.
use crate::view::NotesView;
use makepad_widgets::*;
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let Plain = View{width: Fill height: Fill padding: 0 spacing: 0 show_bg: false}
    let Paper = Plain{show_bg: true draw_bg.color: theme.color_bg_app clip_x: true clip_y: true}
    let Ink = Label{padding: 0 max_lines: 1 text_overflow: TextOverflow.Ellipsis draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 10.5}}}
    let Meta = NotesMeta{}
    let Tap = NotesTap{}
    let IconTap = Tap{glyph := Icon{width: 20 height: 20 icon_walk: Walk{width: 20 height: 20} draw_icon.color: theme.color_focus}}
    let TextTap = Tap{width: Fit{min: 44} flow: Right padding: Inset{left: 8 right: 8} caption := Ink{draw_text.color: theme.color_focus draw_text.text_style: theme.font_bold{font_size: 10.5}}}
    let Back = Tap{width: Fit flow: Right padding: Inset{left: 8 right: 8} spacing: 4 nav_name: "Back"
        glyph := Icon{width: 16 height: 20 draw_icon +: {color: theme.color_focus svg: crate_resource("self:resources/icons/back.svg")}}
        caption := Ink{draw_text +: {color: theme.color_focus text_style: theme.font_bold{font_size: 10.5}}}
    }
    let Compose = IconTap{nav_name: "New Note" glyph +: {width: 22 height: 22 icon_walk: Walk{width: 22 height: 22} draw_icon.svg: crate_resource("self:resources/icons/compose.svg")}}
    let Checklist = IconTap{nav_name: "Checklist" glyph.draw_icon.svg: crate_resource("self:resources/icons/checklist.svg")}
    let Undo = IconTap{nav_name: "Undo" glyph.draw_icon.svg: crate_resource("self:resources/icons/undo.svg")}
    let Redo = IconTap{nav_name: "Redo" glyph.draw_icon.svg: crate_resource("self:resources/icons/redo.svg")}
    let More = IconTap{nav_name: "More" glyph.draw_icon.svg: crate_resource("self:resources/icons/more.svg")}
    let Rule = NotesRule{}
    let Search = RoundedView{
        width: Fill height: 44 flow: Right padding: Inset{left: 12 right: 4} align: Align{y: 0.5}
        draw_bg +: {color: theme.color_inset border_size: 0 border_radius: 11 color_2: vec4(-1)}
        magnifier := Icon{width: 18 height: 18 icon_walk: Walk{width: 18 height: 18} draw_icon +: {color: theme.color_text_meta svg: crate_resource("self:resources/icons/search.svg")}}
        field := TextInput{
            width: Fill height: Fill margin: 0 padding: Inset{left: 8 right: 4} empty_text: "Search Notes"
            draw_bg +: {pixel: fn(){return vec4(0.0)}}
            draw_text +: {color: theme.color_text color_empty: theme.color_text_meta text_style: theme.font_regular{font_size: 10.5}}
        }
        clear := IconTap{visible: false nav_name: "Clear search" glyph.draw_icon.svg: crate_resource("self:resources/icons/clear.svg")}
    }
    let FolderRow = Plain{
        height: 44 flow: Overlay
        selection := NotesSelection{}
        tap := Tap{
            width: Fill height: Fill flow: Right padding: Inset{left: 20 right: 20} spacing: 10
            icon := Icon{width: 18 height: 18 icon_walk: Walk{width: 18 height: 18} draw_icon +: {color: theme.color_focus svg: crate_resource("self:resources/icons/folder.svg")}}
            name := Ink{width: Fill}
            count := Meta{width: Fit draw_text.text_style.font_size: 9.75 paper: theme.color_bg_container}
            chevron := Plain{visible: false width: 12 height: 12 glyph := Icon{width: 12 height: 12 draw_icon +: {color: theme.color_text_meta svg: crate_resource("self:resources/icons/chevron.svg")}}}
        }
        rule := Rule{visible: false bottom: true margin: Inset{left: 52}}
    }
    let FolderRows = Plain{
        height: Fit flow: Down
        all := FolderRow{tap.icon.draw_icon.svg: crate_resource("self:resources/icons/all_notes.svg")}
        notes := FolderRow{}
        work := FolderRow{}
        personal := FolderRow{}
        trash := FolderRow{tap.icon.draw_icon.svg: crate_resource("self:resources/icons/trash.svg")}
    }
    let NoteRow = Plain{
        height: 80 flow: Overlay
        selection := NotesSelection{}
        tap := Tap{
            width: Fill height: Fill flow: Down align: Align{x: 0 y: 0} padding: Inset{left: 20 right: 20 top: 10 bottom: 10}
            title := Ink{width: Fill height: 20 draw_text.text_style: theme.font_bold{font_size: 12}}
            title_gap := Plain{height: 4}
            preview := Meta{width: Fill height: 18 draw_text.text_style.font_size: 9.75}
            preview_gap := Plain{height: 2}
            metadata := Plain{height: 16 flow: Right spacing: 8 date := Meta{width: Fit} folder := Meta{width: Fill}}
        }
        rule := Rule{bottom: true margin: Inset{left: 20 right: 20}}
    }
    let NoteRows = PortalList{
        width: Fill height: Fill
        Section := Meta{width: Fill height: 28 padding: Inset{left: 20 top: 8} draw_text.text_style: theme.font_bold{font_size: 9}}
        Item := NoteRow{}
        Empty := Plain{
            height: 180 flow: Down align: Align{x: 0.5 y: 0.5} spacing: 12 padding: 20
            title := Ink{width: Fill height: 28 align: Align{x: 0.5 y: 0.5} draw_text.text_style: theme.font_bold{font_size: 15}}
            explanation := Meta{width: Fill height: Fit max_lines: 0 align: Align{x: 0.5 y: 0.5} draw_text.text_style.font_size: 10.5}
            compose := TextTap{nav_name: "New Note" caption.text: "New Note"}
        }
    }
    let NotesTools = GlassPanel{
        width: Fill height: 48 flow: Right spacing: 4 padding: Inset{left: 8 right: 8 top: 2 bottom: 2}
        draw_bg +: {corner_radius: 10 fallback_color: theme.color_bg_app tint_color: theme.color_bg_app}
        format := TextTap{nav_name: "Formatting" caption.text: "Aa"}
        bold := TextTap{visible: false nav_name: "Bold" caption.text: "B" width: 44}
        italic := TextTap{visible: false nav_name: "Italic" caption.text: "I" width: 44 caption.draw_text.text_style: theme.font_bold_italic{font_size: 12.75}}
        checklist := Checklist{}
        undo := Undo{}
        redo := Redo{}
        space := Plain{}
        compose := Compose{}
    }
    let CompactFolders = Paper{
        flow: Down
        nav := Plain{height: 56}
        scroll := ScrollYView{
            width: Fill height: Fill flow: Down
            title := Ink{width: Fill height: 52 padding: Inset{left: 20} text: "Folders" draw_text.text_style: theme.font_bold{font_size: 25.5}}
            account := Meta{width: Fill height: 36 padding: Inset{left: 20 top: 8} text: "On This Device" draw_text.text_style: theme.font_bold{font_size: 11.25}}
            group := RoundedView{width: Fill height: Fit flow: Down margin: Inset{left: 16 right: 16} draw_bg +: {color: theme.color_inset border_radius: 8 border_size: 0} rows := FolderRows{}}
        }
        dock := Plain{height: 64 flow: Right padding: Inset{left: 16 right: 16} align: Align{y: 0.5} space := Plain{} compose := Compose{}}
    }
    let CompactList = Paper{
        flow: Down
        nav := Plain{height: 56 flow: Right back := Back{caption.text: "Folders"} space := Plain{}}
        title := Ink{width: Fill height: 52 padding: Inset{left: 20} draw_text.text_style: theme.font_bold{font_size: 25.5}}
        count := Meta{width: Fill height: 24 padding: Inset{left: 20} draw_text.text_style.font_size: 9.75}
        rows := NoteRows{}
        dock := Plain{height: 72 flow: Right spacing: 12 padding: Inset{left: 16 right: 24} align: Align{y: 0.5} search := Search{height: 48 draw_bg.border_radius: 12} compose := Compose{}}
    }
    let CompactEditor = Paper{
        flow: Down
        nav := Plain{
            height: 56 flow: Right padding: Inset{left: 8 right: 8} align: Align{y: 0.5}
            back := Back{width: 184 caption +: {width: Fill draw_text.text_style: theme.font_bold{font_size: 12.75}}}
            space := Plain{}
            more := More{}
            done := TextTap{visible: false width: 60 nav_name: "Done" caption.text: "Done" caption.draw_text.text_style: theme.font_bold{font_size: 12.75}}
            restore := TextTap{visible: false nav_name: "Restore" caption.text: "Restore"}
        }
        document := NotesTextSurface{width: Fill height: Fill compact: true padding: Inset{left: 20 right: 20 top: 12 bottom: 24}}
        empty := Ink{visible: false width: Fill height: Fill align: Align{x: 0.5 y: 0.5} text: "No Notes"}
        dock := Plain{height: 64 padding: Inset{left: 16 right: 16 top: 8 bottom: 8} tools := NotesTools{format.caption.draw_text.text_style.font_size: 12.75 bold.caption.draw_text.text_style.font_size: 12.75}}
    }
    let OpaquePage = StackNavigationView{
        show_bg: true draw_bg.color: theme.color_bg_app clip_x: true clip_y: true
        header +: {visible: false height: 0 padding: 0}
        body +: {margin: 0 padding: 0}
        animator +: {slide: {
            hide: {ease: Ease.Bezier{cp0: 0.22 cp1: 1.0 cp2: 0.36 cp3: 1.0} from: {all: Play.Forward{duration: 0.26}} apply: {offset: 403.0}}
            show: {ease: Ease.Bezier{cp0: 0.22 cp1: 1.0 cp2: 0.36 cp3: 1.0} from: {all: Play.Forward{duration: 0.26}} apply: {offset: 0.0}}
        }}
    }
    mod.widgets.NotesViewBase = #(NotesView::register_widget(vm))
    mod.widgets.NotesView = set_type_default() do mod.widgets.NotesViewBase{
        width: Fill height: Fill flow: Down padding: 0 spacing: 0 show_bg: true draw_bg.color: theme.color_bg_app clip_x: true clip_y: true
        content := Plain{
            flow: Overlay
            wide := Paper{
                flow: Right
                folders := Plain{
                    width: 224 flow: Down show_bg: true draw_bg.color: theme.color_bg_container
                    header := Ink{width: Fill height: 56 padding: Inset{left: 16} align: Align{y: 0.5} text: "Folders" draw_text.text_style: theme.font_bold{font_size: 10.5}}
                    account := Meta{width: Fill height: 32 padding: Inset{left: 16} text: "On This Device" paper: theme.color_bg_container draw_text.text_style: theme.font_bold{font_size: 9}}
                    rows := FolderRows{}
                    remainder := Plain{}
                }
                divider_a := Rule{width: 1 height: Fill}
                notes_list := Paper{
                    width: 320 flow: Down
                    header := Ink{width: Fill height: 56 padding: Inset{left: 16} align: Align{y: 0.5} draw_text.text_style: theme.font_bold{font_size: 15}}
                    short_header := Plain{visible: false height: 44 flow: Right align: Align{y: 0.5} folders := IconTap{nav_name: "Folders" glyph.draw_icon.svg: crate_resource("self:resources/icons/folder.svg")} collection := Ink{width: Fill} compose := Compose{}}
                    search_slot := Plain{height: 48 padding: Inset{left: 12 right: 12 top: 2 bottom: 2} search := Search{}}
                    rows := NoteRows{}
                    count := Meta{width: Fill height: 24 align: Align{x: 0.5 y: 0.5}}
                }
                divider_b := Rule{width: 1 height: Fill}
                editor := Paper{
                    flow: Down
                    toolbar := Plain{
                        height: 56 flow: Right padding: Inset{left: 12 right: 12} align: Align{y: 0.5}
                        back := Back{visible: false caption.text: "Notes"}
                        restore := TextTap{visible: false nav_name: "Restore" caption.text: "Restore"}
                        format := TextTap{nav_name: "Formatting" caption.text: "Aa"}
                        checklist := Checklist{}
                        space := Plain{}
                        undo := Undo{}
                        redo := Redo{}
                        more := More{}
                        compose := Compose{}
                    }
                    document := NotesTextSurface{width: Fill height: Fill padding: Inset{left: 32 right: 32 top: 12 bottom: 32}}
                    empty := Plain{visible: false flow: Down align: Align{x: 0.5 y: 0.5} spacing: 12 title := Ink{text: "No Notes" draw_text.text_style: theme.font_bold{font_size: 15}} explanation := Meta{text: "Choose a note or create a new one."} compose := TextTap{nav_name: "New Note" caption.text: "New Note"}}
                }
            }
            compact_host := Plain{
                visible: false
                compact := StackNavigation{
                    width: Fill height: Fill
                    root_view := CompactFolders{}
                    list_view := OpaquePage{body +: {screen := CompactList{}}}
                    editor_view := OpaquePage{body +: {screen := CompactEditor{}}}
                }
            }
            loading := Meta{visible: false width: Fill height: Fill align: Align{x: 0.5 y: 0.5} text: "Loading"}
            overlay := Plain{
                visible: false flow: Overlay
                dismiss := Tap{width: Fill height: Fill nav_name: "Dismiss menu"}
                panel := GlassPanel{
                    width: 240 height: Fit flow: Down padding: 8 spacing: 0
                    draw_bg +: {corner_radius: 10 fallback_color: theme.color_bg_app tint_color: theme.color_bg_app}
                    heading := Ink{visible: false width: Fill height: 32}
                    rows := ScrollYView{
                        width: Fill height: Fit flow: Down
                        pin := TextTap{width: Fill nav_name: "Pin" caption.text: "Pin"}
                        move := TextTap{width: Fill nav_name: "Move" caption.text: "Move"}
                        delete := TextTap{width: Fill nav_name: "Delete" caption.text: "Delete" caption.draw_text.color: theme.color_error}
                        separator := Rule{}
                        undo := TextTap{width: Fill nav_name: "Undo" caption.text: "Undo"}
                        redo := TextTap{width: Fill nav_name: "Redo" caption.text: "Redo"}
                        notes := TextTap{visible: false width: Fill nav_name: "Notes" caption.text: "Notes" current := Plain{visible: false width: 20 height: 20 glyph := Icon{width: 20 height: 20 draw_icon +: {color: theme.color_focus svg: crate_resource("self:resources/icons/check.svg")}}}}
                        work := TextTap{visible: false width: Fill nav_name: "Work" caption.text: "Work" current := Plain{visible: false width: 20 height: 20 glyph := Icon{width: 20 height: 20 draw_icon +: {color: theme.color_focus svg: crate_resource("self:resources/icons/check.svg")}}}}
                        personal := TextTap{visible: false width: Fill nav_name: "Personal" caption.text: "Personal" current := Plain{visible: false width: 20 height: 20 glyph := Icon{width: 20 height: 20 draw_icon +: {color: theme.color_focus svg: crate_resource("self:resources/icons/check.svg")}}}}
                        cancel := TextTap{visible: false width: Fill nav_name: "Cancel" caption.text: "Cancel"}
                    }
                    formatting := ScrollYView{
                        visible: false width: Fill height: Fit flow: Down
                        styles := Plain{height: 44 flow: Right
                            style_title := TextTap{width: Fill nav_name: "Title" caption.text: "Title"}
                            style_heading := TextTap{width: Fill nav_name: "Heading" caption.text: "Heading"}
                            style_body := TextTap{width: Fill nav_name: "Body" caption.text: "Body"}
                        }
                        marks := Plain{height: 44 flow: Right
                            bold := TextTap{width: Fill nav_name: "Bold" caption.text: "Bold"}
                            italic := TextTap{width: Fill nav_name: "Italic" caption.text: "Italic"}
                            bullets := IconTap{width: Fill nav_name: "Bullets" glyph.draw_icon.svg: crate_resource("self:resources/icons/bullets.svg")}
                            numbers := IconTap{width: Fill nav_name: "Numbers" glyph.draw_icon.svg: crate_resource("self:resources/icons/numbers.svg")}
                        }
                    }
                    folders := ScrollYView{visible: false width: Fill height: Fit flow: Down rows := FolderRows{}}
                }
            }
        }
        status := Paper{visible: false height: 44 flow: Right align: Align{y: 0.5} padding: Inset{left: 16 right: 16}
            status_text := Ink{width: Fill draw_text.color: theme.color_error}
            retry := TextTap{nav_name: "Retry" caption.text: "Retry"}
        }
    }
}
