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
                source: VideoDataSource.Network { url: "http://commondatastorage.googleapis.com/gtv-videos-bucket/sample/BigBuckBunny.mp4"}
                height: 240
                width: 426
                is_looping: true
                hold_to_pause: true
                autoplay: true
            }
        }
    }
}
