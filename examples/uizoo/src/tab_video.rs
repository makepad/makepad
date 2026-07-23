use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoVideo = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Video\n\nVideo widget for hardware-accelerated video playback."}
        }
        demos +: {
            H4{text: "Network Video (autoplay, looping)"}
            Video{
                source: VideoDataSource.Network { url: "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4"}
                height: 240
                width: 426
                show_idle_thumbnail: true
            }
        }
    }
}
