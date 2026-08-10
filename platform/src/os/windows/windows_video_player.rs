use {
    super::{
        windows_mf_source_reader::WindowsMfSourceReaderPlayer,
        windows_video_playback::WindowsVideoPlayer,
    },
    crate::{
        event::video_playback::VideoSource,
        gpu_texture::with_media_d3d11_lock,
        makepad_live_id::LiveId,
        media_plugin::PlaybackPrepared,
        texture::{
            CxTexturePool, TextureAlloc, TextureCategory, TextureFormat, TextureId, TexturePixel,
        },
        video_decode::software_video::PlaybackSessionHandle,
        video_decode::yuv::YuvPlaneData,
        windows::{
            core::Interface,
            Win32::Graphics::{
                Direct3D11::{
                    ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11ShaderResourceView,
                    ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC,
                    D3D11_USAGE_DEFAULT,
                },
                Dxgi::Common::{DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC},
            },
        },
    },
};

struct D3d11R8GpuPlane {
    width: u32,
    height: u32,
    texture: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
}

struct D3d11YuvCpuCache {
    y: Option<D3d11R8GpuPlane>,
    u: Option<D3d11R8GpuPlane>,
    v: Option<D3d11R8GpuPlane>,
}

impl D3d11YuvCpuCache {
    fn upload_y(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        data: &[u8],
        width: u32,
        height: u32,
        bytes_per_row: u32,
    ) {
        Self::upload_plane(
            &mut self.y,
            device,
            context,
            textures,
            texture_id,
            data,
            width,
            height,
            bytes_per_row,
        );
    }

    fn upload_u(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        data: &[u8],
        width: u32,
        height: u32,
        bytes_per_row: u32,
    ) {
        Self::upload_plane(
            &mut self.u,
            device,
            context,
            textures,
            texture_id,
            data,
            width,
            height,
            bytes_per_row,
        );
    }

    fn upload_v(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        data: &[u8],
        width: u32,
        height: u32,
        bytes_per_row: u32,
    ) {
        Self::upload_plane(
            &mut self.v,
            device,
            context,
            textures,
            texture_id,
            data,
            width,
            height,
            bytes_per_row,
        );
    }

    fn upload_plane(
        slot: &mut Option<D3d11R8GpuPlane>,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        data: &[u8],
        width: u32,
        height: u32,
        bytes_per_row: u32,
    ) {
        let w = width as usize;
        let h = height as usize;
        let bpr = bytes_per_row.max(width) as usize;
        if width == 0 || height == 0 || data.len() < bpr * (h.saturating_sub(1)) + w {
            return;
        }

        let needs_alloc = slot
            .as_ref()
            .map(|p| p.width != width || p.height != height)
            .unwrap_or(true);

        if needs_alloc {
            let desc = D3D11_TEXTURE2D_DESC {
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
            let mut texture = None;
            if unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.is_err() {
                return;
            }
            let Some(texture) = texture else {
                return;
            };
            let Ok(resource) = texture.cast::<ID3D11Resource>() else {
                return;
            };
            let mut srv = None;
            if unsafe {
                device.CreateShaderResourceView(&resource, None, Some(&mut srv))
            }
            .is_err()
            {
                return;
            };
            let Some(srv) = srv else {
                return;
            };
            *slot = Some(D3d11R8GpuPlane {
                width,
                height,
                texture,
                srv,
            });
        }

        let Some(plane) = slot.as_ref() else {
            return;
        };

        let dst_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: width,
            bottom: height,
            back: 1,
        };
        let resource: ID3D11Resource = match plane.texture.cast() {
            Ok(r) => r,
            Err(_) => return,
        };
        with_media_d3d11_lock(|| unsafe {
            context.UpdateSubresource(
                &resource,
                0,
                Some(&dst_box as *const _),
                data.as_ptr() as *const _,
                bpr as u32,
                0,
            );
        });

        let cxtexture = &mut textures[texture_id];
        cxtexture.os.texture = Some(plane.texture.clone());
        cxtexture.os.shader_resource_view = Some(plane.srv.clone());
        cxtexture.format = TextureFormat::VideoYuvPlane;
        cxtexture.alloc = Some(TextureAlloc {
            width: w,
            height: h,
            pixel: TexturePixel::VideoYuvPlane,
            category: TextureCategory::Video,
        });
    }
}

