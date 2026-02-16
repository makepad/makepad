use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.WidgetsOverview = View{
        spacing: theme.space_2
        padding: theme.mspace_2
        flow: Down
        align: Align{x: 0.5 y: 0.5}
        height: Fill width: Fill

        ScrollYView{
            flow: Down
            width: 430. height: Fill
            align: Align{x: 0.0 y: 0.4}
            spacing: theme.space_3

            Image{margin: Inset{bottom: 10.} width: 250 height: 36.5 src: crate_resource("self:resources/logo_makepad.png") fit: ImageFit.Biggest}

            H4{text: "Makepad is an open-source, cross-platform UI framework written in and for Rust. It runs natively and on the web, supporting all major platforms: Windows, Linux, macOS, iOS, and Android."}
            P{
                text: "Built on a shader-based architecture, Makepad delivers high performance, making it suitable for complex applications like Photoshop or even 3D/VR/AR experiences."
            }
            P{
                text: "One of Makepad's standout features is live styling - a powerful system that reflects UI code changes instantly without recompilation or restarts. This tight feedback loop bridges the gap between developers and designers, streamlining collaboration and maximizing productivity."
            }
            P{
                text: "This example application provides an overview of the currently supported widgets."
            }

            TextBox{height: Fit text: "UI Zoo hosts a high number of widgets and variants, resulting in loading times not representative of typical Makepad applications."}
        }
    }
}
