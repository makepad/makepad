use crate::{
    app::AppData,
    build_manager::build_manager::BuildManager,
    makepad_platform::os::shared_framebuf::*,
    makepad_platform::studio::{RemoteKeyModifiers, RemoteMouseMove, RemoteTweakRay, StudioToApp, TweakHitsResponse},
    tweak_view::TweakView,
    makepad_widgets::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RunViewBase = #(RunView::register_widget(vm))

    mod.widgets.RunView = set_type_default() do mod.widgets.RunViewBase {
        draw_app +: {
            tex: texture_2d(float)
            inactive: instance(0.0)
            started: instance(0.0)
            tex_scale: instance(vec2(0.0, 0.0))
            tex_size: instance(vec2(0.0, 0.0))
            y_flip: instance(0.0)
            pixel: fn() {
                let tp1 = self.tex.sample(vec2(0.5/self.tex_size.x,0.5/self.tex_size.y))
                let tp2 = self.tex.sample(vec2(1.5/self.tex_size.x,0.5/self.tex_size.y))
                let tp = vec2(tp1.r*65280.0 + tp1.b*255.0,tp2.r*65280.0 + tp2.b*255.0)
                let counter = (self.rect_size * self.draw_pass.dpi_factor) / tp
                let tex_scale = tp / self.tex_size
                let uv = vec2(self.pos.x, self.pos.y + self.y_flip - 2.0 * self.y_flip * self.pos.y)
                let fb = self.tex.sample(uv * tex_scale * counter)
                if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                    return #2
                }
                return fb.mix(#4, self.inactive * 0.4)
            }
        }
        draw_click +: {
            dot_radius: instance(5.0)
            dot_alpha: instance(0.0)
            ripple_radius: instance(5.0)
            ripple_alpha: instance(0.0)
            color: uniform(#x00d4ff)
            pixel: fn() {
                if self.dot_alpha <= 0.001 && self.ripple_alpha <= 0.001 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let dot_r = self.dot_radius.min(self.rect_size.x * 0.5).min(self.rect_size.y * 0.5)
                if self.dot_alpha > 0.001 {
                    sdf.circle(c.x, c.y, dot_r)
                    sdf.fill(vec4(self.color.xyz, self.dot_alpha))
                }
                if self.ripple_alpha > 0.001 {
                    let ripple_r = self.ripple_radius.min(self.rect_size.x * 0.5).min(self.rect_size.y * 0.5)
                    sdf.circle(c.x, c.y, ripple_r)
                    sdf.stroke(vec4(self.color.xyz, self.ripple_alpha), 1.5)
                }
                return sdf.result
            }
        }
        tweak_view: TweakView {
            draw_vector+: {
                ..mod.draw.DrawVector
            }
        }
        animator: Animator {
            started: {
                default: @off
                off: AnimatorState {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_app: {started: 0.0}}
                }
                on: AnimatorState {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_app: {started: 1.0}}
                }
            }
            inactive: {
                default: @off
                off: AnimatorState {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_app: {inactive: 0.0}}
                }
                on: AnimatorState {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_app: {inactive: 1.0}}
                }
            }
            click: {
                default: @off
                off: AnimatorState {
                    from: {all: Snap}
                    apply: {draw_click: {dot_radius: 5.0 dot_alpha: 0.0 ripple_radius: 5.0 ripple_alpha: 0.0}}
                }
                down: AnimatorState {
                    from: {all: Snap}
                    apply: {draw_click: {dot_radius: 5.0 dot_alpha: 0.95 ripple_radius: 5.0 ripple_alpha: 0.45}}
                }
                up: AnimatorState {
                    from: {all: Forward {duration: 0.5}}
                    apply: {draw_click: {dot_radius: 5.0 dot_alpha: 0.0 ripple_radius: 22.0 ripple_alpha: 0.0}}
                }
            }
        }
    }
}

#[derive(Script, Widget, Animator)]
pub struct RunView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[apply_default]
    animator: Animator,
    #[redraw]
    #[live]
    draw_app: DrawQuad,
    #[live]
    draw_click: DrawQuad,
    #[live]
    tweak_view: TweakView,
    #[rust]
    last_rect: Rect,
    #[rust]
    click_pos: Vec2d,
    #[rust(100usize)]
    redraw_countdown: usize,
    #[rust]
    started: bool,
    #[rust]
    pub build_id: Option<LiveId>,
    #[rust]
    pub window_id: usize,
    #[rust(WindowKindId::Main)]
    pub kind_id: WindowKindId,
}

