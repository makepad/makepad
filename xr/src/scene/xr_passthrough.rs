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

struct XrPassthroughEnvCubeFace {
    pass: DrawPass,
    draw_list: DrawList2d,
    initialized: bool,
}

pub(super) struct XrPassthroughEnvCube {
    texture: Texture,
    faces: [XrPassthroughEnvCubeFace; 6],
}

impl XrPassthroughEnvCube {
    fn new(cx: &mut Cx) -> Self {
        let face_size = XR_PASSTHROUGH_ENV_FACE_SIZE;
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderCubeBGRAu8 {
                size: TextureSize::Fixed {
                    width: face_size,
                    height: face_size,
                },
                initial: true,
            },
        );
        let faces = std::array::from_fn(|face| {
            let pass = DrawPass::new_with_name(cx, &format!("xr_passthrough_env_face_{face}"));
            pass.set_size(cx, dvec2(face_size as f64, face_size as f64));
            XrPassthroughEnvCubeFace {
                pass,
                draw_list: DrawList2d::new(cx),
                initialized: false,
            }
        });
        Self { texture, faces }
    }

    fn texture(&self) -> Texture {
        self.texture.clone()
    }

    pub(super) fn reset_state(&mut self) {
        for face in &mut self.faces {
            face.initialized = false;
        }
    }
}

