use super::*;
use crate::mobile::{self, PhoneHit, PhoneScreen};
use crate::mobile_tiles::{Face, TILE_RADIUS};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    set_type_default() do #(DrawPhoneApp::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        opacity: 1.0 radius: 0.0 y_flip: 0.0
        // The part of the texture this rect shows: all of it, or a window
        // onto it (a crossfade tile app opening: its full frame clipped
        // to the tile's growing rect, never scaled).
        uv_pos: vec2(0.0, 0.0) uv_size: vec2(1.0, 1.0)
        pixel: fn() {
            let sdf=Sdf2d.viewport(self.pos*self.rect_size)
            sdf.box(0.0,0.0,self.rect_size.x,self.rect_size.y,self.radius)
            let uv=self.uv_pos+self.pos*self.uv_size
            let uv=vec2(uv.x,mix(uv.y,1.0-uv.y,self.y_flip))
            sdf.fill(self.image.sample(uv)*self.opacity)
            return sdf.result
        }
    }
}
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPhoneApp {
    #[deref] draw_super: DrawQuad,
    #[live] pub opacity: f32,
    #[live] pub radius: f32,
    #[live] pub y_flip: f32,
    #[live] pub uv_pos: Vec2f,
    #[live] pub uv_size: Vec2f,
}

/// One off-screen capture of a client's tile widget at one viewport.
struct Capture {
    frame: WindowFrame,
    dirty: bool,
    size: Vec2d,
    style: crate::desktop::DesktopStyle,
    dark: bool,
}
impl Capture {
    fn new(cx: &mut Cx, style: crate::desktop::DesktopStyle, dark: bool) -> Self {
        Self { frame: WindowFrame::new_with_name(cx, "wm_phone_capture"), dirty: true, size: dvec2(0.0, 0.0), style, dark }
    }
    fn stale(&self, size: Vec2d, style: crate::desktop::DesktopStyle, dark: bool) -> bool {
        self.dirty || self.size != size || self.style != style || self.dark != dark
    }
    fn settle(&mut self, size: Vec2d, style: crate::desktop::DesktopStyle, dark: bool) {
        self.dirty = false;
        self.size = size;
        self.style = style;
        self.dark = dark;
    }
}

/// A client's captures on the phone. The full-screen face (Recents cards,
/// the open/close warp) and the compact home-tile face are kept apart: a
/// tile-face frame never lands in a card, and a window frame never lands in
/// a tile, whatever the client happens to present while it switches.
#[derive(Default)]
pub(super) struct PhoneFrame {
    full: Option<Capture>,
    tile: Option<Capture>,
}

