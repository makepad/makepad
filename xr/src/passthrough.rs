use super::*;

#[derive(Clone)]
pub(super) struct XrPassthroughCameraChoice {
    input_id: VideoInputId,
    format_id: VideoFormatId,
    width: usize,
    height: usize,
}

#[derive(Clone)]
pub(super) struct XrPassthroughCameraTextures {
    pub(super) camera: Texture,
    pub(super) tex_y: Option<Texture>,
    pub(super) tex_u: Option<Texture>,
    pub(super) tex_v: Option<Texture>,
}

pub(super) struct XrPassthroughEnvAtlas {
    pass: DrawPass,
    draw_list: DrawList2d,
    ping: Texture,
    pong: Texture,
    ping_is_current: bool,
    initialized: bool,
    pending_swap: bool,
}

impl XrPassthroughEnvAtlas {
    fn new(cx: &mut Cx) -> Self {
        let atlas_width = XR_PASSTHROUGH_ENV_ATLAS_WIDTH;
        let atlas_height = XR_PASSTHROUGH_ENV_ATLAS_HEIGHT;
        let ping = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: atlas_width,
                    height: atlas_height,
                },
                initial: true,
            },
        );
        let pong = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: atlas_width,
                    height: atlas_height,
                },
                initial: true,
            },
        );
        let pass = DrawPass::new_with_name(cx, "xr_passthrough_env_atlas");
        pass.set_size(cx, dvec2(atlas_width as f64, atlas_height as f64));
        Self {
            pass,
            draw_list: DrawList2d::new(cx),
            ping,
            pong,
            ping_is_current: true,
            initialized: false,
            pending_swap: false,
        }
    }

    fn current_texture(&self) -> &Texture {
        if self.ping_is_current {
            &self.ping
        } else {
            &self.pong
        }
    }

    fn target_texture(&self) -> &Texture {
        if self.ping_is_current {
            &self.pong
        } else {
            &self.ping
        }
    }

    fn finish_frame(&mut self) {
        self.ping_is_current = !self.ping_is_current;
        self.initialized = true;
        self.pending_swap = false;
    }

    pub(super) fn reset_state(&mut self) {
        self.ping_is_current = true;
        self.initialized = false;
        self.pending_swap = false;
    }
}

impl XrScene {
    fn passthrough_camera_center_offset_uv(&self) -> Vec2f {
        let source_size = self.passthrough_camera_source_size;
        let aspect = if source_size.y > 1.0 {
            source_size.x / source_size.y
        } else {
            4.0 / 3.0
        };
        let half_height = XR_PASSTHROUGH_QUAD_DISTANCE
            * (XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES.to_radians() * 0.5).tan()
            * XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
        let half_width = half_height * aspect;
        vec2f(
            -XR_PASSTHROUGH_QUAD_WORLD_OFFSET_X / (2.0 * half_width.max(0.0001)),
            XR_PASSTHROUGH_QUAD_WORLD_OFFSET_Y / (2.0 * half_height.max(0.0001)),
        )
    }

    pub(super) fn pick_passthrough_camera_choice(
        ev: &VideoInputsEvent,
    ) -> Option<XrPassthroughCameraChoice> {
        fn better(
            a: &makepad_widgets::makepad_platform::video::VideoFormat,
            b: &makepad_widgets::makepad_platform::video::VideoFormat,
        ) -> bool {
            let a_is_preferred_square = a.width == 1280 && a.height == 1280;
            let b_is_preferred_square = b.width == 1280 && b.height == 1280;
            if a_is_preferred_square != b_is_preferred_square {
                return a_is_preferred_square;
            }

            let a_fits_cap = a.width <= 1920 && a.height <= 1920;
            let b_fits_cap = b.width <= 1920 && b.height <= 1920;
            if a_fits_cap != b_fits_cap {
                return a_fits_cap;
            }

            let a_is_square = a.width == a.height;
            let b_is_square = b.width == b.height;
            if a_is_square != b_is_square {
                return a_is_square;
            }
            let a_pixels = a.width * a.height;
            let b_pixels = b.width * b.height;
            if a_pixels != b_pixels {
                return a_pixels > b_pixels;
            }
            a.frame_rate.unwrap_or(0.0) > b.frame_rate.unwrap_or(0.0)
        }

        let desc = ev
            .descs
            .iter()
            .find(|desc| desc.name == "Back Camera")
            .or_else(|| ev.descs.iter().find(|desc| desc.name == "External Camera"))
            .or_else(|| ev.descs.first())?;

        let mut best = None;
        for format in &desc.formats {
            if format.pixel_format != VideoPixelFormat::YUV420 {
                continue;
            }
            if best.as_ref().is_none_or(|current| better(format, current)) {
                best = Some(*format);
            }
        }

        let format = best?;
        Some(XrPassthroughCameraChoice {
            input_id: desc.input_id,
            format_id: format.format_id,
            width: format.width,
            height: format.height,
        })
    }