impl ScriptHook for RunView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.draw_app.set_texture(0, &cx.null_texture());
        });
    }
}

impl RunView {
    // TODO: Old LiveHook pattern - needs adaptation for new system
    // fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
    //     if let ApplyFrom::UpdateFromDoc{..} = apply.from{
    //         self.last_rect = Default::default();
    //         self.animator_cut(cx, ids!(started.on));
    //     }
    // }

    pub fn draw_complete_and_flip(
        &mut self,
        cx: &mut Cx,
        presentable_draw: &PresentableDraw,
        manager: &mut BuildManager,
    ) {
        let window_id = self.window_id;
        if self.build_id.is_none() {
            return;
        }
        if let Some(v) = manager
            .active
            .builds
            .get_mut(self.build_id.as_ref().unwrap())
        {
            // Only allow presenting images in the current host swapchain
            // (or the previous one, before any draws on the current one),
            // and look them up by their unique IDs, to avoid rendering
            // different textures than the ones the client just drew to.
            let mut try_present_through = |swapchain: &Option<shared_framebuf::HostSwapchain>| {
                let swapchain = swapchain.as_ref()?;
                let drawn = swapchain.get_image(presentable_draw.target_id)?;

                #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                if let Some(buffer) = drawn.software_buffer.as_ref() {
                    cx.upload_presentable_image_software_buffer(
                        &drawn.texture,
                        swapchain.alloc_width,
                        swapchain.alloc_height,
                        buffer.as_bytes(),
                    );
                }

                self.draw_app.set_texture(0, &drawn.texture);
                self.draw_app.draw_vars.set_dyn_instance(
                    cx,
                    id!(tex_scale),
                    &[
                        (presentable_draw.width as f32) / (swapchain.alloc_width as f32),
                        (presentable_draw.height as f32) / (swapchain.alloc_height as f32),
                    ],
                );
                self.draw_app.draw_vars.set_dyn_instance(
                    cx,
                    id!(tex_size),
                    &[
                        (swapchain.alloc_width as f32),
                        (swapchain.alloc_height as f32),
                    ],
                );
                #[cfg(target_os = "linux")]
                self.draw_app.draw_vars.set_dyn_instance(
                    cx, id!(y_flip), &[1.0f32],
                );

                if !self.started {
                    self.started = true;
                    self.animator_play(cx, ids!(started.on));
                }
                self.redraw_countdown = 20;
                self.redraw(cx);
                Some(())
            };

            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            let current_uses_software_fallback = v
                .swapchain_mut(window_id)
                .as_ref()
                .map(|swapchain| {
                    swapchain
                        .presentable_images
                        .iter()
                        .any(|image| image.software_buffer.is_some())
                })
                .unwrap_or(false);
            #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
            let current_uses_software_fallback = false;

            if try_present_through(&v.swapchain_mut(window_id)).is_some() {
                // The client is now drawing to the current swapchain,
                // we can discard any previous one we were stashing.
                *v.last_swapchain_with_completed_draws_mut(window_id) = None;
            } else {
                if current_uses_software_fallback {
                    // During software fallback resize churn, showing stale frames from
                    // previous swapchains causes visible partial/garbled regions.
                    // Prefer waiting for current-swapchain frames.
                    return;
                }
                // New draws to a previous swapchain are fine, just means
                // the client hasn't yet drawn on the current swapchain,
                // what lets us accept draws is their target `Texture`s.
                try_present_through(&v.last_swapchain_with_completed_draws_mut(window_id));
            }
        }
    }

    pub fn ready_to_start(&mut self, cx: &mut Cx) {
        self.animator_play(cx, ids!(inactive.off));
        // cause a resize event to fire
        self.last_rect = Default::default();
        self.redraw(cx);
    }

    pub fn websocket_disconnect(&mut self, cx: &mut Cx) {
        self.animator_play(cx, ids!(inactive.on));
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_app.redraw(cx);
    }

    pub fn request_animation_frame(&mut self, cx: &mut Cx) {
        // Keep repainting to ensure ticks are sent to the child.
        // Use the same value as draw_complete_and_flip (20) since both indicate
        // the child is actively animating and needs consistent tick delivery.
        // This provides enough buffer to account for message round-trip latency
        // and ensures smooth animation even when the host is otherwise idle.
        if self.redraw_countdown < 120 {
            self.redraw_countdown = 120;
        }
        self.redraw(cx);
    }

    pub fn resend_framebuffer(&mut self, _cx: &mut Cx) {
        self.last_rect = Default::default();
    }

    pub fn ai_click_viz(&mut self, cx: &mut Cx, x: f64, y: f64, is_down: bool) {
        let abs = dvec2(self.last_rect.pos.x + x, self.last_rect.pos.y + y);
        if !self.last_rect.contains(abs) {
            return;
        }
        self.click_pos = abs;
        if is_down {
            self.animator_play(cx, ids!(click.down));
        } else {
            self.animator_play(cx, ids!(click.up));
        }
        self.redraw(cx);
    }

    pub fn set_tweak_hits(&mut self, cx: &mut Cx, hits: &TweakHitsResponse) {
        self.tweak_view.set_hits(cx, hits);
    }

    pub fn draw_run_view(
        &mut self,
        cx: &mut Cx2d,
        run_view_id: LiveId,
        manager: &mut BuildManager,
        walk: Walk,
    ) {
        if self.build_id.is_none() {
            return;
        }
        // alright so here we draw em texturezs
        // pick a texture off the buildstate
        let dpi_factor = cx.current_dpi_factor();
        let rect = cx.walk_turtle(walk).dpi_snap(dpi_factor);
        // lets pixelsnap rect in position and size
        if self.redraw_countdown > 0 {
            self.redraw_countdown -= 1;
            self.redraw(cx);
        }
        if self.last_rect != rect {
            manager.send_host_to_stdin(
                run_view_id,
                StudioToApp::WindowGeomChange {
                    window_id: self.window_id,
                    dpi_factor,
                    left: 0.0,
                    top: 0.0,
                    width: rect.size.x,
                    height: rect.size.y,
                },
            );
        }

        if self.last_rect.size != rect.size {
            let min_width = ((rect.size.x * dpi_factor).ceil() as u32).max(1);
            let min_height = ((rect.size.y * dpi_factor).ceil() as u32).max(1);

            let active_build_needs_new_swapchain = manager
                .active
                .builds
                .get_mut(&run_view_id)
                .filter(|v| {
                    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                    {
                        v.aux_chan_host_endpoint.is_some()
                    }
                    #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
                    {
                        let _ = v;
                        true
                    }
                })
                .filter(|v| {
                    v.swapchain(self.window_id)
                        .map(|swapchain| {
                            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                            {
                                // Keep Linux swapchains exact-sized to prevent
                                // alloc-vs-draw Y offsets in subprocess presentation.
                                min_width != swapchain.alloc_width
                                    || min_height != swapchain.alloc_height
                            }
                            #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
                            {
                                min_width > swapchain.alloc_width
                                    || min_height > swapchain.alloc_height
                            }
                        })
                        .unwrap_or(true)
                });

            if let Some(v) = active_build_needs_new_swapchain {
                #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                let aux_chan_host_endpoint = v
                    .aux_chan_host_endpoint
                    .clone()
                    .expect("missing Linux aux channel host endpoint");

                // HACK(eddyb) there is no check that there were any draws on
                // the current swapchain, but the absence of an older swapchain
                // (i.e. `last_swapchain_with_completed_draws`) implies either
                // zero draws so far, or a draw to the current one discarded it.
                if v.last_swapchain_with_completed_draws(self.window_id)
                    .is_none()
                {
                    let chain = v.swapchain_mut(self.window_id).take();
                    *v.last_swapchain_with_completed_draws_mut(self.window_id) = chain;
                }

                // `Texture`s can be reused, but all `PresentableImageId`s must
                // be regenerated, to tell apart swapchains when e.g. resizing
                // constantly, so textures keep getting created and replaced.
                if let Some(swapchain) = v.swapchain_mut(self.window_id) {
                    swapchain.regenerate_ids();
                }

                #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                let (alloc_width, alloc_height) = (min_width.max(1), min_height.max(1));
                #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
                let (alloc_width, alloc_height) = (
                    min_width.max(64).next_power_of_two(),
                    min_height.max(64).next_power_of_two(),
                );

                let swapchain = v.swapchain_mut(self.window_id).get_or_insert_with(|| {
                    shared_framebuf::HostSwapchain::new(self.window_id, alloc_width, alloc_height, cx)
                });

                // Create shared swapchain for cross-process serialization
                #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                let shared_swapchain = match shared_framebuf::SharedSwapchain::from_host_swapchain(
                    swapchain,
                    cx,
                    &aux_chan_host_endpoint,
                ) {
                    Ok(shared_swapchain) => shared_swapchain,
                    Err(shared_framebuf::SharedSwapchainCreateError::AuxChannelSend(err)) => {
                        crate::error!(
                            "Linux RunView aux channel send failed: {err:?}; disabling shared texture path"
                        );
                        v.aux_chan_host_endpoint = None;
                        return;
                    }
                    Err(shared_framebuf::SharedSwapchainCreateError::SoftwareFallback(err)) => {
                        crate::error!("Linux RunView software fallback setup failed: {err:?}");
                        v.aux_chan_host_endpoint = None;
                        return;
                    }
                };
                #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
                let shared_swapchain =
                    shared_framebuf::SharedSwapchain::from_host_swapchain(swapchain, cx);

                // Inform the client about the new swapchain it *should* use
                manager.send_host_to_stdin(run_view_id, StudioToApp::Swapchain(shared_swapchain));
            }
        }
        self.last_rect = rect;
        self.draw_app.draw_abs(cx, rect);
        let click_rect = Rect {
            pos: self.click_pos - dvec2(28.0, 28.0),
            size: dvec2(56.0, 56.0),
        };
        self.draw_click.draw_abs(cx, click_rect);
        self.tweak_view.draw_overlay(cx, rect);
        // lets store the area
        if let Some(ab) = manager.active.builds.get_mut(&run_view_id) {
            ab.app_area.insert(self.window_id, self.draw_app.area());
        }
    }
}