pub struct WindowsUnifiedVideoPlayer {
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    yuv_matrix: f32,
    yuv_biplanar: bool,
    /// True when the last NV12 frame used Texture2DArray zero-copy SRVs.
    yuv_array: bool,
    yuv_full_range: bool,
    #[cfg(target_os = "windows")]
    gpu_frame_keep_alive: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    /// Ping-pong ArraySize=1 NV12 targets for D3D11VA Texture2DArray present.
    #[cfg(target_os = "windows")]
    nv12_present: crate::gpu_texture::D3d11Nv12PresentCache,
    yuv_cpu_cache: D3d11YuvCpuCache,
    d3d11_device: ID3D11Device,
    d3d11_context: ID3D11DeviceContext,
    source: VideoSource,
    autoplay: bool,
    is_looping: bool,
    mode: WindowsPlayerMode,
    prepare_resolved: bool,
    /// After mid-playback SourceReader→MediaEngine fallback, seek once prepared.
    pending_fallback_seek_ms: Option<u64>,
}

enum WindowsPlayerMode {
    /// Progressive files: Source Reader + DXGI NV12 (true zero-copy when possible).
    SourceReader(WindowsMfSourceReaderPlayer),
    /// Adaptive / fallback: MediaEngine Transfer present.
    Native(WindowsVideoPlayer),
    Software(PlaybackSessionHandle),
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
            WindowsPlayerMode::Software(PlaybackSessionHandle::new(
                video_id,
                texture_id,
                source.clone(),
                autoplay,
                is_looping,
            ))
        } else if let Some(reader) = WindowsMfSourceReaderPlayer::try_new(
            d3d11_device,
            video_id,
            tex_y_id,
            tex_u_id,
            source.clone(),
            autoplay,
            is_looping,
        ) {
            crate::log!("VIDEO: using MF SourceReader DXGI path (scheme A)");
            WindowsPlayerMode::SourceReader(reader)
        } else if let Some(native) = WindowsVideoPlayer::new(
            d3d11_device,
            video_id,
            texture_id,
            tex_y_id,
            tex_u_id,
            source.clone(),
            autoplay,
            is_looping,
        ) {
            WindowsPlayerMode::Native(native)
        } else {
            crate::log!("VIDEO: Windows native playback unavailable, using software video decoder");
            WindowsPlayerMode::Software(PlaybackSessionHandle::new(
                video_id,
                texture_id,
                source.clone(),
                autoplay,
                is_looping,
            ))
        };

        let d3d11_context = unsafe { d3d11_device.GetImmediateContext() }.unwrap_or_else(|e| {
            crate::error!("VIDEO: GetImmediateContext failed: {:?}", e);
            panic!("D3D11 immediate context required for Windows video");
        });

        Self {
            video_id,
            texture_id,
            tex_y_id,
            tex_u_id,
            tex_v_id,
            yuv_matrix: 0.0,
            yuv_biplanar: false,
            yuv_array: false,
            yuv_full_range: false,
            gpu_frame_keep_alive: None,
            nv12_present: crate::gpu_texture::D3d11Nv12PresentCache::default(),
            yuv_cpu_cache: D3d11YuvCpuCache {
                y: None,
                u: None,
                v: None,
            },
            d3d11_device: d3d11_device.clone(),
            d3d11_context,
            source,
            autoplay,
            is_looping,
            mode,
            prepare_resolved: false,
            pending_fallback_seek_ms: None,
        }
    }

    fn switch_to_media_engine(&mut self, reason: &str) {
        let resume_ms = self.current_position_ms() as u64;
        crate::log!(
            "VIDEO: SourceReader failed, falling back to MediaEngine: {}",
            reason
        );
        self.yuv_biplanar = false;
        self.yuv_array = false;
        self.yuv_full_range = false;
        self.gpu_frame_keep_alive = None;
        if let Some(native) = WindowsVideoPlayer::new(
            &self.d3d11_device,
            self.video_id,
            self.texture_id,
            self.tex_y_id,
            self.tex_u_id,
            self.source.clone(),
            self.autoplay,
            self.is_looping,
        ) {
            self.mode = WindowsPlayerMode::Native(native);
            if resume_ms > 0 {
                self.pending_fallback_seek_ms = Some(resume_ms);
            }
        } else {
            self.switch_to_software("MediaEngine unavailable after SourceReader failure");
        }
    }

    fn switch_to_software(&mut self, reason: &str) {
        crate::log!(
            "VIDEO: Windows native playback failed, falling back to software video decoder: {}",
            reason
        );
        self.yuv_biplanar = false;
        self.yuv_array = false;
        self.yuv_full_range = false;
        self.gpu_frame_keep_alive = None;
        self.pending_fallback_seek_ms = None;
        self.mode = WindowsPlayerMode::Software(PlaybackSessionHandle::new(
            self.video_id,
            self.texture_id,
            self.source.clone(),
            self.autoplay,
            self.is_looping,
        ));
    }

    /// Pump the active native worker once per UI paint (coalesces Tick).
    pub fn sync_worker(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.sync_worker(),
            WindowsPlayerMode::Native(player) => player.sync_worker(),
            WindowsPlayerMode::Software(_) => {}
        }
        // Mid-playback SourceReader fatal → MediaEngine (prepare-time uses check_prepared).
        if let WindowsPlayerMode::SourceReader(player) = &mut self.mode {
            if let Some(err) = player.take_runtime_error() {
                self.switch_to_media_engine(&err);
                self.prepare_resolved = false;
                if let WindowsPlayerMode::Native(native) = &mut self.mode {
                    native.sync_worker();
                }
            }
        }
    }

    pub fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        let out = match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => match player.check_prepared() {
                Some(Err(err)) => {
                    self.switch_to_media_engine(&err);
                    self.prepare_resolved = false;
                    match &mut self.mode {
                        WindowsPlayerMode::Native(native) => native.check_prepared(),
                        WindowsPlayerMode::Software(software) => software.check_prepared(),
                        WindowsPlayerMode::SourceReader(_) => Some(Err(err)),
                    }
                }
                other => other,
            },
            WindowsPlayerMode::Native(player) => match player.check_prepared() {
                Some(Err(err)) => {
                    self.switch_to_software(&err);
                    self.prepare_resolved = false;
                    if let WindowsPlayerMode::Software(software) = &mut self.mode {
                        software.check_prepared()
                    } else {
                        Some(Err(err))
                    }
                }
                other => other,
            },
            WindowsPlayerMode::Software(player) => player.check_prepared(),
        };
        if let Some(Ok(_)) = &out {
            self.prepare_resolved = true;
            if let Some(ms) = self.pending_fallback_seek_ms.take() {
                self.seek_to(ms);
                if self.autoplay {
                    self.play();
                }
            }
        } else if out.is_some() {
            self.prepare_resolved = true;
        }
        out
    }

    pub fn poll_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => {
                let ok = player.poll_frame(textures);
                self.yuv_biplanar = player.presents_nv12();
                self.yuv_array = player.yuv_array();
                self.yuv_matrix = player.yuv_matrix();
                self.yuv_full_range = player.yuv_full_range();
                ok
            }
            WindowsPlayerMode::Native(player) => {
                let ok = player.poll_frame(textures);
                self.yuv_biplanar = player.presents_nv12();
                self.yuv_array = false;
                ok
            }
            WindowsPlayerMode::Software(player) => {
                if !player.poll_frame() {
                    return false;
                }
                if let Some(gpu) = player.take_d3d11_nv12_frame() {
                    self.yuv_matrix = gpu.matrix.as_f32();
                    self.yuv_biplanar = true;
                    match crate::gpu_texture::adopt_d3d11_nv12_biplanar(
                        &self.d3d11_device,
                        textures,
                        self.tex_y_id,
                        self.tex_u_id,
                        &gpu,
                        &mut self.nv12_present,
                    ) {
                        Ok(zero_copy) => {
                            self.yuv_array = zero_copy;
                            if zero_copy {
                                // Decoder surface must stay alive while Texture2DArray SRVs sample it.
                                self.gpu_frame_keep_alive = Some(gpu.keep_alive.clone());
                            } else {
                                // Blit copied pixels into present textures — release the
                                // D3D11VA surface immediately. Holding AVFrames (queue + keep_alive)
                                // exhausts the decoder pool → "Failed to add bitstream buffer".
                                self.gpu_frame_keep_alive = None;
                                drop(gpu);
                            }
                            true
                        }
                        Err(err) => {
                            crate::error!("VIDEO: adopt D3D11 NV12 failed: {err}");
                            self.gpu_frame_keep_alive = None;
                            self.yuv_array = false;
                            // Fall back to CPU YUV if the plugin also queued one.
                            if let Some(planes) = player.take_yuv_frame() {
                                self.yuv_matrix = planes.matrix.as_f32();
                                self.yuv_biplanar = false;
                                self.upload_yuv_to_d3d11(textures, &planes);
                                true
                            } else {
                                false
                            }
                        }
                    }
                } else if let Some(planes) = player.take_yuv_frame() {
                    self.yuv_matrix = planes.matrix.as_f32();
                    self.yuv_biplanar = false;
                    self.yuv_array = false;
                    self.gpu_frame_keep_alive = None;
                    self.upload_yuv_to_d3d11(textures, &planes);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn upload_yuv_to_d3d11(&mut self, textures: &mut CxTexturePool, planes: &YuvPlaneData) {
        let (cw, ch) = planes.layout.chroma_size(planes.width, planes.height);
        self.yuv_cpu_cache.upload_y(
            &self.d3d11_device,
            &self.d3d11_context,
            textures,
            self.tex_y_id,
            &planes.y,
            planes.width,
            planes.height,
            planes.y_bytes_per_row(),
        );
        self.yuv_cpu_cache.upload_u(
            &self.d3d11_device,
            &self.d3d11_context,
            textures,
            self.tex_u_id,
            &planes.u,
            cw,
            ch,
            planes.u_bytes_per_row(),
        );
        self.yuv_cpu_cache.upload_v(
            &self.d3d11_device,
            &self.d3d11_context,
            textures,
            self.tex_v_id,
            &planes.v,
            cw,
            ch,
            planes.v_bytes_per_row(),
        );
    }

    pub fn is_software_mode(&self) -> bool {
        matches!(self.mode, WindowsPlayerMode::Software(_))
    }

    /// YUV shader path: software decode, SourceReader DXGI NV12, or MF Transfer NV12.
    pub fn uses_yuv(&self) -> bool {
        self.is_software_mode()
            || matches!(self.mode, WindowsPlayerMode::SourceReader(_))
            || self.yuv_biplanar
    }

    pub fn yuv_matrix(&self) -> f32 {
        self.yuv_matrix
    }

    pub fn yuv_biplanar(&self) -> bool {
        self.yuv_biplanar
    }

    pub fn yuv_array(&self) -> bool {
        self.yuv_array
    }

    pub fn yuv_full_range(&self) -> bool {
        self.yuv_full_range
    }

    pub fn check_eos(&mut self) -> bool {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.check_eos(),
            WindowsPlayerMode::Native(player) => player.check_eos(),
            WindowsPlayerMode::Software(player) => player.check_eos(),
        }
    }

    pub fn is_playing(&self) -> bool {
        match &self.mode {
            WindowsPlayerMode::SourceReader(player) => player.is_playing(),
            WindowsPlayerMode::Native(player) => player.is_playing(),
            WindowsPlayerMode::Software(player) => player.is_playing(),
        }
    }

    /// Keep Poll while Media Foundation is still buffering, or while playing.
    pub fn keep_polling(&self) -> bool {
        match &self.mode {
            WindowsPlayerMode::SourceReader(player) => player.keep_polling(),
            WindowsPlayerMode::Native(player) => player.keep_polling(),
            WindowsPlayerMode::Software(player) => !self.prepare_resolved || player.is_playing(),
        }
    }

    pub fn play(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.play(),
            WindowsPlayerMode::Native(player) => player.play(),
            WindowsPlayerMode::Software(player) => player.play(),
        }
    }

    pub fn pause(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.pause(),
            WindowsPlayerMode::Native(player) => player.pause(),
            WindowsPlayerMode::Software(player) => player.pause(),
        }
    }

    pub fn resume(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.resume(),
            WindowsPlayerMode::Native(player) => player.resume(),
            WindowsPlayerMode::Software(player) => player.resume(),
        }
    }

    pub fn mute(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.mute(),
            WindowsPlayerMode::Native(player) => player.mute(),
            WindowsPlayerMode::Software(player) => player.mute(),
        }
    }

    pub fn unmute(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.unmute(),
            WindowsPlayerMode::Native(player) => player.unmute(),
            WindowsPlayerMode::Software(player) => player.unmute(),
        }
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.seek_to(position_ms),
            WindowsPlayerMode::Native(player) => player.seek_to(position_ms),
            WindowsPlayerMode::Software(player) => player.seek_to(position_ms),
        }
    }

    pub fn set_volume(&mut self, volume: f64) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.set_volume(volume),
            WindowsPlayerMode::Native(player) => player.set_volume(volume),
            WindowsPlayerMode::Software(player) => player.set_volume(volume),
        }
    }

    pub fn set_playback_rate(&mut self, rate: f64) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.set_playback_rate(rate),
            WindowsPlayerMode::Native(player) => player.set_playback_rate(rate),
            WindowsPlayerMode::Software(player) => player.set_playback_rate(rate),
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        match &self.mode {
            WindowsPlayerMode::SourceReader(player) => player.current_position_ms(),
            WindowsPlayerMode::Native(player) => player.current_position_ms(),
            WindowsPlayerMode::Software(player) => player.current_position_ms(),
        }
    }

    pub fn cleanup(&mut self) {
        match &mut self.mode {
            WindowsPlayerMode::SourceReader(player) => player.cleanup(),
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