    pub(super) fn reset_passthrough_camera_state(&mut self) {
        self.passthrough_camera_playback_requested = false;
        self.passthrough_camera_failed = false;
        self.passthrough_camera_textures = None;
        self.passthrough_camera_video = VideoYuvMetadata::disabled();
        self.passthrough_camera_has_frame = false;
    }

    pub(super) fn sync_passthrough_camera(&mut self, cx: &mut Cx) {
        if matches!(
            self.passthrough_camera_permission,
            Some(PermissionStatus::DeniedCanRetry) | Some(PermissionStatus::DeniedPermanent)
        ) {
            crate::warning!(
                "XR passthrough camera: sync blocked by permission state {:?}",
                self.passthrough_camera_permission
            );
            return;
        }

        let Some(choice) = self.passthrough_camera_choice.clone() else {
            crate::warning!("XR passthrough camera: sync waiting for camera choice");
            return;
        };

        self.passthrough_camera_source_size = vec2f(choice.width as f32, choice.height as f32);
        if self.passthrough_camera_textures.is_none() {
            self.passthrough_camera_textures = Some(XrPassthroughCameraTextures {
                camera: Texture::new_with_format(cx, TextureFormat::VideoExternal),
                tex_y: None,
                tex_u: None,
                tex_v: None,
            });
        }
        if self.passthrough_camera_failed || self.passthrough_camera_playback_requested {
            return;
        }

        cx.prepare_headset_camera_playback(
            Self::passthrough_video_id(),
            VideoSource::Camera(choice.input_id, choice.format_id),
            CameraPreviewMode::Texture,
            0,
            self.passthrough_camera_textures
                .as_ref()
                .map(|textures| textures.camera.texture_id())
                .unwrap_or_default(),
            false,
            false,
        );
        self.passthrough_camera_playback_requested = true;
    }

    fn upsert_passthrough_env_atlas_geometry(
        &mut self,
        cx: &mut Cx2d,
        width: f64,
        height: f64,
    ) -> GeometryId {
        let corners = [
            [0.0f32, 0.0f32, 0.0f32],
            [width as f32, 0.0f32, 0.0f32],
            [width as f32, height as f32, 0.0f32],
            [0.0f32, height as f32, 0.0f32],
        ];
        let normal = [0.0, 0.0, 1.0];
        let tangent = [1.0, 0.0, 0.0, 1.0];
        let color = [1.0, 1.0, 1.0, 1.0];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut vertices = Vec::with_capacity(4 * 16);
        for (corner, uv) in corners.iter().zip(uvs.iter()) {
            vertices.extend_from_slice(&[
                corner[0],
                corner[1],
                corner[2],
                normal[0],
                normal[1],
                normal[2],
                uv[0],
                uv[1],
                color[0],
                color[1],
                color[2],
                color[3],
                tangent[0],
                tangent[1],
                tangent[2],
                tangent[3],
            ]);
        }
        let indices = vec![0, 1, 2, 2, 3, 0, 0, 2, 1, 0, 3, 2];
        let geometry = self
            .passthrough_env_atlas_quad
            .get_or_insert_with(|| Geometry::new(cx.cx.cx));
        geometry.update(cx.cx.cx, indices, vertices);
        geometry.geometry_id()
    }

    fn current_head_basis(state: &XrState) -> (Vec3f, Vec3f, Vec3f, Vec3f) {
        let head = state.head_pose.position;
        let right = (state.vec_in_head_space(vec3(1.0, 0.0, 0.0)) - head).normalize();
        let up = (state.vec_in_head_space(vec3(0.0, 1.0, 0.0)) - head).normalize();
        let forward = (state.vec_in_head_space(vec3(0.0, 0.0, -1.0)) - head).normalize();
        (head, right, up, forward)
    }

