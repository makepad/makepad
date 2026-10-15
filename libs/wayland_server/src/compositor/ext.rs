//! Small protocol extensions: viewporter, fractional scale, cursor shape, server-side decoration.
use super::*;

impl Compositor {
    pub(crate) fn ext_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &[Value]) -> Result<(), Fatal> {
        match o.iface.name {
            "wp_viewporter" => if op == wp_viewporter::REQ_GET_VIEWPORT {
                let (id, surf) = (a[0].as_uint(), a[1].as_uint());
                let Some(s) = self.surface((c, surf)) else { return Ok(()) };
                if s.viewport_obj.is_some() { return Err(Fatal::new(o.id, wp_viewporter::ERROR_VIEWPORT_EXISTS, "surface already has a viewport")); }
                s.viewport_obj = Some(id);
                self.objs.insert((c, id), Obj::Viewport { surface: surf });
            },
            "wp_viewport" => {
                let Some(Obj::Viewport { surface }) = self.objs.get(&(c, o.id)) else { return Ok(()) };
                let surf = *surface;
                let Some(s) = self.surface((c, surf)) else {
                    if op == wp_viewport::REQ_DESTROY { return Ok(()); }
                    return Err(Fatal::new(o.id, wp_viewport::ERROR_NO_SURFACE, "the surface is gone"));
                };
                match op {
                    wp_viewport::REQ_SET_SOURCE => {
                        let v: Vec<f64> = a[0..4].iter().map(|x| x.as_f64()).collect();
                        if v.iter().all(|&x| x == -1.0) { s.pending_viewport().src = None; }
                        else if v[0] < 0.0 || v[1] < 0.0 || v[2] <= 0.0 || v[3] <= 0.0 { return Err(Fatal::new(o.id, wp_viewport::ERROR_BAD_VALUE, "bad source rectangle")); }
                        else { s.pending_viewport().src = Some((v[0], v[1], v[2], v[3])); }
                    }
                    wp_viewport::REQ_SET_DESTINATION => {
                        let (w, h) = (a[0].as_int(), a[1].as_int());
                        if (w, h) == (-1, -1) { s.pending_viewport().dst = None; }
                        else if w <= 0 || h <= 0 { return Err(Fatal::new(o.id, wp_viewport::ERROR_BAD_VALUE, "bad destination size")); }
                        else { s.pending_viewport().dst = Some((w, h)); }
                    }
                    _ => {}
                }
            }
            "wp_fractional_scale_manager_v1" => if op == wp_fractional_scale_manager_v1::REQ_GET_FRACTIONAL_SCALE {
                let id = a[0].as_uint();
                self.objs.insert((c, id), Obj::FractionalScale);
                self.fractional.push((c, id));
                let _ = core.send(c, id, wp_fractional_scale_v1::EVT_PREFERRED_SCALE, vec![Value::Uint((self.scale * 120.0).round() as u32)]);
            },
            "wp_cursor_shape_manager_v1" => match op {
                wp_cursor_shape_manager_v1::REQ_GET_POINTER | wp_cursor_shape_manager_v1::REQ_GET_TABLET_TOOL_V2 => { self.objs.insert((c, a[0].as_uint()), Obj::CursorDevice); }
                _ => {}
            },
            "wp_cursor_shape_device_v1" => if op == wp_cursor_shape_device_v1::REQ_SET_SHAPE {
                let shape = a[1].as_uint();
                if !(1..=34).contains(&shape) { return Err(Fatal::new(o.id, wp_cursor_shape_device_v1::ERROR_INVALID_SHAPE, "unknown cursor shape")); }
                // Only the client that has the pointer may set it.
                if self.input.pointer_focus.is_some_and(|k| k.0 == c) { self.events.push(UiEvent::SetCursorShape { client: c, shape }); }
            },
            "zxdg_decoration_manager_v1" => if op == zxdg_decoration_manager_v1::REQ_GET_TOPLEVEL_DECORATION {
                let id = a[0].as_uint();
                self.objs.insert((c, id), Obj::Decoration);
                // The WM frames its windows: always server side.
                let _ = core.send(c, id, zxdg_toplevel_decoration_v1::EVT_CONFIGURE, vec![Value::Uint(zxdg_toplevel_decoration_v1::MODE_SERVER_SIDE)]);
            },
            "zxdg_toplevel_decoration_v1" => if matches!(op, zxdg_toplevel_decoration_v1::REQ_SET_MODE | zxdg_toplevel_decoration_v1::REQ_UNSET_MODE) {
                let _ = core.send(c, o.id, zxdg_toplevel_decoration_v1::EVT_CONFIGURE, vec![Value::Uint(zxdg_toplevel_decoration_v1::MODE_SERVER_SIDE)]);
            },
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn ext_gone(&mut self, c: ClientId, o: ObjRef) {
        let k = (c, o.id);
        match self.objs.remove(&k) {
            Some(Obj::Viewport { surface }) => {
                if let Some(s) = self.surface((c, surface)) { s.viewport_obj = None; *s.pending_viewport() = Default::default(); }
            }
            Some(Obj::FractionalScale { .. }) => self.fractional.retain(|&f| f != k),
            _ => {}
        }
    }

    /// The output's scale changed: tell fractional-scale objects, surfaces and the wl_output.
    pub fn set_scale(&mut self, core: &mut Core, scale: f64) {
        if (self.scale - scale).abs() < 1e-9 { return; }
        self.scale = scale;
        self.output.scale = scale.ceil().max(1.0) as i32;
        for (c, id) in self.fractional.clone() {
            let _ = core.send(c, id, wp_fractional_scale_v1::EVT_PREFERRED_SCALE, vec![Value::Uint((scale * 120.0).round() as u32)]);
        }
        let surfaces: Vec<SurfKey> = self.objs.iter().filter(|(_, o)| matches!(o, Obj::Surface(_))).map(|(k, _)| *k).collect();
        for k in surfaces {
            let _ = core.send(k.0, k.1, wl_surface::EVT_PREFERRED_BUFFER_SCALE, vec![Value::Int(self.output.scale)]);
        }
        for (c, id) in self.outputs.clone() {
            if let Some(o) = core.obj(c, id) { self.send_output(core, c, id, o.version); }
        }
    }
}
