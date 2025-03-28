> Currently only supported on Android.

A widget for loading, displaying, and playing video data.

## Not yet supported
### UI
- Playback controls
- Progress/seek-to bar

### Widget API
- Seek to timestamp
- Option to restart playback manually when not looping.
- Hotswap video source: `set_source(VideoDataSource)` only works if the video is in the `Unprepared` state.

## [Layouting](Layouting.md)
Complete layouting feature set support.

## DrawShaders
### draw_bg ([DrawColor](DrawColor.md))
The `DrawShader` responsible for displaying the video content.

## Fields
### autoplay (bool = false)
Determines if the video should start playback when the widget is created. Defaults to `false`.

### hold_to_pause (bool = false)
Determines if the video should be paused when the user holds the pause button. Defaults to `false`.

### is_looping (bool = false)
Determines if the video should play in a loop. Defaults to `false`.

### mute (bool = false)
Mutes the video playback. Defaults to `false`.

### scale (f64 = 1.0)
A scaling factor for the displayed video. Defaults to `1.0`.

### show_thumbnail_before_playback (bool = false)
An option that shows a placeholder thumbnail before video playback. Defaults to `false`.

### source ([VideoDataSource](#videodatasource))
Specifies the source for the video playback. Can be one of:

- `Network { url: "https://www.someurl.com/video.mkv" }`  
  On Android, it supports HLS, DASH, RTMP, RTSP, and progressive HTTP downloads.

- `Filesystem { path: "/storage/.../DCIM/Camera/video.mp4" }`  
  On Android, it requires read permissions that must be granted at runtime.

- `Dependency { path: dep("crate://self/resources/video.mp4") }`  
  For in-memory videos loaded through `LiveDependencies`.

## VideoDataSource
An enumeration specifying the source of the video data. Can be one of:

- **Dependency { path: LiveDependency }**  
  The path to a `LiveDependency` (an asset loaded with `dep("crate://...")`).

- **Network { url: String }**  
  The URL of a video file. On Android, it supports HLS, DASH, RTMP, RTSP, and progressive HTTP downloads.

- **Filesystem { path: String }**  
  The path to a video file on the local filesystem. On Android, this requires runtime-approved permissions for reading storage.
### thumbnail_source (LiveDependency)
Determines the source for the thumbnail image. Currently only supports `LiveDependencies`.

## Examples
### Basic
```Rust
<Video> {
    source: dep("crate://self/resources/video.mp4"), // Path to the video file.
    thumbnail_source: dep("crate://self/resources/logo.png"), // Path to the thumbnail file.

    // LAYOUT PROPERTIES

    height: 360.0,
    // Element is 360.0 units high.

    width: 480.0,
    // Element is 480.0 units wide.
}
```

### Typical
```Rust
<Video> {
    autoplay: true, // Video playback starts automatically.
    hold_to_pause: false, // Video does not pause when holding the pause button.
    is_looping: true, // Video playback loops continuously.
    mute: false, // The video sound is not muted.
    scale: 1.0, // Default scaling factor.
    show_thumbnail_before_playback: true, // Display a placeholder cover image before playback.
    source: dep("crate://self/resources/video.mp4"), // Path to the video file.
    thumbnail_source: dep("crate://self/resources/logo.png"), // Path to the thumbnail file.

    // LAYOUT PROPERTIES

    height: 360.0,
    // Element is 360.0 units high.

    width: 480.0,
    // Element is 480.0 units wide.
}
```

### Advanced
```Rust
MyVideo = <Video> {
    autoplay: true, // Video playback starts automatically.
    hold_to_pause: false, // Video does not pause when holding the pause button.
    is_looping: true, // Video playback loops continuously.
    mute: false, // The video sound is not muted.
    scale: 1.1, // Scaling factor increased by 10%.
    show_thumbnail_before_playback: true, // Display a placeholder cover image before playback.
    source: dep("crate://self/resources/video.mp4"), // Path to the video file.
    thumbnail_source: dep("crate://self/resources/logo.png"), // Path to the thumbnail file.

    draw_bg: {
        shape: Solid, // The shape of the video display area.
        fill: Image,
        texture video_texture: textureOES, // Texture for video content.
        texture thumbnail_texture: texture2d, // Texture for thumbnail image.
        uniform show_thumbnail: 0.0, // Flag to show thumbnail.

        instance opacity: 1.0, // Opacity of the video display.
        instance image_scale: vec2(1.0, 1.0), // Scaling of the image.
        instance image_pan: vec2(0.5, 0.5), // Panning of the image.

        uniform source_size: vec2(1.0, 1.0), // Size of the source video.
        uniform target_size: vec2(-1.0, -1.0), // Size of the target display area.

        fn get_color_scale_pan(self) -> vec4 {
            // Early return for default scaling and panning,
            // used when walk size is not specified or non-fixed.
            if self.target_size.x <= 0.0 && self.target_size.y <= 0.0 {
                if self.show_thumbnail > 0.0 {
                    return sample2d(self.thumbnail_texture, self.pos).xyzw;
                } else {
                    return sample2dOES(self.video_texture, self.pos);
                }
            }

            var scale = self.image_scale;
            let pan = self.image_pan;
            let source_aspect_ratio = self.source_size.x / self.source_size.y;
            let target_aspect_ratio = self.target_size.x / self.target_size.y;

            // Adjust scale based on aspect ratio difference
            if source_aspect_ratio != target_aspect_ratio {
                if source_aspect_ratio > target_aspect_ratio {
                    scale.x = target_aspect_ratio / source_aspect_ratio;
                    scale.y = 1.0;
                } else {
                    scale.x = 1.0;
                    scale.y = source_aspect_ratio / target_aspect_ratio;
                }
            }

            // Calculate the range for panning
            let pan_range_x = max(0.0, (1.0 - scale.x));
            let pan_range_y = max(0.0, (1.0 - scale.y));

            // Adjust the user pan values to be within the pan range
            let adjusted_pan_x = pan_range_x * pan.x;
            let adjusted_pan_y = pan_range_y * pan.y;
            let adjusted_pan = vec2(adjusted_pan_x, adjusted_pan_y);
            let adjusted_pos = (self.pos * scale) + adjusted_pan;

            if self.show_thumbnail > 0.5 {
                return sample2d(self.thumbnail_texture, adjusted_pos).xyzw;
            } else {
                return sample2dOES(self.video_texture, adjusted_pos);
            }
        }

        fn pixel(self) -> vec4 {
            let color = self.get_color_scale_pan();
            return Pal::premul(vec4(color.xyz, color.w * self.opacity));
        }
    }

    // LAYOUT PROPERTIES

    height: 360.0,
    // Element is 360.0 units high.

    width: 640.0,
    // Element is 640.0 units wide.

    margin: 10.0,
    // Margin of 10.0 units around the element.

    padding: 0.0,
    // No padding inside the element.

    align: { x: 0.0, y: 0.0 },
    // Element alignment in its parent container.
}

<MyVideo> {
    source: dep("crate://self/resources/my_video.mp4"), // Path to the custom video file.
    thumbnail_source: dep("crate://self/resources/my_logo.png"), // Path to the custom thumbnail file.
}
```