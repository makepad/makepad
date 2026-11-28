use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoVideo = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Video\n\n`Video` plays hardware-accelerated video; `VideoPlayer` is a `Video` with the glass `PlayerControls` over it (play/pause, scrubber, skip, volume; tap to show or hide, double tap a side to skip 10 s, Space to play or pause)."}
        }
        demos +: {
            H4{text: "Network Video (loads when you press play)"}
            VideoPlayer{
                height: 360
                width: 640
                video +: {
                    source: VideoDataSource.Network { url: "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4"}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use makepad_widgets::test_cx::{checkout_test_cx, on_test_cx};
    use makepad_widgets::*;

    #[test]
    fn the_network_video_loads_only_when_play_is_pressed() {
        on_test_cx(|| {
            let mut cx = checkout_test_cx(|vm| {
                App::script_mod(vm);
            });
            let demo = cx.with_vm(|vm| {
                let widgets = vm.module(id!(widgets));
                let value = vm.value(widgets, id!(DemoVideo));
                WidgetRef::script_from_value(vm, value)
            });
            set_ui_root(&mut cx, &demo);
            let video = demo.video(&cx, ids!(video));
            let parent = cx
                .widget_tree()
                .parent_of(video.widget_uid())
                .expect("DemoVideo must contain a video child");
            let player = cx.widget_tree().widget(parent);
            let mut player = player
                .borrow_mut::<VideoPlayer>()
                .expect("the video child must belong to a VideoPlayer");

            assert!(
                video.is_unprepared(),
                "opening the Video tab must not load the network video"
            );
            player.play(&mut cx);
            assert!(
                video.is_preparing() || video.is_prepared(),
                "pressing play must load the network video"
            );
        });
    }
}