    pub fn render_passthrough_env_atlas(
        &mut self,
        cx: &mut Cx2d,
        state: &XrState,
    ) -> Option<Texture> {
        let source_size = self.passthrough_camera_source_size;
        let rotation_steps = self.passthrough_camera_video.rotation_steps;
        let camera_enabled = if self.passthrough_camera_has_frame {
            1.0
        } else {
            0.0
        };
        let camera_texture = self
            .passthrough_camera_textures
            .as_ref()
            .map(|textures| textures.camera.clone())?;
        let atlas_width = XR_PASSTHROUGH_ENV_ATLAS_WIDTH as f64;
        let atlas_height = XR_PASSTHROUGH_ENV_ATLAS_HEIGHT as f64;
        let (_, camera_right, camera_up, camera_forward) = Self::current_head_basis(state);
        let camera_center_offset_uv = self.passthrough_camera_center_offset_uv();
        let geometry_id = self.upsert_passthrough_env_atlas_geometry(cx, atlas_width, atlas_height);

        let Self {
            passthrough_env_atlas,
            draw_passthrough_cube_atlas,
            ..
        } = self;
        let atlas =
            passthrough_env_atlas.get_or_insert_with(|| XrPassthroughEnvAtlas::new(cx.cx.cx));
        if atlas.pending_swap {
            atlas.finish_frame();
        }
        atlas.pass.set_size(cx.cx.cx, dvec2(atlas_width, atlas_height));
        let previous_texture = atlas.current_texture().clone();
        let display_texture = atlas.initialized.then_some(previous_texture.clone());
        let target_texture = atlas.target_texture().clone();
        let bootstrap_mix = if atlas.initialized { 0.0 } else { 1.0 };

        atlas.pass.set_color_texture(
            cx.cx.cx,
            &target_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );

        cx.make_child_pass(&atlas.pass);
        cx.begin_pass(&atlas.pass, Some(1.0));
        atlas.draw_list.begin_always(cx);

        draw_passthrough_cube_atlas.draw_vars.options.depth_write = false;
        draw_passthrough_cube_atlas.source_size = source_size;
        draw_passthrough_cube_atlas.camera_enabled = camera_enabled;
        draw_passthrough_cube_atlas.rotation_steps = rotation_steps;
        draw_passthrough_cube_atlas.bootstrap_mix = bootstrap_mix;
        draw_passthrough_cube_atlas.update_strength = XR_PASSTHROUGH_ENV_UPDATE_STRENGTH;
        draw_passthrough_cube_atlas.camera_fov_y_degrees =
            XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES;
        draw_passthrough_cube_atlas.camera_projection_scale =
            XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
        draw_passthrough_cube_atlas.camera_exposure = XR_PASSTHROUGH_CAMERA_EXPOSURE;
        draw_passthrough_cube_atlas.camera_center_offset_uv = camera_center_offset_uv;
        draw_passthrough_cube_atlas.camera_right = camera_right;
        draw_passthrough_cube_atlas.camera_up = camera_up;
        draw_passthrough_cube_atlas.camera_forward = camera_forward;
        draw_passthrough_cube_atlas
            .draw_vars
            .set_texture(0, &camera_texture);
        draw_passthrough_cube_atlas
            .draw_vars
            .set_texture(1, &previous_texture);
        draw_passthrough_cube_atlas.draw_geometry(cx, geometry_id);

        atlas.draw_list.end(cx);
        cx.end_pass(&atlas.pass);
        atlas.pending_swap = true;
        display_texture
    }

