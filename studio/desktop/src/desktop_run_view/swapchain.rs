use super::*;
use crate::makepad_widgets::makepad_platform::shared_framebuf::shared_swapchain_from_host_swapchain;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use crate::makepad_widgets::makepad_platform::shared_framebuf::aux_chan;

impl DesktopRunView {
    pub(crate) fn try_present_draw(
        &mut self,
        cx: &mut Cx,
        presentable_draw: PresentableDraw,
    ) -> bool {
        if let Some(swapchain) = self.swapchain.as_ref() {
            if Self::apply_presentable_draw_to_quad(
                cx,
                &mut self.draw_app,
                &mut self.redraw_countdown,
                presentable_draw,
                swapchain,
            ) {
                self.last_swapchain_with_completed_draws = None;
                self.redraw(cx);
                return true;
            }
        }
        if let Some(swapchain) = self.last_swapchain_with_completed_draws.as_ref() {
            if Self::apply_presentable_draw_to_quad(
                cx,
                &mut self.draw_app,
                &mut self.redraw_countdown,
                presentable_draw,
                swapchain,
            ) {
                self.redraw(cx);
                return true;
            }
        }
        false
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub(crate) fn setup_aux_chan(&mut self, studio_addr: Option<&str>, build_id: QueryId) {
        // Only create the listener once per target
        if self.aux_chan_host_endpoint.is_some() {
            return;
        }
        let Some(studio_addr) = studio_addr else {
            return;
        };
        let listener = match aux_chan::ExternalEndpointListener::new_for_studio(
            studio_addr,
            &build_id.0.to_string(),
        ) {
            Ok(l) => l,
            Err(err) => {
                log!("aux_chan listener failed: {}", err);
                return;
            }
        };
        let slot = Arc::new(std::sync::Mutex::new(None));
        self.aux_chan_host_endpoint = Some(slot.clone());
        // Accept in background — the child may take a long time to compile and start.
        std::thread::Builder::new()
            .name("aux-chan-accept".into())
            .spawn(move || match listener.accept_host_endpoint() {
                Ok(endpoint) => {
                    *slot.lock().unwrap() = Some(endpoint);
                }
                Err(err) => {
                    crate::log!("aux_chan accept failed: {}", err);
                }
            })
            .ok();
    }

    pub(crate) fn ensure_swapchain_for_rect(
        &mut self,
        cx: &mut Cx,
        rect: Rect,
        dpi_factor: f64,
        target: RunTarget,
    ) {
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }

        let min_width = ((rect.size.x * dpi_factor).ceil() as u32).max(1);
        let min_height = ((rect.size.y * dpi_factor).ceil() as u32).max(1);
        let needs_new_swapchain = self
            .swapchain
            .as_ref()
            .map(|swapchain| {
                #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                {
                    min_width != swapchain.alloc_width
                        || min_height != swapchain.alloc_height
                        || swapchain.window_id != target.window_id
                }
                #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
                {
                    min_width > swapchain.alloc_width
                        || min_height > swapchain.alloc_height
                        || swapchain.window_id != target.window_id
                }
            })
            .unwrap_or(true);

        let rect_changed = self.last_rect != rect || self.last_dpi_factor != dpi_factor;
        if needs_new_swapchain {
            if self.last_swapchain_with_completed_draws.is_none() {
                self.last_swapchain_with_completed_draws = self.swapchain.take();
            } else {
                self.swapchain = None;
            }

            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            let (alloc_width, alloc_height) = (min_width.max(1), min_height.max(1));
            #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
            let (alloc_width, alloc_height) = (
                min_width.max(64).next_power_of_two(),
                min_height.max(64).next_power_of_two(),
            );

            self.swapchain = Some(HostSwapchain::new(
                target.window_id,
                alloc_width,
                alloc_height,
                cx,
            ));
        }

        if rect_changed || needs_new_swapchain {
            self.bootstrap_pending = true;
            self.bootstrap_tick_count = 0;
        }

        if std::env::var_os("MAKEPAD_RUNVIEW_DPI_TRACE").is_some()
            && (rect_changed || needs_new_swapchain)
        {
            log!(
                "runview host ensure build={} window={} rect=({}, {}) dpi={} min_px=({}, {}) new_swapchain={} alloc={:?}",
                target.build_id.0,
                target.window_id,
                rect.size.x,
                rect.size.y,
                dpi_factor,
                min_width,
                min_height,
                needs_new_swapchain,
                self.swapchain
                    .as_ref()
                    .map(|swapchain| (swapchain.alloc_width, swapchain.alloc_height))
            );
        }

