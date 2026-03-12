use {
    super::windows_video_playback::WindowsVideoPlayer,
    crate::{
        event::video_playback::VideoSource,
        makepad_live_id::LiveId,
        texture::{
            CxTexturePool, TextureAlloc, TextureCategory, TextureFormat, TextureId, TexturePixel,
        },
        video_decode::software_video::SoftwareVideoPlayer,
        video_decode::yuv::YuvPlaneData,
        windows::{
            core::Interface,
            Win32::Graphics::{
                Direct3D11::{
                    ID3D11Device, ID3D11Resource, ID3D11ShaderResourceView, ID3D11Texture2D,
                    D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
                    D3D11_USAGE_DEFAULT,
                },
                Dxgi::Common::{DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC},
            },
        },
    },
};

pub struct WindowsUnifiedVideoPlayer {
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    yuv_matrix: f32,
    d3d11_device: ID3D11Device,
    source: VideoSource,
    autoplay: bool,
    is_looping: bool,
    mode: WindowsPlayerMode,
}

enum WindowsPlayerMode {
    Native(WindowsVideoPlayer),
    Software(SoftwareVideoPlayer),
}

impl WindowsUnifiedVideoPlayer {
    pub fn new(
        d3d11_device: &ID3D11Device,
        video_id: LiveId,
        texture_id: TextureId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        let force_software = std::env::var_os("MAKEPAD_FORCE_SOFTWARE_VIDEO").is_some();
        let mode = if force_software {
            crate::log!("VIDEO: MAKEPAD_FORCE_SOFTWARE_VIDEO set, using software video decoder");
            WindowsPlayerMode::Software(SoftwareVideoPlayer::new(
                video_id,
                texture_id,
                source.clone(),
                autoplay,
                is_looping,
            ))
        } else if let Some(native) = WindowsVideoPlayer::new(
            d3d11_device,
            video_id,
            texture_id,
            source.clone(),
            autoplay,
            is_looping,
        ) {
            WindowsPlayerMode::Native(native)
        } else {
            crate::log!("VIDEO: Windows native playback unavailable, using software video decoder");
            WindowsPlayerMode::Software(SoftwareVideoPlayer::new(
                video_id,
                texture_id,
                source.clone(),
                autoplay,
                is_looping,
            ))
        };

        Self {
            video_id,
            texture_id,
            tex_y_id,
            tex_u_id,
            tex_v_id,
            yuv_matrix: 0.0,
            d3d11_device: d3d11_device.clone(),
            source,
            autoplay,
            is_looping,
            mode,
        }
    }

    fn switch_to_software(&mut self, reason: &str) {
        crate::log!(
            "VIDEO: Windows native playback failed, falling back to software video decoder: {}",
            reason
        );
        self.mode = WindowsPlayerMode::Software(SoftwareVideoPlayer::new(
            self.video_id,
            self.texture_id,
            self.source.clone(),
            self.autoplay,
            self.is_looping,
        ));
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => match player.check_prepared() {
                Some(Err(err)) => {
                    self.switch_to_software(&err);
                    if let WindowsPlayerMode::Software(software) = &mut self.mode {
                        software.check_prepared()
                    } else {
                        Some(Err(err))
                    }
                }
                other => other,
            },
            WindowsPlayerMode::Software(player) => player.check_prepared(),
        }
    }

    pub fn poll_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.poll_frame(textures),
            WindowsPlayerMode::Software(player) => {
                if !player.poll_frame() {
                    return false;
                }
                if let Some(planes) = player.take_yuv_frame() {
                    self.yuv_matrix = planes.matrix.as_f32();
                    self.upload_yuv_to_d3d11(textures, &planes);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn upload_yuv_to_d3d11(&self, textures: &mut CxTexturePool, planes: &YuvPlaneData) {
        let (cw, ch) = planes.layout.chroma_size(planes.width, planes.height);
        self.upload_r8_plane_to_d3d11(
            textures,
            self.tex_y_id,
            &planes.y,
            planes.width,
            planes.height,
        );
        self.upload_r8_plane_to_d3d11(textures, self.tex_u_id, &planes.u, cw, ch);
        self.upload_r8_plane_to_d3d11(textures, self.tex_v_id, &planes.v, cw, ch);
    }

    fn upload_r8_plane_to_d3d11(
        &self,
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        data: &[u8],
        width: u32,
        height: u32,
    ) {
        let w = width as usize;
        let h = height as usize;
        if data.len() < w * h {
            return;
        }

        let sub_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr() as *const _,
            SysMemPitch: width,
            SysMemSlicePitch: 0,
        };

        let texture_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };

        let mut texture: Option<ID3D11Texture2D> = None;
        if self
            .d3d11_device
            .CreateTexture2D(&texture_desc, Some(&sub_data), Some(&mut texture))
            .is_err()
        {
            return;
        }

        let texture = match texture {
            Some(t) => t,
            None => return,
        };

        let resource: ID3D11Resource = match texture.cast() {
            Ok(r) => r,
            Err(_) => return,
        };

        let mut shader_resource_view: Option<ID3D11ShaderResourceView> = None;
        if self
            .d3d11_device
            .CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view))
            .is_err()
        {
            return;
        }

        let cxtexture = &mut textures[texture_id];
        cxtexture.os.texture = Some(texture);
        cxtexture.os.shader_resource_view = shader_resource_view;
        cxtexture.format = TextureFormat::VideoYuvPlane;
        cxtexture.alloc = Some(TextureAlloc {
            width: w,
            height: h,
            pixel: TexturePixel::VideoYuvPlane,
            category: TextureCategory::Video,
        });
    }

    pub fn is_software_mode(&self) -> bool {
        matches!(self.mode, WindowsPlayerMode::Software(_))
    }

    pub fn yuv_matrix(&self) -> f32 {
        self.yuv_matrix
    }

    pub fn check_eos(&mut self) -> bool {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.check_eos(),
            WindowsPlayerMode::Software(player) => player.check_eos(),
        }
    }

    pub fn is_playing(&self) -> bool {
        match &self.mode {
            WindowsPlayerMode::Native(player) => player.is_playing(),
            WindowsPlayerMode::Software(player) => player.is_playing(),
        }
    }

    pub fn play(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.play(),
            WindowsPlayerMode::Software(player) => player.play(),
        }
    }

    pub fn pause(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.pause(),
            WindowsPlayerMode::Software(player) => player.pause(),
        }
    }

    pub fn resume(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.resume(),
            WindowsPlayerMode::Software(player) => player.resume(),
        }
    }

    pub fn mute(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.mute(),
            WindowsPlayerMode::Software(player) => player.mute(),
        }
    }

    pub fn unmute(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.unmute(),
            WindowsPlayerMode::Software(player) => player.unmute(),
        }
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.seek_to(position_ms),
            WindowsPlayerMode::Software(player) => player.seek_to(position_ms),
        }
    }

    pub fn set_volume(&mut self, volume: f64) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.set_volume(volume),
            WindowsPlayerMode::Software(player) => player.set_volume(volume),
        }
    }

    pub fn set_playback_rate(&mut self, rate: f64) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.set_playback_rate(rate),
            WindowsPlayerMode::Software(player) => player.set_playback_rate(rate),
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        match &self.mode {
            WindowsPlayerMode::Native(player) => player.current_position_ms(),
            WindowsPlayerMode::Software(player) => player.current_position_ms(),
        }
    }

    pub fn cleanup(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::Native(player) => player.cleanup(),
            WindowsPlayerMode::Software(player) => player.cleanup(),
        }
    }
}

impl Drop for WindowsUnifiedVideoPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