    pub fn draw_reflective_rounded_cube(
        &mut self,
        cx: &mut Cx2d,
        state: &XrState,
        env_atlas: Option<Texture>,
        pose: Pose,
        half_extents: Vec3f,
        corner_radius: f32,
        base_color: Vec4f,
        metallic: f32,
        roughness: f32,
        spec_strength: f32,
        env_intensity: f32,
        ambient: f32,
        light_color: Vec3f,
    ) {
        self.prepare_pbr(cx);
        self.draw_pbr.camera_pos = state.head_pose.position;
        if let Some(env_atlas) = env_atlas {
            self.draw_pbr.set_env_texture(None);
            self.draw_pbr.set_env_atlas_texture(Some(env_atlas));
        } else {
            let env_tex = self.draw_pbr.default_env_texture(cx);
            self.draw_pbr.set_env_texture(Some(env_tex));
            self.draw_pbr.set_env_atlas_texture(None);
        }
        self.draw_pbr.ambient = ambient;
        self.draw_pbr.spec_strength = spec_strength;
        self.draw_pbr.env_intensity = env_intensity;
        self.draw_pbr.light_color = light_color;
        self.draw_pbr.set_base_color_factor(base_color);
        self.draw_pbr.set_metal_roughness(metallic, roughness);
        self.draw_pbr.set_transform(pose.to_mat4());
        let _ = self.draw_pbr.draw_rounded_cube(
            cx,
            vec3(half_extents.x, half_extents.y, half_extents.z),
            corner_radius,
            XR_PBR_FACE_SUBDIVISIONS,
            XR_PBR_CORNER_SEGMENTS,
        );
    }

    pub fn draw_refractive_rounded_cube(
        &mut self,
        cx: &mut Cx2d,
        state: &XrState,
        env_atlas: Option<Texture>,
        pose: Pose,
        half_extents: Vec3f,
        corner_radius: f32,
        base_color: Vec4f,
        roughness: f32,
        spec_strength: f32,
        env_intensity: f32,
        focus_distance: f32,
    ) {
        self.prepare_refractive_pbr(cx);
        self.draw_pbr_refractive.source_size = self.passthrough_camera_source_size;
        self.draw_pbr_refractive.camera_pos = state.head_pose.position;
        self.draw_pbr_refractive.camera_enabled = if self.passthrough_camera_has_frame {
            1.0
        } else {
            0.0
        };
        self.draw_pbr_refractive.rotation_steps = self.passthrough_camera_video.rotation_steps;
        self.draw_pbr_refractive.camera_fov_y_degrees =
            XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES;
        self.draw_pbr_refractive.camera_projection_scale =
            XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
        self.draw_pbr_refractive.camera_exposure = XR_PASSTHROUGH_CAMERA_EXPOSURE;
        self.draw_pbr_refractive.camera_center_offset_uv =
            self.passthrough_camera_center_offset_uv();
        let cube_transform = pose.to_mat4();
        self.draw_pbr_refractive.object_center = pose.position;
        self.draw_pbr_refractive.object_right = cube_transform
            .transform_vec4(vec4f(1.0, 0.0, 0.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr_refractive.object_up = cube_transform
            .transform_vec4(vec4f(0.0, 1.0, 0.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr_refractive.object_forward = cube_transform
            .transform_vec4(vec4f(0.0, 0.0, 1.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr_refractive.object_half_extents = half_extents;
        self.draw_pbr_refractive.object_corner_radius = corner_radius;
        self.draw_pbr_refractive.transmission_focus_distance = focus_distance;
        self.draw_pbr_refractive.set_depth_write(true);
        self.draw_pbr_refractive.set_camera_texture(
            self.passthrough_camera_textures
                .as_ref()
                .map(|textures| textures.camera.clone()),
        );
        if let Some(env_atlas) = env_atlas {
            self.draw_pbr_refractive.set_env_texture(None);
            self.draw_pbr_refractive
                .set_env_atlas_texture(Some(env_atlas));
        } else {
            let env_tex = self.draw_pbr_refractive.default_env_texture(cx);
            self.draw_pbr_refractive.set_env_texture(Some(env_tex));
            self.draw_pbr_refractive.set_env_atlas_texture(None);
        }
        self.draw_pbr_refractive.ambient = 0.002;
        self.draw_pbr_refractive.spec_strength = spec_strength;
        self.draw_pbr_refractive.env_intensity = env_intensity;
        self.draw_pbr_refractive.light_color = vec3(0.10, 0.10, 0.10);
        self.draw_pbr_refractive.set_base_color_factor(base_color);
        self.draw_pbr_refractive.set_metal_roughness(0.0, roughness);
        self.draw_pbr_refractive.set_transform(cube_transform);
        let _ = self.draw_pbr_refractive.draw_rounded_cube(
            cx,
            vec3(half_extents.x, half_extents.y, half_extents.z),
            corner_radius,
            XR_PBR_FACE_SUBDIVISIONS,
            XR_PBR_CORNER_SEGMENTS,
        );
    }
}