        self.last_rect = rect;
        self.last_dpi_factor = dpi_factor;
    }

    pub(crate) fn build_bootstrap_msgs(
        &mut self,
        cx: &mut Cx,
        target: RunTarget,
    ) -> Vec<StudioToApp> {
        if self.last_rect.size.x <= 0.0 || self.last_rect.size.y <= 0.0 {
            return Vec::new();
        }

        let mut outbound = vec![StudioToApp::WindowGeomChange {
            window_id: target.window_id,
            dpi_factor: self.last_dpi_factor,
            left: 0.0,
            top: 0.0,
            width: self.last_rect.size.x,
            height: self.last_rect.size.y,
        }];

        if std::env::var_os("MAKEPAD_RUNVIEW_DPI_TRACE").is_some() {
            let trace_bootstrap = (
                target.build_id.0,
                target.window_id,
                self.last_rect.size.x.to_bits(),
                self.last_rect.size.y.to_bits(),
                self.last_dpi_factor.to_bits(),
            );
            if self.last_trace_bootstrap != Some(trace_bootstrap) {
                self.last_trace_bootstrap = Some(trace_bootstrap);
                log!(
                    "runview host bootstrap build={} window={} logical=({}, {}) dpi={} px=({}, {})",
                    target.build_id.0,
                    target.window_id,
                    self.last_rect.size.x,
                    self.last_rect.size.y,
                    self.last_dpi_factor,
                    self.last_rect.size.x * self.last_dpi_factor,
                    self.last_rect.size.y * self.last_dpi_factor
                );
            }
        }

        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            if !self.app_ready_for_swapchain {
                return outbound;
            }
            let Some(endpoint_slot) = self.aux_chan_host_endpoint.as_ref() else {
                return outbound;
            };
            let endpoint_guard = endpoint_slot.lock().unwrap();
            let Some(host_endpoint) = endpoint_guard.as_ref() else {
                return outbound;
            };
            if let Some(swapchain) = self.swapchain.as_mut() {
                match shared_swapchain_from_host_swapchain(swapchain, cx, host_endpoint) {
                    Ok(shared) => outbound.push(StudioToApp::Swapchain(shared)),
                    Err(err) => log!("swapchain share failed: {:?}", err),
                }
            }
        }
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        {
            // Keep websocket-only targets, such as Android app sockets, on the
            // remote frame path unless the app has explicitly signaled stdin-loop
            // style readiness via RunViewCreated.
            if !self.app_ready_for_swapchain {
                return outbound;
            }
            if let Some(swapchain) = self.swapchain.as_ref() {
                let shared_swapchain = shared_swapchain_from_host_swapchain(swapchain, cx);
                outbound.push(StudioToApp::Swapchain(shared_swapchain));
            }
        }

        outbound
    }

    pub(crate) fn set_presentable_draw(&mut self, cx: &mut Cx, presentable_draw: PresentableDraw) {
        if self.try_present_draw(cx, presentable_draw) {
            self.pending_draw = None;
            self.debug_present_ok_count += 1;
            self.bootstrap_pending = false;
            self.bootstrap_tick_count = 0;
            self.remote_mode = false;
            self.remote_frame_request_in_flight = false;
            self.remote_requested_frame_id = None;
        } else {
            self.pending_draw = Some(presentable_draw);
        }
    }

    fn apply_presentable_draw_to_quad(
        cx: &mut Cx,
        draw_app: &mut DrawQuad,
        redraw_countdown: &mut usize,
        presentable_draw: PresentableDraw,
        swapchain: &HostSwapchain,
    ) -> bool {
        // Ignore zero-sized frames from early startup races (before geom is applied).
        // Treating these as "presented" can stall bootstrap until a manual resize.
        if presentable_draw.width == 0 || presentable_draw.height == 0 {
            return false;
        }

        let Some(drawn) = swapchain.get_image(presentable_draw.target_id) else {
            return false;
        };

        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        if let Some(buffer) = drawn.software_buffer.as_ref() {
            cx.upload_presentable_image_software_buffer(
                &drawn.texture,
                swapchain.alloc_width,
                swapchain.alloc_height,
                buffer.as_bytes(),
            );
        }

        draw_app.set_texture(0, &drawn.texture);
        draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(tex_scale),
            &[
                (presentable_draw.width as f32) / (swapchain.alloc_width as f32),
                (presentable_draw.height as f32) / (swapchain.alloc_height as f32),
            ],
        );
        draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(tex_size),
            &[
                (swapchain.alloc_width as f32),
                (swapchain.alloc_height as f32),
            ],
        );
        draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(y_flip), &[1.0f32]);
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(y_flip), &[0.0f32]);

        *redraw_countdown = (*redraw_countdown).max(20);
        true
    }
}
