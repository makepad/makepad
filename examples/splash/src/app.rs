use makepad_widgets::animator::Animate;
use makepad_widgets::file_tree::FileTree;
use makepad_widgets::*;
use std::path::Path;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    let TestDraw = #(TestDraw::register_widget(vm)) {
        width: 250
        height: 150
        draw_quad +: {
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos*self.rect_size)
                sdf.circle(40 40 35)
                sdf.fill(mix(#0f0 #f00 self.pos.y))
                sdf.result
            }
        }
        draw_text.color: #0f0
    }

    // ===========================================
    // SCROLLBAR TEST - Variable height items
    // ===========================================

    // Small item template (30px)
    let ScrollTestSmall = RoundedView{
        width: Fill height: 30
        margin: Inset{top: 1 bottom: 1 left: 5 right: 5}
        padding: Inset{left: 10 right: 10}
        draw_bg.color: #346
        draw_bg.radius: 3.0
        align: Align{y: 0.5}
        label := Label{text: "Small" draw_text.color: #fff draw_text.text_style.font_size: 9}
    }

    // Medium item template (60px)
    let ScrollTestMedium = RoundedView{
        width: Fill height: 60
        margin: Inset{top: 1 bottom: 1 left: 5 right: 5}
        padding: Inset{left: 10 right: 10}
        draw_bg.color: #463
        draw_bg.radius: 3.0
        align: Align{y: 0.5}
        label := Label{text: "Medium" draw_text.color: #fff draw_text.text_style.font_size: 10}
    }

    // Large item template (120px)
    let ScrollTestLarge = RoundedView{
        width: Fill height: 120
        margin: Inset{top: 1 bottom: 1 left: 5 right: 5}
        padding: Inset{left: 10 right: 10}
        draw_bg.color: #634
        draw_bg.radius: 3.0
        align: Align{y: 0.5}
        label := Label{text: "Large" draw_text.color: #fff draw_text.text_style.font_size: 11}
    }

    // Extra large item template (200px)
    let ScrollTestXLarge = RoundedView{
        width: Fill height: 200
        margin: Inset{top: 1 bottom: 1 left: 5 right: 5}
        padding: Inset{left: 10 right: 10}
        draw_bg.color: #643
        draw_bg.radius: 3.0
        align: Align{y: 0.5}
        label := Label{text: "Extra Large" draw_text.color: #fff draw_text.text_style.font_size: 12}
    }

    // Scrollbar test list widget
    let ScrollbarTestList = #(ScrollbarTestList::register_widget(vm)) {
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            Small := ScrollTestSmall{}
            Medium := ScrollTestMedium{}
            Large := ScrollTestLarge{}
            XLarge := ScrollTestXLarge{}
        }
    }

    // Tab content for scrollbar test
    let TabScrollbarTest = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Down spacing: 10

        View{
            width: Fill height: Fit
            padding: 15
            flow: Down spacing: 5
            Label{text: "Scrollbar Height Test" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "100 items with varying heights (30/60/120/200px). Scrollbar should reflect actual content size." draw_text.color: #888 draw_text.text_style.font_size: 10}
        }

        ScrollbarTestList{}
    }

    // ===========================================
    // SELECTION TEST - TextFlow in PortalList
    // ===========================================

    // Item template for the selectable TextFlow list
    let SelectableTextItem = View{
        width: Fill height: Fit
        padding: Inset{top: 4 bottom: 4 left: 10 right: 10}

        selectable := TextFlow{
            width: Fill height: Fit
            selectable: true
            font_size: 10
        }
    }

    // Widget that demonstrates cross-boundary selection in PortalList
    let SelectionTestList = #(SelectionTestList::register_widget(vm)) {
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            selectable: true
            drag_scrolling: false
            Item := SelectableTextItem{}
        }
    }

    // Tab content for selection test
    let TabSelectionTest = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Down spacing: 10

        View{
            width: Fill height: Fit
            padding: 15
            flow: Down spacing: 5
            Label{text: "Cross-Boundary Text Selection Test" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Click and drag to select text across multiple items. Use Cmd+C to copy." draw_text.color: #888 draw_text.text_style.font_size: 10}
        }

        SelectionTestList{}
    }

    // ===========================================
    // PORTAL LIST DEMO
    // ===========================================

    // Item template for the PortalList
    let ListItem = RoundedView{
        width: Fill height: Fit
        margin: Inset{top: 2 bottom: 2 left: 5 right: 5}
        padding: Inset{top: 10 bottom: 10 left: 15 right: 15}
        draw_bg.color: #445
        draw_bg.radius: 5.0
        flow: Right align: HCenter spacing: 10

        View{
            width: Fill height: Fit flow: Down spacing: 4
            title := Label{text: "Item Title" draw_text.color: #fff draw_text.text_style.font_size: 11}
            subtitle := Label{text: "Item subtitle text" draw_text.color: #888 draw_text.text_style.font_size: 9}
        }
        action_btn := ButtonFlatter{text: "View" draw_text.text_style.font_size: 9}
    }

    let ListHeader = View{
        width: Fill height: 40 padding: Inset{left: 10 right: 10} align: Align{y: 0.5}
        Label{text: "PortalList Demo" draw_text.color: #fff draw_text.text_style.font_size: 12}
    }

    let ListFooter = View{
        width: Fill height: 60 align: Center
        Label{text: "End of List" draw_text.color: #666}
    }

    // Custom NewsList widget that uses PortalList
    let NewsListTest = #(NewsListTest::register_widget(vm)) {
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            Header := ListHeader{}
            Item := ListItem{}
            Footer := ListFooter{}
        }
    }

    // ===========================================
    // TAB CONTENT TEMPLATES BY WIDGET TYPE
    // ===========================================

    // Buttons tab - all button variants
    let TabButtons = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Overlay

        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Button Variants" draw_text.color: #fff draw_text.text_style.font_size: 13}

            View{width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.5}}
            button := Button{text: "Standard"}
            flat_button := ButtonFlat{text: "Flat"}
            flatter_button := ButtonFlatter{text: "Flatter"}

            icon_button := Button{
                text: "With Icon"
                icon_walk: Walk{width: 16 height: 16}
                draw_icon.color: #fff
                draw_icon.svg: crate_resource("self:../../widgets2/resources/icons/icon_file.svg")
            }

            Hr{}

            Label{text: "Icon Only" draw_text.color: #888 draw_text.text_style.font_size: 10}
            View{width: Fill height: Fit flow: Right spacing: 15}
            test_icon := Icon{
                draw_icon.svg: crate_resource("self:../../widgets2/resources/icons/icon_file.svg")
                draw_icon.color: #0ff
                icon_walk: Walk{width: 32 height: 32}
            }
            Icon{
                draw_icon.svg: crate_resource("self:../../widgets2/resources/icons/icon_select.svg")
                draw_icon.color: #f80
                icon_walk: Walk{width: 32 height: 32}
            }

            Hr{}

            Label{text: "Tooltip Demo" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Click buttons to show tooltips, click elsewhere to hide" draw_text.color: #888 draw_text.text_style.font_size: 10}

            View{width: Fill height: Fit flow: Right spacing: 10}
            tooltip_btn1 := Button{text: "Show Tooltip 1"}
            tooltip_btn2 := Button{text: "Show Tooltip 2"}
            tooltip_btn3 := ButtonFlat{text: "Show Help Tip"}

            Hr{}

            Label{text: "Popup Notification Demo" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Click to show/hide notification popup" draw_text.color: #888 draw_text.text_style.font_size: 10}

            View{width: Fill height: Fit flow: Right spacing: 10}
            show_popup_btn := Button{text: "Show Notification"}
            hide_popup_btn := ButtonFlat{text: "Hide Notification"}
        }

        // Tooltip overlay
        buttons_tooltip := Tooltip{}

        // Popup notification overlay
        popup_notif := PopupNotification{
            align: Align{x: 1.0 y: 0.0}
            content +: {
                margin: Inset{top: 10 right: 10}

                RoundedView{
                    width: 250
                    height: Fit
                    padding: 15
                    draw_bg +: {
                        color: uniform(#2a5)
                        radius: uniform(8.0)
                    }
                    flow: Down spacing: 8

                    Label{text: "Success!" draw_text.color: #fff draw_text.text_style.font_size: 12}
                    Label{text: "Your changes have been saved successfully." draw_text.color: #dfd draw_text.text_style.font_size: 10}
                }
            }
        }
    }

    // Toggles tab - checkboxes, toggles, radio buttons
    let TabToggles = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 10

            Label{text: "Checkboxes" draw_text.color: #fff draw_text.text_style.font_size: 13}
            checkbox := CheckBox{text: "Enable feature"}
            CheckBox{text: "Show notifications"}
            CheckBox{text: "Auto-save on exit"}

            Hr{}

            Label{text: "Toggles" draw_text.color: #fff draw_text.text_style.font_size: 13}
            toggle := Toggle{text: "Dark mode"}
            Toggle{text: "Compact view"}
            Toggle{text: "Developer mode"}

            Hr{}

            Label{text: "Radio Buttons" draw_text.color: #fff draw_text.text_style.font_size: 13}
            radio1 := RadioButton{text: "Option A"}
            radio2 := RadioButton{text: "Option B"}
            radio3 := RadioButton{text: "Option C"}
        }
    }

    // Sliders tab - sliders and numeric inputs
    let TabSliders = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Sliders" draw_text.color: #fff draw_text.text_style.font_size: 13}

            slider := Slider{width: Fill text: "Volume" min: 0.0 max: 100.0 default: 50.0}
            Slider{width: Fill text: "Brightness" min: 0.0 max: 100.0 default: 75.0}
            Slider{width: Fill text: "Contrast" min: -50.0 max: 50.0 default: 0.0}
            Slider{width: Fill text: "Saturation" min: 0.0 max: 200.0 default: 100.0}

            Hr{}

            Label{text: "Fine Control" draw_text.color: #888 draw_text.text_style.font_size: 10}
            Slider{width: Fill text: "Font Size" min: 8.0 max: 24.0 default: 12.0}
            Slider{width: Fill text: "Line Height" min: 1.0 max: 3.0 default: 1.5}
        }
    }

    // Text tab - labels, headings, text inputs
    let TabText = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 10

            Label{text: "Headings" draw_text.color: #fff draw_text.text_style.font_size: 13}
            heading := H1{text: "Heading 1"}
            H2{text: "Heading 2"}
            H3{text: "Heading 3"}

            Hr{}

            Label{text: "Text Inputs" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Username:" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            username := TextInput{width: Fill height: Fit empty_text: "Enter username"}
            Label{text: "Password:" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            password := TextInput{width: Fill height: Fit empty_text: "Enter password" is_password: true}

            Hr{}

            Label{text: "Links" draw_text.color: #fff draw_text.text_style.font_size: 13}
            link := LinkLabel{text: "Visit Makepad" url: "https://makepad.dev"}
        }
    }

    // Dropdowns tab - dropdown and selection widgets
    let TabDropdowns = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Dropdown" draw_text.color: #fff draw_text.text_style.font_size: 13}
            dropdown := DropDown{labels: ["Option A" "Option B" "Option C" "Option D"]}

            Hr{}

            Label{text: "More Dropdowns" draw_text.color: #fff draw_text.text_style.font_size: 13}
            DropDown{labels: ["Small" "Medium" "Large" "Extra Large"]}
            DropDown{labels: ["Red" "Green" "Blue" "Yellow" "Purple"]}
        }
    }

    // HTML/Markdown tab
    let TabMarkup = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 15

            Label{text: "Markdown (selectable - try selecting text!)" draw_text.color: #fff draw_text.text_style.font_size: 13}
            markdown := Markdown{
                width: Fill height: Fit
                selectable: true
                body: "# Heading\n\nThis is **bold** and *italic*.\n\n- List item 1\n- List item 2\n\n> Blockquote\n\n`inline code`"
            }

            Hr{}

            Label{text: "HTML (selectable)" draw_text.color: #fff draw_text.text_style.font_size: 13}
            html := Html{
                width: Fill height: Fit
                selectable: true
                body: "<h3>HTML Content</h3><p><b>Bold</b> and <i>italic</i> text.</p><ul><li>Item one</li><li>Item two</li></ul><p><a href='https://makepad.dev'>Link</a></p>"
            }
        }
    }

    // Expandable Panel tab
    let TabExpandable = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Down

        Label{text: "Expandable Panel Demo" draw_text.color: #fff draw_text.text_style.font_size: 13 padding: 15}
        Label{text: "Drag the panel up/down" draw_text.color: #888 draw_text.text_style.font_size: 10 padding: Inset{left: 15 bottom: 10}}

        expandable := ExpandablePanel{
            width: Fill height: Fill
            initial_offset: 100.0

            // Background content (visible when panel is dragged down)
            SolidView{
                width: Fill height: Fill
                draw_bg.color: #224
                align: Center
                Label{text: "Background Content" draw_text.color: #88f draw_text.text_style.font_size: 16}
            }

            // The draggable panel
            panel := RoundedView{
                width: Fill height: Fill
                draw_bg.color: #445
                draw_bg.radius: vec4(15.0 15.0 0.0 0.0)
                flow: Down padding: 20 spacing: 10

                // Drag handle indicator
                View{
                    width: Fill height: Fit align: Center padding: Inset{bottom: 10}
                    RoundedView{
                        width: 40 height: 4
                        draw_bg.color: #666
                        draw_bg.radius: 2.0
                    }
                }

                Label{text: "Draggable Panel" draw_text.color: #fff draw_text.text_style.font_size: 14}
                Label{text: "This panel can be dragged up and down. The initial_offset property controls the starting position." draw_text.color: #aaa draw_text.text_style.font_size: 10}

                Hr{}

                Label{text: "Panel Content" draw_text.color: #fff draw_text.text_style.font_size: 12}
                CheckBox{text: "Option 1"}
                CheckBox{text: "Option 2"}
                CheckBox{text: "Option 3"}

                View{height: Fill}

                reset_btn := Button{text: "Reset Panel Position"}
            }
        }
    }

    // Fold Headers tab
    let TabFolds = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 10

            Label{text: "Fold Headers" draw_text.color: #fff draw_text.text_style.font_size: 13}

            FoldHeader{
                header: View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    padding: Inset{top: 5 bottom: 5} spacing: 8
                    FoldButton{}
                    Label{text: "Settings" draw_text.color: #fff draw_text.text_style.font_size: 11}
                }
                body: View{
                    width: Fill height: Fit flow: Down
                    padding: Inset{left: 23 top: 5 bottom: 10} spacing: 8
                    CheckBox{text: "Enable notifications"}
                    CheckBox{text: "Auto-save"}
                    Toggle{text: "Dark theme"}
                }
            }
            FoldHeader{
                header: View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    padding: Inset{top: 5 bottom: 5} spacing: 8
                    FoldButton{}
                    Label{text: "Recent Files" draw_text.color: #fff draw_text.text_style.font_size: 11}
                }
                body: View{
                    width: Fill height: Fit flow: Down
                    padding: Inset{left: 23 top: 5 bottom: 10} spacing: 5
                    Label{text: "document.txt" draw_text.color: #8af}
                    Label{text: "project.rs" draw_text.color: #8af}
                    Label{text: "config.toml" draw_text.color: #8af}
                }
            }
            FoldHeader{
                header: View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    padding: Inset{top: 5 bottom: 5} spacing: 8
                    FoldButton{}
                    Label{text: "Advanced" draw_text.color: #fff draw_text.text_style.font_size: 11}
                }
                body: View{
                    width: Fill height: Fit flow: Down
                    padding: Inset{left: 23 top: 5 bottom: 10} spacing: 8
                    Button{text: "Import..."}
                    Button{text: "Export..."}
                    Slider{width: Fill text: "Opacity" min: 0.0 max: 100.0 default: 75.0}
                }
            }
        }
    }

    // Lists tab - PortalList demo
    let TabLists = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        NewsListTest{}
    }

    // FileTree demo widget
    let FileTreeDemo = #(FileTreeDemo::register_widget(vm)){
        width: Fill
        height: Fill
        file_tree: FileTree{}
    }

    // FileTree tab - file tree demo
    let TabFileTree = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Down padding: 10 spacing: 10

        //Label{text: "FileTree Demo" draw_text.color: #fff draw_text.text_style.font_size: 13}
        //Label{text: "Displays file system hierarchy" draw_text.color: #888 draw_text.text_style.font_size: 10}
        View{
            new_batch: true
            FileTreeDemo{
                width: Fill height: Fill
            }
        }
    }

    // SlidePanel tab - slide panel demo
    let TabSlidePanel = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Overlay

        // Main content area
        View{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "SlidePanel Demo" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Click buttons to slide panels in/out from different sides" draw_text.color: #888 draw_text.text_style.font_size: 10}

            Hr{}

            View{width: Fill height: Fit flow: Right spacing: 10}
            slide_left_btn := Button{text: "Toggle Left Panel"}
            slide_top_btn := Button{text: "Toggle Top Panel"}
            slide_right_btn := Button{text: "Toggle Right Panel"}

            // Content area placeholder
            View{
                width: Fill height: Fill
                align: Center
                Label{text: "Main Content Area" draw_text.color: #666 draw_text.text_style.font_size: 14}
            }
        }

        // Left slide panel
        left_panel := SlidePanel{
            side: SlideSide.Left
            width: 200
            height: Fill

            RoundedView{
                width: Fill height: Fill
                draw_bg.color: #456
                draw_bg.radius: vec4(0.0 8.0 8.0 0.0)
                padding: 15 flow: Down spacing: 10

                Label{text: "Left Panel" draw_text.color: #fff draw_text.text_style.font_size: 12}
                Label{text: "This panel slides in from the left side." draw_text.color: #aaa draw_text.text_style.font_size: 10}
                Hr{}
                CheckBox{text: "Option 1"}
                CheckBox{text: "Option 2"}
                CheckBox{text: "Option 3"}
            }
        }

        // Top slide panel
        top_panel := SlidePanel{
            side: SlideSide.Top
            width: Fill
            height: 120

            RoundedView{
                width: Fill height: Fill
                draw_bg.color: #546
                draw_bg.radius: vec4(0.0 0.0 8.0 8.0)
                padding: 15 flow: Down spacing: 8

                Label{text: "Top Panel" draw_text.color: #fff draw_text.text_style.font_size: 12}
                Label{text: "This panel slides in from the top." draw_text.color: #aaa draw_text.text_style.font_size: 10}
                View{width: Fill height: Fit flow: Right spacing: 10}
                Button{text: "Action 1"}
                Button{text: "Action 2"}
            }
        }

        // Right slide panel
        right_panel := SlidePanel{
            side: SlideSide.Right
            width: 200
            height: Fill

            RoundedView{
                width: Fill height: Fill
                draw_bg.color: #564
                draw_bg.radius: vec4(8.0 0.0 0.0 8.0)
                padding: 15 flow: Down spacing: 10

                Label{text: "Right Panel" draw_text.color: #fff draw_text.text_style.font_size: 12}
                Label{text: "This panel slides in from the right side." draw_text.color: #aaa draw_text.text_style.font_size: 10}
                Hr{}
                Toggle{text: "Setting A"}
                Toggle{text: "Setting B"}
            }
        }
    }

    // SlidesView tab - slides presentation demo
    let TabSlides = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Down padding: 10 spacing: 10

        Label{text: "SlidesView Demo" draw_text.color: #fff draw_text.text_style.font_size: 13}
        Label{text: "Use arrow keys (left/right) to navigate slides" draw_text.color: #888 draw_text.text_style.font_size: 10}

        slides := SlidesView{
            width: Fill height: Fill

            slide1 := Slide{
                title := H1{text: "Welcome to Makepad"}
                SlideBody{text: "A modern UI framework for Rust"}
            }

            slide2 := SlideChapter{
                title := H1{text: "Chapter 1: Getting Started"}
                SlideBody{text: "Learn the basics of Makepad widgets"}
            }

            slide3 := Slide{
                title := H1{text: "Features"}
                SlideBody{text: "- Fast GPU rendering"}
                SlideBody{text: "- Cross-platform support"}
                SlideBody{text: "- Live design system"}
            }

            slide4 := SlideChapter{
                title := H1{text: "Chapter 2: Advanced Topics"}
                SlideBody{text: "Dive deeper into Makepad"}
            }

            slide5 := Slide{
                title := H1{text: "Thank You!"}
                SlideBody{text: "Questions?"}
            }
        }
    }

    // ===========================================
    // MATH VIEW TAB - LaTeX math rendering
    // ===========================================

    let TabMathView = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "MathView - LaTeX Rendering" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "LaTeX equations rendered via Typst" draw_text.color: #888 draw_text.text_style.font_size: 10}

            Hr{}

            Label{text: "Quadratic Formula" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            MathView{text: "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}" font_size: 14.0}

            Hr{}

            Label{text: "Euler's Identity" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            MathView{text: "e^{i\\pi} + 1 = 0" font_size: 16.0}

            Hr{}

            Label{text: "Integral" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            MathView{text: "\\int_0^\\infty e^{-x^2} dx = \\frac{\\sqrt{\\pi}}{2}" font_size: 14.0}

            Hr{}

            Label{text: "Matrix" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            MathView{text: "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}" font_size: 14.0}

            Hr{}

            Label{text: "Sum" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            MathView{text: "\\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}" font_size: 14.0}

            Hr{}

            Label{text: "Maxwell's Equations" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            MathView{text: "\\nabla \\times \\mathbf{E} = -\\frac{\\partial \\mathbf{B}}{\\partial t}" font_size: 14.0}

            Hr{}

            Label{text: "Different Sizes" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            View{width: Fill height: Fit flow: Right spacing: 15 align: Align{y: 0.5}}
            MathView{text: "\\alpha + \\beta" font_size: 8.0}
            MathView{text: "\\alpha + \\beta" font_size: 12.0}
            MathView{text: "\\alpha + \\beta" font_size: 18.0}
            MathView{text: "\\alpha + \\beta" font_size: 24.0}
        }
    }

    // ===========================================
    // BIG TEXT TAB - Huge font labels stacked vertically
    // ===========================================

    let TabBigText = SolidView{
        width: Fill height: Fill
        draw_bg.color: #222
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 20 spacing: 0
            align: Align{x: 0.5}

            Label{text: "M" draw_text.color: #f44 draw_text.text_style.font_size: 400}
            Label{text: "ak" draw_text.color: #f80 draw_text.text_style.font_size: 400}
            Label{text: "ep" draw_text.color: #fc0 draw_text.text_style.font_size: 400}
            Label{text: "ad" draw_text.color: #4c4 draw_text.text_style.font_size: 400}
            Label{text: "!" draw_text.color: #48f draw_text.text_style.font_size: 400}
        }
    }

    // Vector tab - SVG examples recreated in Splash Vector syntax
    // These should look identical to the SVG versions in the Media tab

    // Shared gradients for app_icon
    let glass_bg = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #x556677 opacity: 0.45}
        Stop{offset: 1 color: #x334455 opacity: 0.35}
    }
    let glass_border = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #xffffff opacity: 0.35}
        Stop{offset: 0.4 color: #xffffff opacity: 0.08}
        Stop{offset: 1 color: #xffffff opacity: 0.2}
    }
    let glass_spec = Gradient{x1: 0.1 y1: 0 x2: 0.7 y2: 0.8
        Stop{offset: 0 color: #xffffff opacity: 0.14}
        Stop{offset: 0.5 color: #xffffff opacity: 0.02}
        Stop{offset: 1 color: #xffffff opacity: 0.0}
    }
    let brain_glow_grad = RadGradient{cx: 0.5 cy: 0.45 r: 0.45
        Stop{offset: 0 color: #x4466ee opacity: 0.4}
        Stop{offset: 0.45 color: #x4466dd opacity: 0.15}
        Stop{offset: 1 color: #x4466dd opacity: 0.0}
    }
    let brain_grad = Gradient{x1: 0.5 y1: 0 x2: 0.5 y2: 1
        Stop{offset: 0 color: #x77ccff}
        Stop{offset: 0.4 color: #x7799ee}
        Stop{offset: 0.75 color: #x8866dd}
        Stop{offset: 1 color: #x9944cc}
    }
    let fold_grad = Gradient{x1: 0.5 y1: 0 x2: 0.5 y2: 1
        Stop{offset: 0 color: #xaaddff}
        Stop{offset: 1 color: #xbb99ee}
    }
    let kb_grad = Gradient{x1: 0 y1: 0.5 x2: 1 y2: 0.5
        Stop{offset: 0 color: #x9955ee}
        Stop{offset: 0.5 color: #x6688ff}
        Stop{offset: 1 color: #x44ddcc}
    }
    let kb_body = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #x8855dd opacity: 0.45}
        Stop{offset: 1 color: #x44aacc opacity: 0.3}
    }
    let stem_grad = Gradient{x1: 0.5 y1: 0 x2: 0.5 y2: 1
        Stop{offset: 0 color: #x7799dd}
        Stop{offset: 1 color: #x44cccc}
    }

    // Filters for app_icon
    let icon_shadow = Filter{
        DropShadow{dx: 0 dy: 4 blur: 6 color: #x000000 opacity: 0.5}
    }
    let kb_shadow = Filter{
        DropShadow{dx: 0 dy: 1 blur: 2 color: #x000000 opacity: 0.3}
    }

    let TabVector = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "SVG Icons - Splash Vector" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "icon_file, icon_folder, icon_select recreated in Splash" draw_text.color: #888 draw_text.text_style.font_size: 10}

            View{width: Fill height: Fit flow: Right spacing: 15 align: Align{y: 0.5}}

            // icon_file.svg (49x49)
            Vector{width: 32 height: 32 viewbox: vec4(0 0 49 49)
                Path{d: "M12.069,11.678c0,-2.23 1.813,-4.043 4.043,-4.043l10.107,0l0,8.086c0,1.118 0.903,2.021 2.021,2.021l8.086,0l0,18.193c0,2.23 -1.813,4.043 -4.043,4.043l-16.171,0c-2.23,0 -4.043,-1.813 -4.043,-4.043l0,-24.257Zm24.257,4.043l-8.086,0l0,-8.086l8.086,8.086Z"}
            }

            // icon_folder.svg (49x49)
            Vector{width: 32 height: 32 viewbox: vec4(0 0 49 49)
                Path{d: "M11.884,37.957l24.257,0c2.23,0 4.043,-1.813 4.043,-4.043l0,-16.172c0,-2.23 -1.813,-4.042 -4.043,-4.042l-10.107,0c-0.638,0 -1.238,-0.297 -1.617,-0.809l-1.213,-1.617c-0.765,-1.017 -1.965,-1.617 -3.235,-1.617l-8.085,0c-2.23,0 -4.043,1.813 -4.043,4.043l0,20.214c0,2.23 1.813,4.043 4.043,4.043Z"}
            }

            // icon_select.svg (48x49)
            Vector{width: 32 height: 32 viewbox: vec4(0 0 48 49)
                Path{d: "M33.21,28.207l-6.865,0l3.562,8.807c0.259,0.582 0,1.295 -0.583,1.554l-3.173,1.36c-0.583,0.259 -1.295,-0.065 -1.554,-0.648l-3.432,-8.354l-5.569,5.764c-0.777,0.777 -1.943,0.194 -1.943,-0.842l0,-27.781c0,-1.101 1.23,-1.619 1.943,-0.842l18.391,18.909c0.777,0.777 0.194,2.073 -0.777,2.073Z"}
            }

            Hr{}

            Label{text: "App Icon - Splash Vector" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "app_icon.svg with gradients, filters, groups, transforms" draw_text.color: #888 draw_text.text_style.font_size: 10}

            // app_icon.svg (256x256)
            Vector{width: 300 height: 300 viewbox: vec4(0 0 256 256)

                // Glass background
                Rect{x: 16 y: 16 w: 224 h: 224 rx: 44 ry: 44
                    fill: #x444455 fill_opacity: 0.35 filter: icon_shadow}
                Rect{x: 16 y: 16 w: 224 h: 224 rx: 44 ry: 44
                    fill: glass_bg}
                Rect{x: 16 y: 16 w: 224 h: 224 rx: 44 ry: 44
                    fill: false stroke: glass_border stroke_width: 1.5}

                // Glass specular
                Path{d: "M60 16 C35.7 16 16 35.7 16 60 L16 105 Q55 55 160 30 L190 16 Z"
                    fill: glass_spec}

                // Brain glow
                Circle{cx: 128 cy: 95 r: 80 fill: brain_glow_grad}

                // Brain paths
                Group{transform: [Translate{x: 36.8 y: 11.4} Scale{x: 7.6 y: 7.6}]
                    Path{d: "M15.5 13a3.5 3.5 0 0 0 -3.5 3.5v1a3.5 3.5 0 0 0 7 0v-1.8"
                        fill: false stroke: brain_grad stroke_width: 0.35
                        stroke_linecap: "round" stroke_linejoin: "round"}
                    Path{d: "M8.5 13a3.5 3.5 0 0 1 3.5 3.5v1a3.5 3.5 0 0 1 -7 0v-1.8"
                        fill: false stroke: brain_grad stroke_width: 0.35
                        stroke_linecap: "round" stroke_linejoin: "round"}
                    Path{d: "M17.5 16a3.5 3.5 0 0 0 0 -7h-.5"
                        fill: false stroke: brain_grad stroke_width: 0.35
                        stroke_linecap: "round" stroke_linejoin: "round"}
                    Path{d: "M19 9.3v-2.8a3.5 3.5 0 0 0 -7 0"
                        fill: false stroke: brain_grad stroke_width: 0.35
                        stroke_linecap: "round" stroke_linejoin: "round"}
                    Path{d: "M6.5 16a3.5 3.5 0 0 1 0 -7h.5"
                        fill: false stroke: brain_grad stroke_width: 0.35
                        stroke_linecap: "round" stroke_linejoin: "round"}
                    Path{d: "M5 9.3v-2.8a3.5 3.5 0 0 1 7 0v10"
                        fill: false stroke: brain_grad stroke_width: 0.35
                        stroke_linecap: "round" stroke_linejoin: "round"}
                    Path{d: "M15 13a4.17 4.17 0 0 1-3-4 4.17 4.17 0 0 1-3 4"
                        fill: false stroke: fold_grad stroke_width: 0.28
                        stroke_linecap: "round" stroke_linejoin: "round" stroke_opacity: 0.5}
                }

                // Stem
                Path{d: "M128 148 L128 178"
                    fill: false stroke: stem_grad stroke_width: 2.5 stroke_linecap: "round"}

                // Keyboard
                Group{filter: kb_shadow}
                Rect{x: 64 y: 185 w: 128 h: 38 rx: 7 ry: 7 fill: kb_body}
                Rect{x: 64 y: 185 w: 128 h: 38 rx: 7 ry: 7
                    fill: false stroke: kb_grad stroke_width: 1.2}

                // Keyboard Row 1
                Rect{x: 73 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 85 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 97 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 109 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 121 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 133 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 145 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 157 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 169 y: 190 w: 15 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}

                // Keyboard Row 2
                Rect{x: 76 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                Rect{x: 88 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                Rect{x: 100 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                Rect{x: 112 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                Rect{x: 124 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                Rect{x: 136 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                Rect{x: 148 y: 199 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}
                Rect{x: 160 y: 199 w: 24 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.15}

                // Keyboard Row 3
                Rect{x: 73 y: 208 w: 12 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.12}
                Rect{x: 88 y: 208 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.12}
                Rect{x: 100 y: 208 w: 50 h: 6 rx: 2 ry: 2 fill: #xffffff fill_opacity: 0.18}
                Rect{x: 153 y: 208 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.12}
                Rect{x: 165 y: 208 w: 19 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.12}
            }
        }
    }

    // Media tab - images, spinners, custom draws, SVGs
    let TabMedia = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "App Icon - Drop Shadow + Advanced Gradients" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "SVG filter support: feDropShadow, linear/radial gradients with opacity stops" draw_text.color: #888 draw_text.text_style.font_size: 9}
            Svg{
                width: 300 height: 300
                animating: false
                draw_svg +: { svg: crate_resource("self:resources/app_icon.svg") }
            }

            Hr{}

            Label{text: "Ocean Dream - Animated SVG + GPU Effects" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Svg{
                width: 600 height: 450
                animating: true
                draw_svg +: {
                    svg: crate_resource("self:resources/ocean_dream.svg")

                    // Hash function for pseudo-random
                    hash21: fn(p: vec2) -> float {
                        let p3x = fract(p.x * 0.1031f);
                        let p3y = fract(p.y * 0.1031f);
                        let p3z = fract(p.x * 0.1031f);
                        let d = dot(vec3(p3x, p3y, p3z), vec3(p3y + 33.33f, p3z + 33.33f, p3x + 33.33f));
                        return fract((p3x + d + p3y + d) * (p3z + d))
                    }

                    // Voronoi distance for caustics
                    voronoi: fn(uv: vec2) -> float {
                        let g = floor(uv);
                        let f = fract(uv);
                        var md = 8.0f;
                        var i = -1.0f;
                        loop {
                            if i > 1.5f { break }
                            var j = -1.0f;
                            loop {
                                if j > 1.5f { break }
                                let o = vec2(i, j);
                                let h1 = self.hash21(g + o);
                                let h2 = self.hash21(g + o + vec2(17.0f, 31.0f));
                                let r = o + vec2(h1, h2) - f;
                                let d = dot(r, r);
                                if d < md { md = d }
                                j = j + 1.0f;
                            }
                            i = i + 1.0f;
                        }
                        return sqrt(md)
                    }

                    // reflect(I, N) = I - 2 * dot(N, I) * N
                    reflect3: fn(i: vec3, n: vec3) -> vec3 {
                        return i - n * (2.0f * dot(n, i))
                    }

                    get_color: fn() {
                        let base = self.eval_gradient();
                        let id = self.v_shape_id;
                        let t = self.svg_time;

                        if id < 0.5f { return base }

                        // Unpremultiply base color so effects can work in linear RGB
                        let ba = max(base.a, 0.001f);
                        let bc = base.rgb / ba;

                        // Compute UV depending on gradient type:
                        // - Solid (type 0): bbox is in param1-4
                        // - Radial (type 2): center=param1,2 radius=param3,4
                        // - Linear (type 1): use v_world scaled to SVG viewport
                        let grad_type = self.v_param0;
                        var uv = vec2(0.0f, 0.0f);
                        if grad_type < 0.5f {
                            // Solid paint: bbox in params
                            let bmin = vec2(self.v_param1, self.v_param2);
                            let bmax = vec2(self.v_param3, self.v_param4);
                            let bsz = max(bmax - bmin, vec2(0.001f, 0.001f));
                            uv = (self.v_world - (bmin + bmax) * 0.5f) / (max(bsz.x, bsz.y) * 0.5f);
                        } else if grad_type > 1.5f {
                            // Radial gradient: center + radii in params
                            let center = vec2(self.v_param1, self.v_param2);
                            let radii = vec2(max(self.v_param3, 0.001f), max(self.v_param4, 0.001f));
                            uv = (self.v_world - center) / radii;
                        } else {
                            // Linear gradient: use world pos relative to gradient midpoint
                            let p0 = vec2(self.v_param1, self.v_param2);
                            let p1 = vec2(self.v_param3, self.v_param4);
                            let mid = (p0 + p1) * 0.5f;
                            let span = max(length(p1 - p0), 0.001f);
                            uv = (self.v_world - mid) / (span * 0.5f);
                        }

                        // ID 1: Jellyfish glow - soft ambient halo
                        if id < 1.5f {
                            let d = length(uv);
                            let glow = exp(-d * d * 2.0f);
                            let pulse = 0.8f + 0.2f * sin(t * 1.5f);
                            let out_rgb = bc * glow * pulse;
                            let out_a = ba * clamp(glow * pulse, 0.0f, 1.0f);
                            return vec4(out_rgb * out_a, out_a)
                        }

                        // ID 2: Jellyfish bell - dome with downward-flowing plasma
                        if id < 2.5f {
                            let d = length(uv);
                            // Dome-like implied normal from UV
                            let dome = max(1.0f - d * d, 0.0f);
                            let nz = sqrt(max(dome, 0.01f));
                            let normal = normalize(vec3(-uv.x, -uv.y, nz));
                            let view = vec3(0.0f, 0.0f, 1.0f);

                            // Animated light source
                            let light = normalize(vec3(sin(t * 1.1f) * 0.5f, cos(t * 0.7f) * 0.3f - 0.4f, 1.0f));
                            let diff = max(dot(normal, light), 0.0f) * 0.6f + 0.25f;

                            // Specular highlight
                            let half_v = normalize(light + view);
                            let spec = pow(max(dot(normal, half_v), 0.0f), 48.0f);

                            // Fresnel iridescence (rainbow shift at edges)
                            let ndv = max(dot(normal, view), 0.0f);
                            let fresnel = pow(1.0f - ndv, 2.0f);
                            let phase = ndv * 12.0f + t * 0.5f;
                            let iri = vec3(
                                0.5f + 0.5f * sin(phase),
                                0.5f + 0.5f * sin(phase + 2.09f),
                                0.5f + 0.5f * sin(phase + 4.19f)
                            );

                            // Downward-flowing plasma effect
                            let flow_y = uv.y - t * 0.8f;
                            let plasma1 = sin(uv.x * 6.0f + flow_y * 8.0f) * 0.5f + 0.5f;
                            let plasma2 = sin(uv.x * 3.0f - flow_y * 5.0f + 1.5f) * 0.5f + 0.5f;
                            let plasma3 = sin((uv.x + uv.y) * 4.0f + flow_y * 6.0f + 2.8f) * 0.5f + 0.5f;
                            let plasma = plasma1 * 0.5f + plasma2 * 0.3f + plasma3 * 0.2f;

                            // Plasma is stronger toward edges (fresnel-like) and fades at center
                            let plasma_mask = fresnel * 0.6f + 0.15f;
                            let plasma_color = vec3(1.0f, 0.7f, 1.0f) * plasma * plasma_mask;

                            let out_rgb = bc * diff + plasma_color + iri * fresnel * 0.3f + vec3(1.0f, 1.0f, 1.0f) * spec * 0.8f;
                            return vec4(out_rgb * ba, ba)
                        }

                        // ID 3: Bubbles - glass sphere with thin-film rainbow
                        if id < 3.5f {
                            let d = length(uv);
                            let sphere = max(1.0f - d * d, 0.0f);
                            if sphere < 0.01f { return base }
                            let nz = sqrt(sphere);
                            let normal = normalize(vec3(-uv.x, -uv.y, nz));
                            let view = vec3(0.0f, 0.0f, 1.0f);
                            let ndv = max(dot(normal, view), 0.0f);

                            // Fresnel reflectance
                            let fresnel = 0.06f + 0.94f * pow(1.0f - ndv, 4.0f);

                            // Thin-film color bands
                            let thickness = ndv * 16.0f + t * 1.2f + d * 4.0f;
                            let film = vec3(
                                0.5f + 0.5f * sin(thickness),
                                0.5f + 0.5f * sin(thickness + 2.09f),
                                0.5f + 0.5f * sin(thickness + 4.19f)
                            );

                            // Specular highlight (offset upward-left for 3D)
                            let light = normalize(vec3(0.3f, -0.5f, 1.0f));
                            let half_v = normalize(light + view);
                            let spec = pow(max(dot(normal, half_v), 0.0f), 80.0f);

                            // Ocean-tinted environment
                            let env = vec3(0.08f, 0.15f, 0.25f) + vec3(0.05f, 0.1f, 0.2f) * normal.y;

                            let out_rgb = env * fresnel + film * fresnel * 0.5f + vec3(1.0f, 1.0f, 1.0f) * spec * 1.5f;
                            let out_a = clamp(fresnel * 0.7f + spec + 0.05f, 0.0f, 1.0f) * ba;
                            return vec4(out_rgb * out_a, out_a)
                        }

                        // ID 4: Bioluminescent particles - pulsing glow
                        if id < 4.5f {
                            let d = length(uv);
                            let glow = exp(-d * d * 1.8f);
                            let pulse = 0.6f + 0.4f * sin(t * 4.0f + d * 3.0f);
                            // Bioluminescent green-cyan color
                            let bio_color = vec3(
                                0.2f + 0.3f * sin(t * 1.5f),
                                0.8f + 0.2f * sin(t * 0.7f + 1.0f),
                                0.3f + 0.4f * sin(t * 1.1f + 2.5f)
                            );
                            let out_rgb = (bc + bio_color * 2.0f) * glow * pulse;
                            let out_a = clamp(glow * pulse * 1.5f, 0.0f, 1.0f) * ba;
                            return vec4(out_rgb * out_a, out_a)
                        }

                        // ID 5: Light rays - underwater caustic shimmer
                        if id < 5.5f {
                            // Use world-scaled UV for large-scale caustic pattern
                            let cuv = uv * 1.5f;
                            let v1 = self.voronoi(cuv + vec2(t * 0.12f, t * 0.08f));
                            let v2 = self.voronoi(cuv * 1.7f + vec2(-t * 0.15f, t * 0.1f));
                            let caustic = v1 * v2;
                            // Gentle brightness boost along the rays
                            let bright = pow(1.0f - caustic, 1.5f) * 0.5f;
                            let out_rgb = bc * (1.0f + bright);
                            return vec4(out_rgb * ba, ba)
                        }

                        // ID 6: Ocean background - 1D vertical light rays with y-fade
                        if id < 6.5f {
                            let wx = self.v_world.x * 0.012f;

                            // 1D voronoi on X axis only - creates vertical ray columns
                            // Use hash21 with y=0 to get 1D cell pattern
                            let v1 = self.voronoi(vec2(wx * 1.0f + t * 0.06f, 0.0f));
                            let v2 = self.voronoi(vec2(wx * 2.5f - t * 0.04f, 0.0f));
                            let v3 = self.voronoi(vec2(wx * 0.5f + t * 0.03f, 0.0f));

                            // Sharp bright lines where rays are
                            let c1 = pow(1.0f - v1, 4.0f);
                            let c2 = pow(1.0f - v2, 3.0f);
                            let c3 = pow(1.0f - v3, 2.5f);
                            let caustic = c1 * 0.5f + c2 * 0.3f + c3 * 0.2f;

                            // Vertical fade: strong at top, fading toward bottom
                            // uv.y goes from -1 (top) to +1 (bottom) for vertical gradient
                            let depth_fade = clamp(1.0f - (uv.y + 1.0f) * 0.5f, 0.0f, 1.0f);
                            let depth_fade2 = depth_fade * depth_fade * depth_fade;

                            // Add subtle vertical shimmer/movement
                            let wy = self.v_world.y * 0.008f;
                            let shimmer = sin(wy * 3.0f + t * 0.5f) * 0.15f + 0.85f;

                            let ray_color = vec3(0.4f, 0.7f, 1.0f);
                            let ray_strength = caustic * depth_fade2 * shimmer * 0.4f;

                            let out_rgb = bc + ray_color * ray_strength;
                            return vec4(out_rgb * ba, ba)
                        }

                        // ID 7: Tentacle light rays - animated pulses moving down the stroke
                        if id < 7.5f {
                            let sd = self.v_stroke_dist;

                            // Multiple light pulses traveling downward along the tentacle
                            // sd increases along the path length
                            let pulse_speed = 80.0f;
                            let pulse_spacing = 40.0f;
                            let pulse_width = 12.0f;

                            // Create repeating pulses moving down (increasing sd)
                            let phase1 = modf(sd - t * pulse_speed, pulse_spacing);
                            let pulse1 = exp(-phase1 * phase1 / (pulse_width * pulse_width));

                            let phase2 = modf(sd - t * pulse_speed * 0.7f + pulse_spacing * 0.5f, pulse_spacing * 1.3f);
                            let pulse2 = exp(-phase2 * phase2 / (pulse_width * 1.5f * pulse_width * 1.5f));

                            let phase3 = modf(sd - t * pulse_speed * 1.2f + pulse_spacing * 0.3f, pulse_spacing * 0.8f);
                            let pulse3 = exp(-phase3 * phase3 / (pulse_width * 0.8f * pulse_width * 0.8f));

                            let pulse = pulse1 * 0.6f + pulse2 * 0.3f + pulse3 * 0.2f;

                            // Light ray color - bright white-blue glow
                            let ray_color = vec3(0.5f, 0.8f, 1.0f);
                            let brightness = pulse * 1.5f;

                            let out_rgb = bc + ray_color * brightness;
                            let out_a = ba;
                            return vec4(out_rgb * out_a, out_a)
                        }

                        return base
                    }
                }
            }

            Hr{}

            Label{text: "Images" draw_text.color: #fff draw_text.text_style.font_size: 13}
            test_image := Image{width: 180 height: 120 fit: ImageFit.Stretch}

            Hr{}

            Label{text: "SVG Icons" draw_text.color: #fff draw_text.text_style.font_size: 13}
            View{width: Fill height: Fit flow: Right spacing: 15 align: Align{y: 0.5}}
            Svg{
                width: 32 height: 32
                draw_svg +: { svg: crate_resource("self:../../widgets2/resources/icons/icon_file.svg") }
            }
            Svg{
                width: 32 height: 32
                draw_svg +: { svg: crate_resource("self:../../widgets2/resources/icons/icon_folder.svg") }
            }
            Svg{
                width: 32 height: 32
                draw_svg +: { svg: crate_resource("self:../../widgets2/resources/icons/icon_select.svg") }
            }

            Hr{}

            Label{text: "SVG Tiger" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Svg{
                width: 300 height: 300
                animating: false
                draw_svg +: { svg: crate_resource("self:resources/tiger.svg") }
            }

            Hr{}

            Label{text: "Loading Spinner" draw_text.color: #fff draw_text.text_style.font_size: 13}
            spinner := LoadingSpinner{width: 40 height: 40}

            Hr{}

            Label{text: "Custom Shader" draw_text.color: #fff draw_text.text_style.font_size: 13}
            test := TestDraw{}
        }
    }

    // Modal tab - modal dialog demos
    let TabModal = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Overlay

        // Main content with buttons to trigger modals
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Modal Dialogs" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Click the buttons below to open different modal dialogs" draw_text.color: #888 draw_text.text_style.font_size: 10}

            Hr{}

            Label{text: "Basic Modal" draw_text.color: #fff draw_text.text_style.font_size: 11}
            open_modal_btn := Button{text: "Open Modal"}

            Hr{}

            Label{text: "Confirmation Modal" draw_text.color: #fff draw_text.text_style.font_size: 11}
            open_confirm_modal_btn := Button{text: "Open Confirmation Dialog"}

            Hr{}

            Label{text: "Non-dismissable Modal" draw_text.color: #fff draw_text.text_style.font_size: 11}
            Label{text: "This modal cannot be dismissed by clicking outside" draw_text.color: #888 draw_text.text_style.font_size: 9}
            open_nodismiss_modal_btn := Button{text: "Open Non-dismissable Modal"}

            Hr{}

            modal_status := Label{text: "Modal status: Closed" draw_text.color: #8f8 draw_text.text_style.font_size: 10}
        }

        // Basic Modal
        test_modal := Modal{
            content +: {
                width: 300
                height: Fit
                padding: 20
                spacing: 15
                align: Center

                RoundedView{
                    width: Fill height: Fit
                    draw_bg.color: #445
                    draw_bg.radius: 8.0
                    padding: 20 spacing: 12
                    flow: Down align: Center

                    Label{text: "Basic Modal" draw_text.color: #fff draw_text.text_style.font_size: 14}
                    Label{text: "This is a basic modal dialog. Click outside or press Escape to close." draw_text.color: #aaa draw_text.text_style.font_size: 10}

                    View{height: 10}

                    close_modal_btn := Button{text: "Close Modal"}
                }
            }
        }

        // Confirmation Modal
        confirm_modal := Modal{
            content +: {
                width: 350
                height: Fit

                RoundedView{
                    width: Fill height: Fit
                    draw_bg.color: #445
                    draw_bg.radius: 8.0
                    padding: 25 spacing: 15
                    flow: Down

                    Label{text: "Confirm Action" draw_text.color: #fff draw_text.text_style.font_size: 14}
                    Label{text: "Are you sure you want to perform this action? This cannot be undone." draw_text.color: #aaa draw_text.text_style.font_size: 10}

                    View{height: 10}

                    View{
                        width: Fill height: Fit
                        flow: Right spacing: 10 align: Align{x: 1.0 y: 0.5}

                        cancel_confirm_btn := ButtonFlat{text: "Cancel"}
                        confirm_btn := Button{text: "Confirm"}
                    }
                }
            }
        }

        // Non-dismissable Modal
        nodismiss_modal := Modal{
            can_dismiss: false
            content +: {
                width: 320
                height: Fit

                RoundedView{
                    width: Fill height: Fit
                    draw_bg.color: #544
                    draw_bg.radius: 8.0
                    padding: 25 spacing: 15
                    flow: Down align: Center

                    Label{text: "Non-dismissable Modal" draw_text.color: #fff draw_text.text_style.font_size: 14}
                    Label{text: "This modal can only be closed by clicking the button below. Clicking outside or pressing Escape won't work." draw_text.color: #daa draw_text.text_style.font_size: 10}

                    View{height: 10}

                    close_nodismiss_btn := Button{text: "I Understand, Close Modal"}
                }
            }
        }
    }

    let AppDock = Dock{
        width: Fill height: Fill

        // Dock structure - 3 areas: left, center-top, center-bottom
        root := DockSplitter{
            axis: SplitterAxis.Horizontal
            align: SplitterAlign.FromA(280.0)
            a: @left_tabs
            b: @right_split
        }

        right_split := DockSplitter{
            axis: SplitterAxis.Vertical
            align: SplitterAlign.FromB(250.0)
            a: @center_tabs
            b: @bottom_tabs
        }

        // Left panel - Selection test first, then input widgets
        left_tabs := DockTabs{
            tabs: [@scrollbar_test_tab, @selection_test_tab, @toggles_tab, @sliders_tab, @text_tab, @dropdowns_tab]
            selected: 0
            closable: false
        }

        // Center panel - content widgets
        center_tabs := DockTabs{
            tabs: [@bigtext_tab, @math_tab, @vector_tab, @media_tab, @markup_tab, @buttons_tab, @modal_tab, @lists_tab]
            selected: 0
            closable: true
        }

        // Bottom panel - containers/presentations
        bottom_tabs := DockTabs{
            tabs: [@slidepanel_tab, @slides_tab, @filetree_tab, @folds_tab, @expandable_tab]
            selected: 0
            closable: true
        }

        // Selection test tab - first tab for testing cross-boundary selection
        // Scrollbar test tab - first tab for testing variable height items
        scrollbar_test_tab := DockTab{
            name: "Scrollbar"
            template: @CloseableTab
            kind: @TabScrollbarTest        }

        selection_test_tab := DockTab{
            name: "Selection"
            template: @CloseableTab
            kind: @TabSelectionTest        }

        // Individual tabs
        bigtext_tab := DockTab{
            name: "BigText"
            template: @CloseableTab
            kind: @TabBigText        }

        math_tab := DockTab{
            name: "Math"
            template: @CloseableTab
            kind: @TabMathView        }

        vector_tab := DockTab{
            name: "Vector"
            template: @CloseableTab
            kind: @TabVector        }

        buttons_tab := DockTab{
            name: "Buttons"
            template: @CloseableTab
            kind: @TabButtons        }

        toggles_tab := DockTab{
            name: "Toggles"
            template: @CloseableTab
            kind: @TabToggles        }

        sliders_tab := DockTab{
            name: "Sliders"
            template: @CloseableTab
            kind: @TabSliders        }

        text_tab := DockTab{
            name: "Text"
            template: @CloseableTab
            kind: @TabText        }

        dropdowns_tab := DockTab{
            name: "Selects"
            template: @CloseableTab
            kind: @TabDropdowns        }

        markup_tab := DockTab{
            name: "Markup"
            template: @CloseableTab
            kind: @TabMarkup        }

        folds_tab := DockTab{
            name: "Folds"
            template: @CloseableTab
            kind: @TabFolds        }

        lists_tab := DockTab{
            name: "Lists"
            template: @CloseableTab
            kind: @TabLists        }

        expandable_tab := DockTab{
            name: "Expandable"
            template: @CloseableTab
            kind: @TabExpandable        }

        media_tab := DockTab{
            name: "Media"
            template: @CloseableTab
            kind: @TabMedia        }

        filetree_tab := DockTab{
            name: "FileTree"
            template: @CloseableTab
            kind: @TabFileTree        }

        slidepanel_tab := DockTab{
            name: "SlidePanel"
            template: @CloseableTab
            kind: @TabSlidePanel        }

        slides_tab := DockTab{
            name: "Slides"
            template: @CloseableTab
            kind: @TabSlides        }

        modal_tab := DockTab{
            name: "Modal"
            template: @CloseableTab
            kind: @TabModal        }

        // Content templates by widget type
        TabBigText := TabBigText{}
        TabMathView := TabMathView{}
        TabVector := TabVector{}
        TabScrollbarTest := TabScrollbarTest{}
        TabSelectionTest := TabSelectionTest{}
        TabButtons := TabButtons{}
        TabToggles := TabToggles{}
        TabSliders := TabSliders{}
        TabText := TabText{}
        TabDropdowns := TabDropdowns{}
        TabMarkup := TabMarkup{}
        TabFolds := TabFolds{}
        TabLists := TabLists{}
        TabMedia := TabMedia{}
        TabExpandable := TabExpandable{}
        TabModal := TabModal{}
        TabFileTree := TabFileTree{}
        TabSlides := TabSlides{}
        TabSlidePanel := TabSlidePanel{}
    }

    mod.gc.set_static(AppDock)
    mod.gc.run()

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: vec4(0.3 0.3 0.3 1.0)
                window.inner_size: vec2(1000 700)
                body +: {
                    padding: 4
                    dock := AppDock{}
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // Load a test image into the Image widget
        let image_path =
            Path::new("tools/open_harmony/deveco/AppScope/resources/base/media/app_icon.png");
        if let Err(e) = self
            .ui
            .image(cx, ids!(test_image))
            .load_image_file_by_path(cx, image_path)
        {
            log!("Failed to load image: {:?}", e);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(button)).clicked(actions) {
            log!("Button clicked!");
        }
        if self.ui.button(cx, ids!(flat_button)).clicked(actions) {
            log!("Flat button clicked!");
        }
        if self.ui.button(cx, ids!(flatter_button)).clicked(actions) {
            log!("Flatter button clicked!");
        }
        if self.ui.button(cx, ids!(icon_button)).clicked(actions) {
            log!("Icon button clicked!");
        }

        // Tooltip demo - show tooltips on button click
        if self.ui.button(cx, ids!(tooltip_btn1)).clicked(actions) {
            log!("Showing tooltip 1");
            self.ui
                .tooltip(cx, ids!(buttons_tooltip))
                .show_with_options(
                    cx,
                    dvec2(350.0, 280.0),
                    "This is the standard button. Click it to perform the primary action.",
                );
        }

        // Popup notification demo
        if self.ui.button(cx, ids!(show_popup_btn)).clicked(actions) {
            log!("Showing popup notification");
            self.ui.popup_notification(cx, ids!(popup_notif)).open(cx);
        }

        if let Some(value) = self.ui.check_box(cx, ids!(checkbox)).changed(actions) {
            log!("Checkbox changed: {}", value);
        }
        if let Some(value) = self.ui.check_box(cx, ids!(toggle)).changed(actions) {
            log!("Toggle changed: {}", value);
        }
        if let Some(index) = self
            .ui
            .radio_button_set(cx, ids_list!(radio1, radio2, radio3))
            .selected(cx, actions)
        {
            log!("Radio button selected: {}", index);
        }

        // ExpandablePanel test
        if self.ui.button(cx, ids!(reset_btn)).clicked(actions) {
            log!("Resetting expandable panel");
            self.ui.expandable_panel(cx, ids!(expandable)).reset(cx);
        }

        if let Some(offset) = self
            .ui
            .expandable_panel(cx, ids!(expandable))
            .scrolled_at(actions)
        {
            log!("ExpandablePanel scrolled to: {}", offset);
        }

        // Modal tests
        // Open basic modal
        if self.ui.button(cx, ids!(open_modal_btn)).clicked(actions) {
            log!("Opening basic modal");
            self.ui.modal(cx, ids!(test_modal)).open(cx);
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Basic Modal Open");
        }

        // Close basic modal
        if self.ui.button(cx, ids!(close_modal_btn)).clicked(actions) {
            log!("Closing basic modal via button");
            self.ui.modal(cx, ids!(test_modal)).close(cx);
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Closed via button");
        }

        // Check if basic modal was dismissed (clicked outside or pressed Escape)
        if self.ui.modal(cx, ids!(test_modal)).dismissed(actions) {
            log!("Basic modal was dismissed");
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Dismissed (clicked outside or Escape)");
        }

        // Open confirmation modal
        if self
            .ui
            .button(cx, ids!(open_confirm_modal_btn))
            .clicked(actions)
        {
            log!("Opening confirmation modal");
            self.ui.modal(cx, ids!(confirm_modal)).open(cx);
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Confirmation Modal Open");
        }

        // Cancel confirmation
        if self
            .ui
            .button(cx, ids!(cancel_confirm_btn))
            .clicked(actions)
        {
            log!("Confirmation cancelled");
            self.ui.modal(cx, ids!(confirm_modal)).close(cx);
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Confirmation Cancelled");
        }

        // Confirm action
        if self.ui.button(cx, ids!(confirm_btn)).clicked(actions) {
            log!("Action confirmed!");
            self.ui.modal(cx, ids!(confirm_modal)).close(cx);
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Action Confirmed!");
        }

        // Check if confirmation modal was dismissed
        if self.ui.modal(cx, ids!(confirm_modal)).dismissed(actions) {
            log!("Confirmation modal was dismissed");
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Confirmation dismissed");
        }

        // Open non-dismissable modal
        if self
            .ui
            .button(cx, ids!(open_nodismiss_modal_btn))
            .clicked(actions)
        {
            log!("Opening non-dismissable modal");
            self.ui.modal(cx, ids!(nodismiss_modal)).open(cx);
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Non-dismissable Modal Open");
        }

        // Close non-dismissable modal
        if self
            .ui
            .button(cx, ids!(close_nodismiss_btn))
            .clicked(actions)
        {
            log!("Closing non-dismissable modal via button");
            self.ui.modal(cx, ids!(nodismiss_modal)).close(cx);
            self.ui
                .label(cx, ids!(modal_status))
                .set_text(cx, "Modal status: Non-dismissable closed via button");
        }

        // SlidePanel tests
        if self.ui.button(cx, ids!(slide_left_btn)).clicked(actions) {
            log!("Toggling left slide panel");
            self.ui.slide_panel(cx, ids!(left_panel)).toggle(cx);
        }
        if self.ui.button(cx, ids!(slide_top_btn)).clicked(actions) {
            log!("Toggling top slide panel");
            self.ui.slide_panel(cx, ids!(top_panel)).toggle(cx);
        }
        if self.ui.button(cx, ids!(slide_right_btn)).clicked(actions) {
            log!("Toggling right slide panel");
            self.ui.slide_panel(cx, ids!(right_panel)).toggle(cx);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        cx.with_widget_tree(|cx| {
            self.match_event(cx, event);
            self.ui.handle_event(cx, event, &mut Scope::empty());
        });
    }
}

// TestDraw widget with draw_quad and draw_text shaders
#[derive(Script, ScriptHook, Widget)]
pub struct TestDraw {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_quad: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[rust]
    area: Area,
}

impl Widget for TestDraw {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);

        let rect = cx.turtle().rect();

        // Draw the quad with our custom shader
        self.draw_quad.draw_abs(
            cx,
            Rect {
                pos: rect.pos,
                size: dvec2(100.0, 100.0),
            },
        );

        // Draw text below the quad
        self.draw_text
            .draw_abs(cx, rect.pos + dvec2(0.0, 110.0), "Hello Splash!");

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

// NewsListTest widget demonstrating PortalList usage
#[derive(Script, ScriptHook, Widget)]
pub struct NewsListTest {
    #[deref]
    view: View,
}

impl Widget for NewsListTest {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                // Set the item range (header + 50 items + footer)
                list.set_item_range(cx, 0, 52);

                while let Some(item_id) = list.next_visible_item(cx) {
                    // Determine which template to use based on item_id
                    let template = match item_id {
                        //0 => id!(Header),
                        //51 => id!(Footer),
                        _ => id!(Item),
                    };

                    let item = list.item(cx, item_id, template);

                    // Set content for Item template
                    if item_id > 0 && item_id < 51 {
                        let title = format!("Item #{}", item_id);
                        let subtitle = match item_id % 4 {
                            0 => "This is a longer description that shows how text wraps",
                            1 => "Short description",
                            2 => "Medium length subtitle text here",
                            _ => "Another item in the list",
                        };
                        item.label(cx, ids!(title)).set_text(cx, &title);
                        item.label(cx, ids!(subtitle)).set_text(cx, subtitle);
                    }

                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// SelectionTestList widget demonstrating cross-boundary text selection in PortalList
#[derive(Script, ScriptHook, Widget)]
pub struct SelectionTestList {
    #[deref]
    view: View,
}

impl Widget for SelectionTestList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                // 200 items for testing scrolling
                list.set_item_range(cx, 0, 200);

                while let Some(item_id) = list.next_visible_item(cx) {
                    let mut item = list.item(cx, item_id, id!(Item)).as_view();

                    // Generate varied text content for each item
                    let text = match item_id % 10 {
                        0 => format!("[{}] This is a log entry with some important information about the system state.", item_id),
                        1 => format!("[{}] Warning: Something might need attention here. Please review the details.", item_id),
                        2 => format!("[{}] Error occurred at line 42: unexpected token 'foo' in expression.", item_id),
                        3 => format!("[{}] Successfully completed operation in 0.42ms", item_id),
                        4 => format!("[{}] Loading resources from disk... Processing file batch #{}", item_id, item_id * 7),
                        5 => format!("[{}] Connection established to server at 192.168.1.100:8080", item_id),
                        6 => format!("[{}] User 'admin' logged in from IP 10.0.0.1 at timestamp {}", item_id, item_id * 1000),
                        7 => format!("[{}] Memory usage: {}MB / 1024MB ({}%)", item_id, item_id * 5 % 800, (item_id * 5 % 800) * 100 / 1024),
                        8 => format!("[{}] Compiling module 'core' - {} dependencies resolved", item_id, item_id % 20 + 1),
                        _ => format!("[{}] Debug: variable x = {}, y = {}, z = {}", item_id, item_id * 3, item_id * 7, item_id * 11),
                    };

                    // Draw the item and its TextFlow
                    while let Some(step) = item.draw(cx, &mut Scope::empty()).step() {
                        if let Some(mut tf) = step.as_text_flow().borrow_mut() {
                            tf.draw_text(cx, &text);
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// ScrollbarTestList widget demonstrating variable height items in PortalList
#[derive(Script, ScriptHook, Widget)]
pub struct ScrollbarTestList {
    #[deref]
    view: View,
}

impl Widget for ScrollbarTestList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                // 10 items - should give roughly 2x viewport height so scrollbar is ~50%
                list.set_item_range(cx, 0, 10);

                while let Some(item_id) = list.next_visible_item(cx) {
                    // Cycle through different height templates
                    let (template, height_name) = match item_id % 4 {
                        0 => (id!(Small), "Small (30px)"),
                        1 => (id!(Medium), "Medium (60px)"),
                        2 => (id!(Large), "Large (120px)"),
                        _ => (id!(XLarge), "XLarge (200px)"),
                    };

                    let item_widget = list.item(cx, item_id, template);

                    // Set the label text
                    let text = format!("Item {} - {}", item_id, height_name);
                    item_widget.label(cx, ids!(label)).set_text(cx, &text);

                    item_widget.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// FileTreeDemo widget demonstrating FileTree usage
#[derive(Script, ScriptHook, Widget)]
pub struct FileTreeDemo {
    #[uid]
    uid: WidgetUid,
    #[redraw]
    #[live]
    file_tree: FileTree,
    #[walk]
    walk: Walk,
    #[rust]
    file_nodes: LiveIdMap<LiveId, FileNode>,
    #[rust]
    initialized: bool,
}

pub struct FileNode {
    pub name: String,
    pub child_edges: Option<Vec<FileEdge>>,
}

pub struct FileEdge {
    pub name: String,
    pub file_node_id: LiveId,
}

impl FileTreeDemo {
    fn draw_file_node(
        cx: &mut Cx2d,
        file_node_id: LiveId,
        file_tree: &mut FileTree,
        file_nodes: &LiveIdMap<LiveId, FileNode>,
    ) {
        if let Some(file_node) = file_nodes.get(&file_node_id) {
            match &file_node.child_edges {
                Some(child_edges) => {
                    if file_tree
                        .begin_folder(cx, file_node_id, &file_node.name)
                        .is_ok()
                    {
                        for child_edge in child_edges {
                            Self::draw_file_node(
                                cx,
                                child_edge.file_node_id,
                                file_tree,
                                file_nodes,
                            );
                        }
                        file_tree.end_folder();
                    }
                }
                None => {
                    file_tree.file(cx, file_node_id, &file_node.name);
                }
            }
        }
    }

    fn initialize_demo_tree(&mut self) {
        // Create a demo file tree structure
        let mut id_counter = 1u64;
        let mut next_id = || {
            let id = LiveId(id_counter);
            id_counter += 1;
            id
        };

        // Create some demo files and folders
        let file1_id = next_id();
        let file2_id = next_id();
        let file3_id = next_id();
        let subdir_id = next_id();
        let subfile1_id = next_id();
        let subfile2_id = next_id();
        let root_id = live_id!(root);

        // Files in subdirectory
        self.file_nodes.insert(
            subfile1_id,
            FileNode {
                name: "nested_file.rs".to_string(),
                child_edges: None,
            },
        );
        self.file_nodes.insert(
            subfile2_id,
            FileNode {
                name: "another_file.txt".to_string(),
                child_edges: None,
            },
        );

        // Subdirectory
        self.file_nodes.insert(
            subdir_id,
            FileNode {
                name: "src".to_string(),
                child_edges: Some(vec![
                    FileEdge {
                        name: "nested_file.rs".to_string(),
                        file_node_id: subfile1_id,
                    },
                    FileEdge {
                        name: "another_file.txt".to_string(),
                        file_node_id: subfile2_id,
                    },
                ]),
            },
        );

        // Root level files
        self.file_nodes.insert(
            file1_id,
            FileNode {
                name: "main.rs".to_string(),
                child_edges: None,
            },
        );
        self.file_nodes.insert(
            file2_id,
            FileNode {
                name: "Cargo.toml".to_string(),
                child_edges: None,
            },
        );
        self.file_nodes.insert(
            file3_id,
            FileNode {
                name: "README.md".to_string(),
                child_edges: None,
            },
        );

        // Root folder
        self.file_nodes.insert(
            root_id,
            FileNode {
                name: "project".to_string(),
                child_edges: Some(vec![
                    FileEdge {
                        name: "src".to_string(),
                        file_node_id: subdir_id,
                    },
                    FileEdge {
                        name: "main.rs".to_string(),
                        file_node_id: file1_id,
                    },
                    FileEdge {
                        name: "Cargo.toml".to_string(),
                        file_node_id: file2_id,
                    },
                    FileEdge {
                        name: "README.md".to_string(),
                        file_node_id: file3_id,
                    },
                ]),
            },
        );
    }
}

impl Widget for FileTreeDemo {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            self.initialize_demo_tree();
            self.initialized = true;
        }
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            self.file_tree
                .set_folder_is_open(cx, live_id!(root), true, Animate::No);
            Self::draw_file_node(cx, live_id!(root), &mut self.file_tree, &self.file_nodes);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.file_tree.handle_event(cx, event, scope);
    }
}
