use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoVideo = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Video\n\n`Video` plays hardware-accelerated video; `VideoPlayer` is a `Video` with the glass `PlayerControls` over it (play/pause, scrubber, skip, volume; tap to show or hide, double tap a side to skip 10 s, Space to play or pause)."}
        }
        demos +: {
            H4{text: "Network Video (autoplay, looping)"}
            VideoPlayer{
                height: 360
                width: 640
                video +: {
                    source: VideoDataSource.Network { url: "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4"}
                    show_idle_thumbnail: true
                }
            }
        }
    }
}