impl Widget for RunView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let path = cx.widget_tree().path_to(self.widget_uid());
        let run_view_id = path
            .last()
            .copied()
            .unwrap_or(LiveId(0))
            .sub(self.window_id as u64);
        let manager = &mut scope.data.get_mut::<AppData>().unwrap().build_manager;
        self.draw_run_view(cx, run_view_id, manager, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let path = cx.widget_tree().path_to(self.widget_uid());
        let run_view_id = path
            .last()
            .copied()
            .unwrap_or(LiveId(0))
            .sub(self.window_id as u64);
        let manager = &scope.data.get::<AppData>().unwrap().build_manager;

        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx);
        }
        if let Event::MouseMove(e) = event {
            let area = self.draw_app.area();
            if area.is_valid(cx) {
                let rect = area.rect(cx);
                if rect.contains(e.abs) {
                    let local_x = e.abs.x - rect.pos.x;
                    let local_y = e.abs.y - rect.pos.y;
                    e.handled.set(area);
                    if e.modifiers.logo {
                        manager.send_host_to_stdin(
                            run_view_id,
                            StudioToApp::TweakRay(RemoteTweakRay {
                                time: e.time,
                                x: local_x,
                                y: local_y,
                                modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                            }),
                        );
                    } else {
                        self.tweak_view.clear(cx);
                        manager.send_host_to_stdin(
                            run_view_id,
                            StudioToApp::MouseMove(RemoteMouseMove {
                                time: e.time,
                                x: local_x,
                                y: local_y,
                                modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                            }),
                        );
                    }
                }
            }
        }
        // lets send mouse events
        match event.hits(cx, self.draw_app.area()) {
            Hit::FingerDown(_) => {
                cx.set_key_focus(self.draw_app.area());
            }
            Hit::TextInput(e) => {
                manager.send_host_to_stdin(run_view_id, StudioToApp::TextInput(e));
            }
            Hit::KeyDown(e) => {
                manager.send_host_to_stdin(run_view_id, StudioToApp::KeyDown(e));
            }
            Hit::KeyUp(e) => {
                manager.send_host_to_stdin(run_view_id, StudioToApp::KeyUp(e));
            }
            Hit::TextCopy(_) => {
                manager.send_host_to_stdin(run_view_id, StudioToApp::TextCopy);
            }
            Hit::TextCut(_) => {
                manager.send_host_to_stdin(run_view_id, StudioToApp::TextCut);
            }
            _ => (),
        }
    }
}