impl XrPassthroughRuntime {
    pub(crate) fn camera_center_offset_uv(&self) -> Vec2f {
        let source_size = self.camera_source_size;
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

    pub(super) fn pick_camera_choice(ev: &VideoInputsEvent) -> Option<XrPassthroughCameraChoice> {
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

    pub(super) fn reset_camera_state(&mut self) {
        self.camera_playback_requested = false;
        self.camera_failed = false;
        self.camera_textures = None;
        self.camera_video = VideoYuvMetadata::disabled();
        self.camera_has_frame = false;
        if let Some(cube) = self.env_cube.as_mut() {
            cube.reset_state();
        }
    }

    pub(super) fn sync_camera(&mut self, cx: &mut Cx, video_id: LiveId) {
        if matches!(
            self.camera_permission,
            Some(PermissionStatus::DeniedCanRetry) | Some(PermissionStatus::DeniedPermanent)
        ) {
            crate::warning!(
                "XR passthrough camera: sync blocked by permission state {:?}",
                self.camera_permission
            );
            return;
        }

        let Some(choice) = self.camera_choice.clone() else {
            crate::warning!("XR passthrough camera: sync waiting for camera choice");
            return;
        };

        self.camera_source_size = vec2f(choice.width as f32, choice.height as f32);
        if self.camera_textures.is_none() {
            self.camera_textures = Some(XrPassthroughCameraTextures {
                camera: Texture::new_with_format(cx, TextureFormat::VideoExternal),
                tex_y: None,
                tex_u: None,
                tex_v: None,
            });
        }
        if self.camera_failed || self.camera_playback_requested {
            return;
        }

        cx.prepare_headset_camera_playback(
            video_id,
            VideoSource::Camera(choice.input_id, choice.format_id),
            CameraPreviewMode::Texture,
            0,
            self.camera_textures
                .as_ref()
                .map(|textures| textures.camera.texture_id())
                .unwrap_or_default(),
            false,
            false,
        );
        self.camera_playback_requested = true;
    }

    fn upsert_env_face_geometry(&mut self, cx: &mut Cx2d, size: f64) -> GeometryId {
        let corners = [
            [0.0f32, 0.0f32, 0.0f32],
            [size as f32, 0.0f32, 0.0f32],
            [size as f32, size as f32, 0.0f32],
            [0.0f32, size as f32, 0.0f32],
        ];
        let normal = [0.0, 0.0, 1.0];
        let tangent = [1.0, 0.0, 0.0, 1.0];
        let color = [1.0, 1.0, 1.0, 1.0];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut vertices = Vec::with_capacity(4 * 16);
        for (corner, uv) in corners.iter().zip(uvs.iter()) {
            vertices.extend_from_slice(&[
                corner[0], corner[1], corner[2], normal[0], normal[1], normal[2], uv[0], uv[1],
                color[0], color[1], color[2], color[3], tangent[0], tangent[1], tangent[2],
                tangent[3],
            ]);
        }
        let indices = vec![0, 1, 2, 2, 3, 0];
        let geometry = self
            .env_face_quad
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

    fn cube_face_from_dir(dir: Vec3f) -> usize {
        let ad = vec3f(dir.x.abs(), dir.y.abs(), dir.z.abs());
        if ad.x >= ad.y && ad.x >= ad.z {
            if dir.x >= 0.0 {
                0
            } else {
                1
            }
        } else if ad.y >= ad.z {
            if dir.y >= 0.0 {
                2
            } else {
                3
            }
        } else if dir.z >= 0.0 {
            4
        } else {
            5
        }
    }

    fn visible_env_faces(
        source_size: Vec2f,
        right: Vec3f,
        up: Vec3f,
        forward: Vec3f,
    ) -> Vec<usize> {
        let aspect = if source_size.y > 1.0 {
            source_size.x / source_size.y
        } else {
            4.0 / 3.0
        };
        let tan_half_y = (XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES.to_radians() * 0.5).tan()
            * XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
        let tan_half_x = tan_half_y * aspect.max(1.0);
        let directions = [
            forward,
            (forward + right * tan_half_x).normalize(),
            (forward - right * tan_half_x).normalize(),
            (forward + up * tan_half_y).normalize(),
            (forward - up * tan_half_y).normalize(),
            (forward + right * tan_half_x + up * tan_half_y).normalize(),
            (forward - right * tan_half_x + up * tan_half_y).normalize(),
            (forward + right * tan_half_x - up * tan_half_y).normalize(),
            (forward - right * tan_half_x - up * tan_half_y).normalize(),
        ];

        let mut counts = [0u8; 6];
        for (index, dir) in directions.iter().enumerate() {
            let weight = if index == 0 {
                3
            } else if index < 5 {
                2
            } else {
                1
            };
            counts[Self::cube_face_from_dir(*dir)] =
                counts[Self::cube_face_from_dir(*dir)].saturating_add(weight);
        }

        let mut ranked = counts
            .iter()
            .enumerate()
            .filter_map(|(face, count)| (*count > 0).then_some((face, *count)))
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(3);
        ranked.into_iter().map(|(face, _)| face).collect()
    }

    pub fn render_env_cube(
        &mut self,
        draw_passthrough_env_face: &mut DrawPassthroughEnvFace,
        cx: &mut Cx2d,
        state: &XrState,
    ) -> Option<Texture> {
        let source_size = self.camera_source_size;
        let rotation_steps = self.camera_video.rotation_steps;
        let camera_enabled = if self.camera_has_frame { 1.0 } else { 0.0 };
        let camera_texture = self
            .camera_textures
            .as_ref()
            .map(|textures| textures.camera.clone())?;
        let face_size = XR_PASSTHROUGH_ENV_FACE_SIZE as f64;
        let (_, camera_right, camera_up, camera_forward) = Self::current_head_basis(state);
        let camera_center_offset_uv = self.camera_center_offset_uv();
        let geometry_id = self.upsert_env_face_geometry(cx, face_size);
        let visible_faces =
            Self::visible_env_faces(source_size, camera_right, camera_up, camera_forward);

        let cube = self
            .env_cube
            .get_or_insert_with(|| XrPassthroughEnvCube::new(cx.cx.cx));

        for face_index in visible_faces {
            let face = &mut cube.faces[face_index];
            face.pass.set_size(cx.cx.cx, dvec2(face_size, face_size));
            let clear_color = if face.initialized {
                DrawPassClearColor::InitWith(vec4(0.0, 0.0, 0.0, 0.0))
            } else {
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0))
            };
            face.pass.set_color_texture_face(
                cx.cx.cx,
                &cube.texture,
                face_index as u32,
                clear_color,
            );

            cx.make_child_pass(&face.pass);
            cx.begin_pass(&face.pass, Some(1.0));
            face.draw_list.begin_always(cx);

            draw_passthrough_env_face.draw_vars.options.depth_write = false;
            draw_passthrough_env_face.source_size = source_size;
            draw_passthrough_env_face.camera_enabled = camera_enabled;
            draw_passthrough_env_face.rotation_steps = rotation_steps;
            draw_passthrough_env_face.bootstrap_mix = if face.initialized { 0.0 } else { 1.0 };
            draw_passthrough_env_face.update_strength = XR_PASSTHROUGH_ENV_UPDATE_STRENGTH;
            draw_passthrough_env_face.face_index = face_index as f32;
            draw_passthrough_env_face.camera_fov_y_degrees =
                XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES;
            draw_passthrough_env_face.camera_projection_scale =
                XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
            draw_passthrough_env_face.camera_exposure = XR_PASSTHROUGH_CAMERA_EXPOSURE;
            draw_passthrough_env_face.camera_center_offset_uv = camera_center_offset_uv;
            draw_passthrough_env_face.camera_right = camera_right;
            draw_passthrough_env_face.camera_up = camera_up;
            draw_passthrough_env_face.camera_forward = camera_forward;
            draw_passthrough_env_face
                .draw_vars
                .set_texture(0, &camera_texture);
            draw_passthrough_env_face.draw_geometry(cx, geometry_id);

            face.draw_list.end(cx);
            cx.end_pass(&face.pass);
            face.initialized = true;
        }

        Some(cube.texture())
    }
}