impl WmDesk {
    pub(super) fn draw_window_surface(&mut self, cx: &mut Cx2d, frame: &WindowFrame, rect: Rect, radius: f32) {
        self.draw_phone.draw_vars.set_texture(0, frame.texture());
        // The shared drawer also crops (present_capture_window): show all of
        // this frame.
        self.draw_phone.uv_pos = vec2(0.0, 0.0);
        self.draw_phone.uv_size = vec2(1.0, 1.0);
        self.draw_phone.opacity = 1.0;
        // Sdf2d.box uses half the visible corner radius.
        self.draw_phone.radius = radius * 0.5;
        self.draw_phone.y_flip = if matches!(cx.os_type(), OsType::Android(_)) {1.0} else {0.0};
        self.draw_phone.draw_abs(cx, rect);
    }
    fn client_arriving(&self, client: ClientId) -> bool {
        self.items.get(&client).and_then(|item| item.borrow::<MpRunView>())
            .is_some_and(|view| view.arrival_fade() < 1.0)
    }
    pub fn phone_hit(&self,p:Vec2d)->Option<PhoneHit> {self.phone_ui.hit(p)}
    pub fn phone_search_event(&mut self,cx:&mut Cx,event:&Event,state:&mut WmState)->bool {
        let enabled=state.style.target.mobile() && state.phone.screen==PhoneScreen::Drawer;
        self.phone_ui.search_event(cx,event,&mut state.phone,enabled)
    }
    pub fn dismiss_phone_search(&mut self,cx:&mut Cx,phone:&mut crate::mobile::PhoneState,clear:bool) {
        self.phone_ui.dismiss_search(cx,phone,clear);
    }
    pub fn clear_phone_search(&mut self,cx:&mut Cx,phone:&mut crate::mobile::PhoneState) {
        self.phone_ui.clear_search(cx,phone);
    }
    pub fn focus_phone_search(&mut self,cx:&mut Cx,phone:&mut crate::mobile::PhoneState) {
        self.phone_ui.focus_search(cx,phone);
    }
    pub fn phone_search_scroll_max(&self)->f64 {self.phone_ui.search_scroll_max}
    /// A frame from `client` landed in the given face: that face's capture
    /// re-records on the next draw. A frame that belongs to neither (a stale
    /// size while the client switches faces) is left out of both.
    pub fn note_client_frame(&mut self,client:ClientId,face:Option<Face>) {
        let Some(frame)=self.phone_frames.get_mut(&client) else {return};
        match face {
            Some(Face::Full)=>{if let Some(c)=frame.full.as_mut() {c.dirty=true;}}
            Some(Face::Tile)=>{if let Some(c)=frame.tile.as_mut() {c.dirty=true;}}
            None=>{}
        }
    }
    /// Record `client`'s tile widget into `capture` at `rect`, configuring
    /// the child for exactly that viewport.
    fn record_capture(&mut self,cx:&mut Cx2d,scope:&mut Scope,client:ClientId,capture:&mut Capture,rect:Rect,wash:bool) {
        capture.frame.begin(cx,rect);
        if wash {
            // The ground under the app, painted first at z 0: the app's own
            // background (a module's `theme.color_bg_app`, read by the
            // shell), the way the app's own window clears — never the
            // desk's colour, which put a light-theme app's dark ink on the
            // desk's dark ground. The capture pass has a depth buffer, and
            // an app that orders its own ink in depth draws BELOW z 0 as
            // well (the map's ground at -50, its tilted tiles around -24):
            // a ground that wrote depth would win the LessEqual test
            // against all of that and leave only itself. Paint order is the
            // ground's whole claim; it never writes depth.
            let saved=self.draw_panel.color;
            if let Some(ground)=scope.data.get::<WmState>().and_then(|s|s.clients.get(&client)).and_then(|s|s.ground) {
                self.draw_panel.color=ground;
            }
            self.draw_panel.draw_vars.options.depth_write=false;
            self.draw_panel.alpha=1.0;
            self.draw_panel.draw_abs(cx,rect);
            self.draw_panel.color=saved;
        }
        if let Some(item)=self.item(cx,client) {
            with_tile_host(&item,|tile| {tile.set_target_size(Some(rect.size));tile.set_close_crop(None);tile.set_fade(1.0);});
            item.draw_walk_all(cx,scope,Walk::abs_rect(rect));
        }
        capture.frame.end(cx);
        self.compositor.as_mut().unwrap().content_pass(capture.frame.pass_id());
    }
    fn present_capture(&mut self,cx:&mut Cx2d,capture:&Capture,rect:Rect,opacity:f32,radius:f32) {
        self.present_capture_window(cx,capture,rect,rect,opacity,radius);
    }
    /// The device's status-bar band above a foreground app: the app's own
    /// top rows stretched up over the band, so the strip is whatever
    /// colour the app is there (a dark face stays dark, a grey sky grey).
    /// The proper contract — the app extending under the bar with the
    /// host passing the safe insets — is not built yet.
    fn present_capture_band(&mut self,cx:&mut Cx2d,capture:&Capture,full:Rect,band:Rect,opacity:f32) {
        self.draw_phone.draw_vars.set_texture(0,capture.frame.texture());
        self.draw_phone.opacity=opacity;
        self.draw_phone.radius=0.0;
        self.draw_phone.y_flip=if matches!(cx.os_type(),OsType::Android(_)){1.0}else{0.0};
        self.draw_phone.uv_pos=vec2(0.0,0.0);
        self.draw_phone.uv_size=vec2(1.0,(2.0/full.size.y.max(1.0)) as f32);
        self.draw_phone.draw_abs(cx,band);
        self.compositor.as_mut().unwrap().content(band);
    }
    /// Show the part of `capture` (recorded over `full`) that lies under
    /// `window`, at `window`, pixel for pixel: the location-accurate
    /// crossfade of a tile app whose tile is a piece of its full face.
    fn present_capture_window(&mut self,cx:&mut Cx2d,capture:&Capture,full:Rect,window:Rect,opacity:f32,radius:f32) {
        self.draw_phone.draw_vars.set_texture(0,capture.frame.texture());
        self.draw_phone.opacity=opacity;
        self.draw_phone.radius=radius;
        self.draw_phone.y_flip=if matches!(cx.os_type(),OsType::Android(_)){1.0}else{0.0};
        let size=dvec2(full.size.x.max(1.0),full.size.y.max(1.0));
        self.draw_phone.uv_pos=vec2(((window.pos.x-full.pos.x)/size.x) as f32,((window.pos.y-full.pos.y)/size.y) as f32);
        self.draw_phone.uv_size=vec2((window.size.x/size.x) as f32,(window.size.y/size.y) as f32);
        self.draw_phone.draw_abs(cx,window);
        self.compositor.as_mut().unwrap().content(window);
    }
    /// The live tiles on the home page: each tile client's compact capture,
    /// or a placeholder card while it builds, starts, or has not confirmed
    /// its compact face yet.
    fn draw_home_tiles(&mut self,cx:&mut Cx2d,scope:&mut Scope,screen:Rect) {
        let state=scope.data.get_mut::<WmState>().unwrap();
        let phone=state.phone.clone();
        let style=state.style.target;
        let dark=state.style.dark;
        // iOS pans the page (tiles included) off to the left as the library
        // arrives; Android's tiles fade under its rising sheet.
        let ios=style==crate::desktop::DesktopStyle::Ios;
        let opacity=((1.0-phone.openness*0.85)*(if ios {1.0} else {1.0-phone.drawer.clamp(0.0,1.0)})) as f32;
        if opacity<0.01 || (ios && phone.drawer>=0.999) {return;}
        let page=if ios {Rect{pos:screen.pos+dvec2(-phone.drawer*screen.size.x,0.0),size:screen.size}} else {screen};
        let layout=PhoneSurface::home_layout(style,page,phone.chrome,&state.launchable);
        // Everything the placeholders need, read before any tile draws.
        let slots:Vec<(crate::mobile_tiles::TileSlot,Option<ClientId>,String,bool)>=layout.tiles.iter().map(|slot| {
            let client=phone.tiles.client_of(slot.app);
            let (status,connected)=client.and_then(|c|state.clients.get(&c)).map(|s|(s.status.clone(),s.sender.is_some())).unwrap_or_default();
            (*slot,client,status,connected)
        }).collect();
        for (slot,client,status,connected) in slots {
            let gave_up=phone.tiles.gave_up(slot.app);
            let entry=client.and_then(|c|phone.tiles.get(c));
            let mut shown=false;
            if let (Some(client),Some(entry))=(client,entry) {
                let mut stored=self.phone_frames.remove(&client).unwrap_or_default();
                if entry.in_tile_face() {
                    let ready=entry.tile_ready();
                    let mut capture=stored.tile.take().unwrap_or_else(||Capture::new(cx,style,dark));
                    // Not confirmed yet: keep the child driven at the tile
                    // viewport every frame; confirmed: only when it drew.
                    // A MODULE draws inside this very pass — no swapchain
                    // delivers its later frames — so its tile is recorded
                    // on every home draw, the way any visible widget is.
                    if !ready || self.client_arriving(client) || capture.stale(slot.rect.size,style,dark) || self.module_clients.contains(&client) {
                        self.record_capture(cx,scope,client,&mut capture,slot.rect,false);
                        capture.settle(slot.rect.size,style,dark);
                    } else {
                        capture.frame.freeze(cx);
                    }
                    if ready {
                        self.present_capture(cx,&capture,slot.rect,opacity,TILE_RADIUS as f32);
                        shown=true;
                    }
                    stored.tile=Some(capture);
                } else if let Some(capture)=stored.tile.as_mut() {
                    // Open, or still animating home: the last compact face
                    // stands in until the client is back in it.
                    capture.frame.freeze(cx);
                    if capture.size==slot.rect.size {
                        self.present_capture(cx,capture,slot.rect,opacity,TILE_RADIUS as f32);
                        shown=true;
                    }
                }
                self.phone_frames.insert(client,stored);
            }
            if !shown {
                let (headline,detail)=crate::mobile_tiles::placeholder_text(&status,connected,gave_up);
                self.phone_ui.draw_tile_placeholder(cx,slot,style,dark,opacity,headline,&detail);
                self.compositor.as_mut().unwrap().content(slot.rect);
            }
        }
    }
    pub(super) fn draw_phone_scene(&mut self,cx:&mut Cx2d,scope:&mut Scope,screen:Rect) {
        let state=scope.data.get_mut::<WmState>().unwrap();
        state.phone.viewport=screen;
        state.phone.order.retain(|c|state.clients.contains_key(c));
        if state.phone.client.is_some_and(|c|!state.clients.contains_key(&c)) {
            state.phone.client=state.phone.order.first().copied();
            if state.phone.client.is_none() {state.phone.navigate(PhoneScreen::Home);}
        }
        // Only windows in the layout join Recents: a tile client launched by
        // the home page stays out until the person opens it.
        for c in state.layout.clients_on(state.layout.active) {
            if !state.phone.order.contains(&c) {state.phone.order.push(c);}
        }
        self.style=state.style.clone();
        self.title_hits.clear();self.zorder.clear();self.minimized.clear();
        let warps: Vec<ClientId>=self.dock_warps.keys().copied().collect();
        self.drop_dock_warps(cx,warps);
        let gone: Vec<ClientId>=self.phone_frames.keys().filter(|c|!state.clients.contains_key(c)).copied().collect();
        for c in gone {
            if let Some(frame)=self.phone_frames.remove(&c) {
                for capture in [frame.full,frame.tile].into_iter().flatten() {capture.frame.forget(cx);}
            }
        }
        let phone=state.phone.clone();
        let launchable=state.launchable.clone();
        let style=state.style.target;
        let dark=state.style.dark;
        let app=mobile::app_rect(screen,phone.chrome);
        self.compositor.get_or_insert_with(||BackdropCompositor::new(cx)).begin(cx);
        self.phone_ui.begin();
        self.phone_ui.draw_wallpaper(cx,screen,style,dark,phone.wallpaper_time);
        self.compositor.as_mut().unwrap().content(screen);
        let home_backdrop=if style==crate::desktop::DesktopStyle::Ios && phone.openness<0.999 {
            // The dock's profile decides the pyramid it needs (mip0 and its
            // neighbour) and how far past its rect the lens reads.
            {let profile=gauss_view::GlassProfile::dock(dark);
             Some(self.compositor.as_mut().unwrap().backdrop_with_reach(cx,PhoneSurface::home_dock(screen,phone.chrome),profile.requested_level(),profile.sample_reach()))}
        }else{None};
        self.phone_ui.draw_home(cx,state,screen,home_backdrop);
        self.compositor.as_mut().unwrap().content(screen);
        if phone.home_visible() {self.draw_home_tiles(cx,scope,screen);}
        if phone.overview>0.001 {
            let blur = (phone.overview.clamp(0.0, 1.0) * 3.0) as f32;
            self.phone_ui.overview_glass.set_blurriness(cx, blur);
            let backdrop=self.compositor.as_mut().unwrap().backdrop(cx,screen,blur as f64);
            self.phone_ui.overview_glass.draw_surface_with_backdrop(cx,screen,Some(backdrop),phone.overview as f32);
            self.compositor.as_mut().unwrap().content(screen);
        }
        let mut order=phone.order.clone();
        order.reverse();
        // Foreground paints last during launch/return transitions.
        if phone.overview<0.001 {
            if let Some(c)=phone.client {order.retain(|i|*i!=c);order.push(c);}
        }
        // The device's status band above the open app is the app's own
        // frame stretched up (drawn once the card below it has painted).
        let band=(!phone.chrome.fake_status() && phone.overview<0.001 && phone.screen==PhoneScreen::App)
            .then(|| Rect {pos:screen.pos,size:dvec2(screen.size.x,phone.chrome.top_reserve(screen))})
            .filter(|b|b.size.y>0.5);
        let mut band_painted=false;
        for client in order {
            // The client being pulled between its full rect and its card:
            // only while it is open at all. On Home (and a switcher entered
            // from Home) every client is a card, faded in by `overview`.
            let foreground=phone.client==Some(client) && phone.openness>0.001;
            if !foreground && phone.overview<0.001 {continue;}
            if foreground && phone.openness<0.001 {continue;}
            let index=phone.order.iter().position(|c|*c==client).unwrap_or(0);
            let card=mobile::card_rect(screen,phone.chrome,index as f64,phone.cards.page);
            let mut display=if foreground {
                let icon=PhoneSurface::launch_origin(style,screen,phone.chrome,&launchable,phone.tiles.get(client).map(|t|t.app.as_str()));
                mobile::mix_rect(mobile::mix_rect(icon,app,phone.openness),card,phone.overview)
            }else{card};
            if phone.gesture.as_ref().is_some_and(|g|g.hit==Some(PhoneHit::Card(client))) {display.pos.y+=phone.dismiss_y;}
            if display.pos.x+display.size.x<screen.pos.x || display.pos.x>screen.pos.x+screen.size.x {continue;}
            // A tile client's window frames are trusted only in its full
            // face: while it shows (or switches to) the compact face the
            // card keeps the last full-screen capture, whatever arrives.
            let full_ready=phone.tiles.get(client).map_or(true,|t|t.full_ready());
            let mut stored=self.phone_frames.remove(&client).unwrap_or_default();
            let opacity=if foreground {phone.openness as f32}else{phone.overview as f32};
            let radius=((1.0-phone.openness).max(phone.overview)*26.0)as f32;
            // A crossfade tile app between its tile and full screen: its
            // full frame shows through a window that grows from the tile's
            // rect to the app's, pixels in place (the app put its tile
            // subject there), fading in over the tile fading out.
            let crossfade=foreground && phone.overview<0.001 && phone.openness<0.999
                && phone.tiles.get(client).is_some_and(|t|crate::mobile_tiles::crossfade_app(&t.app))
                && display.size.x<app.size.x-0.5;
            match stored.full.take() {
                Some(mut capture)=>{
                    let refresh=full_ready && (foreground && phone.screen==PhoneScreen::App || self.client_arriving(client) || capture.stale(app.size,style,dark));
                    if refresh {
                        self.record_capture(cx,scope,client,&mut capture,app,true);
                        capture.settle(app.size,style,dark);
                    }else{capture.frame.freeze(cx);}
                    if crossfade {self.present_capture_window(cx,&capture,app,display,opacity,radius);}
                    else {self.present_capture(cx,&capture,display,opacity,radius);}
                    if let Some(band)=band.filter(|_|foreground && phone.openness>0.999) {
                        self.present_capture_band(cx,&capture,app,band,opacity);
                        band_painted=true;
                    }
                    stored.full=Some(capture);
                }
                None if full_ready=>{
                    let mut capture=Capture::new(cx,style,dark);
                    self.record_capture(cx,scope,client,&mut capture,app,true);
                    capture.settle(app.size,style,dark);
                    if crossfade {self.present_capture_window(cx,&capture,app,display,opacity,radius);}
                    else {self.present_capture(cx,&capture,display,opacity,radius);}
                    if let Some(band)=band.filter(|_|foreground && phone.openness>0.999) {
                        self.present_capture_band(cx,&capture,app,band,opacity);
                        band_painted=true;
                    }
                    stored.full=Some(capture);
                }
                None=>{
                    // Opened straight from its tile and no full-size frame
                    // yet: a plain launch card, never the squeezed tile.
                    let app_id=phone.tiles.get(client).map(|t|t.app.clone()).unwrap_or_default();
                    self.phone_ui.draw_launch_card(cx,display,&app_id,style,dark,opacity,radius);
                    self.compositor.as_mut().unwrap().content(display);
                }
            }
            self.phone_frames.insert(client,stored);
            if foreground {self.zorder.push(client);}
        }
        let glass=if phone.keyboard>0.5 {
            Some((Rect {pos:screen.pos+dvec2(0.0,screen.size.y-phone.keyboard-phone.chrome.bottom_reserve(screen)),size:dvec2(screen.size.x,phone.keyboard)},4.0))
        }else{None};
        let (backdrop,_,_)=self.compositor.as_mut().unwrap().finish(cx,screen,glass);
        let state=scope.data.get_mut::<WmState>().unwrap();
        state.phone.band_from_app=band_painted;
        self.phone_ui.draw_overlay(cx,state,screen,backdrop);
    }
    pub(super) fn handle_phone_event(&mut self,cx:&mut Cx,event:&Event,scope:&mut Scope) {
        let state=scope.data.get_mut::<WmState>().unwrap();
        let input=matches!(event,Event::TouchUpdate(_)|Event::MouseDown(_)|Event::MouseUp(_)|Event::MouseMove(_)|Event::Scroll(_)|Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_));
        let client=state.phone.client;
        if input && !state.phone.accepts_app_input() {return;}
        let items:Vec<_>=self.items.iter().filter(|(c,_)|!input || Some(**c)==client).map(|(_,w)|w.clone()).collect();
        for item in items {item.handle_event(cx,event,scope);}
    }
}
