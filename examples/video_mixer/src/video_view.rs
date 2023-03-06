
use {
    crate::{
        makepad_draw::*,
        makepad_widgets::*,
        makepad_platform::video::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawVideo = {{DrawVideo}} {
        texture image: texture2d
        fn yuv_to_rgb(y: float, u: float, v: float) -> vec4 {
            return vec4(
                y + 1.14075 * (v-0.5),
                y - 0.3455 * (u-0.5) - 0.7169 * (v-0.5),
                y + 1.7790 * (u-0.5),
                1.0
            )
        }
        
        fn get_video_pixel(self)->vec4{ 
            let pix = self.pos * self.tex_size;
            
            // fetch pixel
            let data = sample2d(self.image, self.pos).xyzw;
            
            if mod (pix.x, 2.0)>1.0 {
                return yuv_to_rgb(data.x, data.y, data.w)
            }
            return yuv_to_rgb(data.z, data.y, data.w)
        }
        
        fn pixel(self) -> vec4 {
            return self.get_video_pixel();
        }
    }
    
    VideoView = {{VideoView}} {
        walk: {
            //margin: {top: 3, right: 10, bottom: 3, left: 10},
            width: Fill,
            height: Fill
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawVideo {
    draw_super: DrawQuad,
    tex_size: Vec2,
}

#[derive(Live, LiveHook)]
#[live_design_fn(widget_factory!(VideoView))]
pub struct VideoView {
    walk: Walk,
    video_texture: Texture,
    draw_video: DrawVideo
}

impl Widget for VideoView {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_video.redraw(cx);
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_video.set_texture(0, &self.video_texture);
        self.draw_video.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct VideoViewRef(WidgetRef);

impl VideoViewRef {
    pub fn update_video(&self, cx: &mut Cx, mut vfb: VideoFrameBuf) {
        if let Some(mut inner) = self.inner_mut() {
            inner.video_texture.set_desc(cx, TextureDesc {
                format: TextureFormat::ImageBGRA,
                width: Some(vfb.desc.width / 2),
                height: Some(vfb.desc.height)
            });
            inner.draw_video.tex_size = vec2(vfb.desc.width as f32, vfb.desc.height as f32);
            inner.video_texture.swap_image_u32(cx, &mut vfb.data);
            inner.redraw(cx);
        }
    }
}
 
