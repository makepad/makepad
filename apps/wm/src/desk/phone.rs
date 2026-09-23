use super::*;
use crate::mobile::{self, PhoneHit, PhoneScreen};
use crate::mobile_tiles::{Face, HomeMetrics};
use makepad_widgets::makepad_platform::event::{DigitDevice, DigitId, FingerCancelEvent, TouchPoint, TouchState, TouchUpdateEvent};
use makepad_widgets::makepad_platform::{ReadbackChannelOrder, ReadbackError, ReadbackOrigin, ReadbackRequest, ReadbackTicket, TextureReadback};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    set_type_default() do #(DrawPhoneApp::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        opacity: 1.0 radius: 0.0
        // The part of the texture this rect shows: all of it, or a window
        // onto it (a crossfade tile app opening: its full frame clipped
        // to the tile's growing rect, never scaled).
        uv_pos: vec2(0.0, 0.0) uv_size: vec2(1.0, 1.0)
        pixel: fn() {
            let uv=self.uv_pos+self.pos*self.uv_size
            // Square: a hard edge, so a full-screen app meets the bars
            // without a device pixel of the wallpaper between them.
            if self.radius<0.001 {
                return self.image.sample(uv)*self.opacity
            }
            let sdf=Sdf2d.viewport(self.pos*self.rect_size)
            sdf.box(0.0,0.0,self.rect_size.x,self.rect_size.y,self.radius)
            // The capture is premultiplied already: `fill` would multiply
            // by alpha again — opacity squared, the grey ghost of a card
            // fading in or out.
            sdf.fill_premul(self.image.sample(uv)*self.opacity)
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
    /// A tile face not confirmed yet (back from full screen) is recorded
    /// here, driving the child at the tile viewport, while the last good
    /// tile capture keeps standing in — never a "Loading…" placeholder
    /// over a tile that was showing a moment ago.
    tile_pending: Option<Capture>,
    bands: Bands,
}

/// The colour of the app's edge rows under the status and navigation bars,
/// read back from the GPU (`present_capture_band`). The bars are that
/// colour, and their ink is chosen against it whatever the app's theme
/// says (Weather's sky is dark under a light theme).
///
/// Only the two edge strips are read: the desk copies them out of the
/// app's capture into a small target of its own (`strip`) and reads that.
/// `revision` moves only when the app's content changed (it asked for a
/// redraw, or a frame of its arrived) or the key changed. A sample is
/// taken on a later, quiet draw — never in the draw that recorded the
/// change, so the strip cannot copy last frame's capture — at most once a
/// `BAND_EVERY` on the monotonic clock. A change waiting on either gets
/// one timer (`retry`); a failed or unusable sample counts as taken, so
/// nothing retries forever.
#[derive(Default)]
struct Bands {
    /// The top and bottom rows once read: their mean (see `BandRows`).
    top: Option<BandRows>,
    bottom: Option<BandRows>,
    /// What the rows were read for: a new viewport, density, appearance or
    /// skin drops them.
    key: Option<BandKey>,
    /// Captures recorded, and the one the last sample was taken of.
    revision: u64,
    sampled: u64,
    /// The sample in flight: its ticket, and the key and revision it is for.
    pending: Option<(ReadbackTicket, BandKey, u64)>,
    asked_at: Option<f64>,
    retry: Timer,
    /// When `retry` is due: a timer whose event went elsewhere (the desk
    /// left the phone skin) is dropped once well past it.
    retry_due: f64,
    /// The revision moved in the draw being recorded.
    changed_now: bool,
    /// The small target the two strips are copied into.
    strip: Option<WindowFrame>,
}

/// What a band sample is valid for.
#[derive(Clone, Copy, PartialEq, Debug)]
struct BandKey {
    size: (i64, i64),
    dpi_milli: i64,
    style: crate::desktop::DesktopStyle,
    dark: bool,
}
/// At most one band sample per client this often (seconds, monotonic).
const BAND_EVERY: f64 = 1.0;
/// A change is sampled no sooner than this after it (a later, quiet draw).
const BAND_SETTLE: f64 = 0.1;

/// Arm `bands`' one retry `delay` from now unless one is armed.
fn arm_band_retry(cx:&mut Cx,bands:&mut Bands,delay:f64) {
    if !bands.retry.is_empty() {return;}
    bands.retry=cx.start_timeout(delay.max(0.0));
    bands.retry_due=cx.seconds_since_app_start()+delay.max(0.0);
}

/// One band's rows as read back.
#[derive(Clone, Copy, PartialEq)]
struct BandRows {
    /// Mean colour, sRGB.
    color: Vec4f,
    /// Relative luminance of that mean.
    luma: f32,
    /// Nothing on them (a flat colour, a sky): the band stretches the
    /// rows themselves and meets the app seamlessly. Rows that something
    /// crosses (a toolbar's buttons, a list's text) would streak, and the
    /// band is their mean colour instead.
    plain: bool,
}
impl BandRows {
    /// The solid colour the band is, or None to stretch the rows.
    fn solid(&self) -> Option<Vec4f> { (!self.plain).then_some(self.color) }
}
impl Bands {
    /// A capture was recorded for `key`; `changed` when the app's content
    /// changed with it. Rows read for another key go.
    fn recorded(&mut self, key: BandKey, changed: bool) {
        self.changed_now = false;
        if self.key != Some(key) {
            self.key = Some(key);
            self.top = None;
            self.bottom = None;
            self.revision += 1;
            self.changed_now = true;
        } else if changed {
            self.revision += 1;
            self.changed_now = true;
        }
    }
    fn dirty(&self) -> bool { self.revision > self.sampled }
    /// A sample for `key` taken of `revision` came back with `rows` (None:
    /// failed or unusable). It counts as taken either way; a sample for a
    /// key the app has left changes nothing (its key change moved the
    /// revision, which asks again). True when the bars should change.
    fn finish(&mut self, key: BandKey, revision: u64, rows: Option<(BandRows, BandRows)>) -> bool {
        if self.key != Some(key) {
            return false;
        }
        self.sampled = self.sampled.max(revision);
        let Some((top, bottom)) = rows else { return false };
        // A change the eye would catch: text scrolling past the edge moves
        // the mean a little.
        let moved = |old: Option<BandRows>, new: BandRows| old.map_or(true, |old| {
            let (c, n) = (old.color, new.color);
            old.plain != new.plain || (c.x - n.x).abs().max((c.y - n.y).abs()).max((c.z - n.z).abs()) > 0.02
        });
        let mut changed = false;
        if moved(self.top, top) { self.top = Some(top); changed = true; }
        if moved(self.bottom, bottom) { self.bottom = Some(bottom); changed = true; }
        changed
    }
}

/// The strip's rows per band in device pixels at `dpi`, and the strip's
/// height in points: whole pixels, so both bands always fit the texture
/// the pass allocates (its height is the points times the density,
/// truncated), at any density.
fn strip_rows(dpi: f64) -> (usize, f64) {
    let rows = (BAND_ROWS * dpi).ceil().max(1.0) as usize;
    (rows, (2.0 * rows as f64 + 0.25) / dpi)
}
/// Band rows are plain while at most this share of neighbouring pixels
/// steps by more than `BAND_EDGE` in (sRGB) luminance: a sky measures
/// 0.2 %, a row of text or a toolbar's buttons 3 %.
const BAND_PLAIN: f32 = 0.01;
const BAND_EDGE: f32 = 0.03;

/// How many device pixels in from the app's top and bottom edges the band
/// rows start: past the edge row, which an app's root blends with its
/// theme ground, and far enough that filtering never reaches it.
const BAND_INSET_PX: f64 = 2.0;
/// How tall the band rows are, in points.
const BAND_ROWS: f64 = 2.0;

/// sRGB byte to linear light.
fn linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
/// Linear light back to sRGB.
fn srgb(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// The mean colour (averaged in linear light) and relative luminance of
/// `rows` device rows `inset` rows in from the top and from the bottom of a
/// readback: the band rows.
fn band_sample(r: &TextureReadback, inset: usize, rows: usize) -> Option<(BandRows, BandRows)> {
    let data = r.data.as_ref().ok()?;
    if r.width == 0 || rows == 0 || r.height < (inset + rows) * 2 || r.stride < r.width * 4 { return None; }
    let (ri, bi) = match r.channel_order { ReadbackChannelOrder::Bgra => (2, 0), ReadbackChannelOrder::Rgba => (0, 2) };
    let mean = |from: usize| {
        let mut sum = [0.0f32; 3];
        let mut edges = 0usize;
        for y in from..from + rows {
            let row = &data[y * r.stride..y * r.stride + r.width * 4];
            let mut last: Option<f32> = None;
            for px in row.chunks_exact(4) {
                let (red, green, blue) = (linear(px[ri]), linear(px[1]), linear(px[bi]));
                sum[0] += red;
                sum[1] += green;
                sum[2] += blue;
                let seen = (0.2126 * px[ri] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[bi] as f32) / 255.0;
                if let Some(last) = last { edges += usize::from((seen - last).abs() > BAND_EDGE); }
                last = Some(seen);
            }
        }
        let n = (rows * r.width) as f32;
        let [red, green, blue] = sum.map(|c| c / n);
        BandRows {
            color: vec4(srgb(red), srgb(green), srgb(blue), 1.0),
            luma: 0.2126 * red + 0.7152 * green + 0.0722 * blue,
            plain: edges as f32 / (rows * (r.width - 1).max(1)) as f32 <= BAND_PLAIN,
        }
    };
    // Rows count from the top of the texture unless it says otherwise.
    let (top, bottom) = (mean(inset), mean(r.height - inset - rows));
    Some(match r.origin {
        ReadbackOrigin::TopLeft => (top, bottom),
        ReadbackOrigin::BottomLeft => (bottom, top),
    })
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
        self.draw_phone.draw_abs(cx, rect);
    }
    /// Where the open app zooms out of and back into. Opening: the icon or
    /// tile that was tapped. Closing: its tile or home icon if the home page
    /// has one, else a short shrink toward the middle of the screen (an app
    /// opened from the drawer has nothing on the home page to go back to).
    /// False with the fallback: nothing to land on, so the card fades as it goes.
    fn phone_zoom_origin(&self,phone:&crate::mobile::PhoneState,launchable:&crate::apps::Launchable,style:crate::desktop::DesktopStyle,screen:Rect,app:Rect,client:ClientId,app_id:&str)->(Rect,bool) {
        let tapped=phone.origin.filter(|(c,_,_)|*c==client);
        if phone.screen==PhoneScreen::App {
            if let Some((_,rect,in_drawer))=tapped {
                // A home icon's hit is its whole cell; the zoom starts from the icon.
                if !in_drawer {if let Some(icon)=self.phone_ui.home_rect_of(style,screen,phone,launchable,app_id) {return (icon,true);}}
                return (rect,true);
            }
        } else if let Some(rect)=self.phone_ui.home_rect_of(style,screen,phone,launchable,app_id) {
            return (rect,true);
        } else if tapped.is_some_and(|(_,_,in_drawer)|!in_drawer) {
            return (tapped.unwrap().1,true);
        }
        let tile=phone.tiles.get(client).map(|t|t.app.as_str());
        if tile.is_some() {return (PhoneSurface::launch_origin(style,screen,phone.chrome,launchable,tile),true);}
        let size=app.size*0.4;
        (Rect {pos:app.pos+(app.size-size)*0.5+dvec2(0.0,app.size.y*0.15),size},false)
    }
    fn client_arriving(&self, client: ClientId) -> bool {
        self.items.get(&client).and_then(|item| item.borrow::<MpRunView>())
            .is_some_and(|view| view.arrival_fade() < 1.0)
    }
    pub fn phone_hit(&self,p:Vec2d)->Option<PhoneHit> {self.phone_ui.hit(p)}
    pub fn phone_icon_at(&self,p:Vec2d,app:&str)->Option<Rect> {self.phone_ui.icon_at(p,app)}
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
    /// The status band over the open app and its navigation band under it,
    /// each drawn when the phone has it; what the bars' ink is chosen
    /// against goes to `luma` (the rows read back, or the app's theme
    /// ground until they are).
    #[allow(clippy::too_many_arguments)]
    fn present_bands(&mut self,cx:&mut Cx2d,frame:&mut PhoneFrame,capture:&Capture,app:Rect,open:bool,top:Option<Rect>,bottom:Option<Rect>,opacity:f32,ground:Option<Vec4f>,key:BandKey,painted:&mut bool,luma:&mut (Option<f32>,Option<f32>)) {
        if !open {return;}
        let fresh=frame.bands.key==Some(key);
        let (top_rows,bottom_rows)=if fresh {(frame.bands.top,frame.bands.bottom)} else {(None,None)};
        let theme=ground_luma(ground);
        if let Some(r)=top {
            self.present_capture_band(cx,capture,app,r,opacity,top_rows.and_then(|t|t.solid()));
            *painted=true;
            luma.0=Some(top_rows.map_or(theme,|t|t.luma));
        }
        if let Some(r)=bottom {
            self.present_capture_band(cx,capture,app,r,opacity,bottom_rows.and_then(|b|b.solid()));
            luma.1=Some(bottom_rows.map_or(theme,|b|b.luma));
        }
    }
    /// A bar over the app's top edge or under its bottom edge in the app's
    /// own colour there: `solid`, the mean the GPU read back, once known
    /// (content that reaches the edge, a toolbar's buttons or a list's
    /// text, would streak if stretched); until then the edge rows
    /// themselves, stretched.
    fn present_capture_band(&mut self,cx:&mut Cx2d,capture:&Capture,full:Rect,band:Rect,opacity:f32,solid:Option<Vec4f>) {
        let below=band.pos.y>=full.pos.y;
        // One device pixel over the app's edge row: an app's root blends
        // that row with its theme ground, a light seam under a dark band.
        let px=1.0/cx.current_dpi_factor();
        let band=if below {Rect{pos:band.pos-dvec2(0.0,px),size:band.size+dvec2(0.0,px)}} else {Rect{pos:band.pos,size:band.size+dvec2(0.0,px)}};
        if let Some(color)=solid {
            let saved=self.draw_panel.color;
            self.draw_panel.color=color;
            self.draw_panel.alpha=opacity;
            self.draw_panel.draw_abs(cx,band);
            self.draw_panel.color=saved;
        } else {
            self.draw_band_rows(cx,capture,full,band,below,opacity);
        }
        self.compositor.as_mut().unwrap().content(band);
    }
    /// The band rows of `capture` (its top rows, or its bottom rows when
    /// `below`), drawn stretched over `r`.
    fn draw_band_rows(&mut self,cx:&mut Cx2d,capture:&Capture,full:Rect,r:Rect,below:bool,opacity:f32) {
        let px=1.0/cx.current_dpi_factor();
        self.draw_phone.draw_vars.set_texture(0,capture.frame.texture());
        self.draw_phone.opacity=opacity;
        self.draw_phone.radius=0.0;
        let h=full.size.y.max(1.0);
        let rows=(BAND_ROWS/h) as f32;
        let inset=(BAND_INSET_PX*px/h) as f32;
        self.draw_phone.uv_pos=vec2(0.0,if below {1.0-inset-rows}else{inset});
        self.draw_phone.uv_size=vec2(1.0,rows);
        self.draw_phone.draw_abs(cx,r);
    }
    /// Sample the foreground app's band rows if they changed, on a quiet
    /// draw the clock allows: copy them into the small strip target and
    /// read that back. Otherwise leave the strip frozen, and arm the one
    /// retry when a change waits.
    fn ask_band_sample(&mut self,cx:&mut Cx2d,frame:&mut PhoneFrame,full:Rect,key:BandKey) {
        let now=cx.seconds_since_app_start();
        let bands=&mut frame.bands;
        // A retry whose timer event never came here (the desk left the
        // phone skin meanwhile) is forgotten.
        if !bands.retry.is_empty() && now>bands.retry_due+1.0 {bands.retry=Timer::empty();}
        let quiet=!std::mem::take(&mut bands.changed_now);
        if self.phone_ui.band_sampling_off || !bands.dirty() || bands.pending.is_some() || !quiet {
            if let Some(strip)=bands.strip.as_mut() {strip.freeze(cx);}
            if bands.dirty() && !quiet && bands.pending.is_none() && !self.phone_ui.band_sampling_off {
                // Changed in this very draw: sample on a later one.
                let due=bands.asked_at.map_or(now,|t|t+BAND_EVERY).max(now+BAND_SETTLE);
                arm_band_retry(cx,bands,due-now);
            }
            return;
        }
        let due=bands.asked_at.map_or(0.0,|t|t+BAND_EVERY);
        if now<due {
            if let Some(strip)=bands.strip.as_mut() {strip.freeze(cx);}
            arm_band_retry(cx,bands,due-now);
            return;
        }
        let Some(capture)=frame.full.as_ref() else {return};
        let started=std::time::Instant::now();
        let (rows,height)=strip_rows(key.dpi_milli as f64/1000.0);
        let half=rows as f64/(key.dpi_milli as f64/1000.0);
        let mut strip=frame.bands.strip.take().unwrap_or_else(||WindowFrame::new_with_name(cx,"wm_band_strip"));
        // Top rows in the upper half, bottom rows in the lower.
        let r=Rect{pos:full.pos,size:dvec2(full.size.x,height)};
        strip.begin(cx,r);
        self.draw_band_rows(cx,capture,full,Rect{pos:r.pos,size:dvec2(r.size.x,half)},false,1.0);
        self.draw_band_rows(cx,capture,full,Rect{pos:r.pos+dvec2(0.0,half),size:dvec2(r.size.x,half)},true,1.0);
        strip.end(cx);
        let bands=&mut frame.bands;
        bands.asked_at=Some(now);
        match strip.texture().read_back(cx,ReadbackRequest::default()) {
            Ok(ticket)=>{
                bands.pending=Some((ticket,key,bands.revision));
                self.phone_ui.band_samples+=1;
                if std::env::var_os("MAKEPAD_WM_TRACE_BANDS").is_some() {
                    log!("wm: band sample {} asked (rev {}) in {:.3} ms",self.phone_ui.band_samples,bands.revision,started.elapsed().as_secs_f64()*1000.0);
                }
            }
            // Refused: this revision counts as sampled, nothing retries.
            // A backend that cannot read back at all is not asked again.
            Err(error)=>{
                bands.sampled=bands.revision;
                if error==ReadbackError::UnsupportedBackend {
                    self.phone_ui.band_sampling_off=true;
                    log!("wm: band colours need texture readback, which this backend lacks; the bars keep the app's theme colours");
                }
            }
        }
        frame.bands.strip=Some(strip);
    }
    /// Band samples that came back: each client's band rows, and a redraw
    /// when the bars should change. Only drained while a sample of the
    /// desk's is out (the platform hands out every readback at once).
    fn take_band_readbacks(&mut self,cx:&mut Cx) {
        if !self.phone_frames.values().any(|f|f.bands.pending.is_some()) {return;}
        let results=cx.try_take_texture_readbacks();
        let now=cx.seconds_since_app_start();
        let mut changed=false;
        for result in results {
            let Some(frame)=self.phone_frames.values_mut().find(|f|f.bands.pending.is_some_and(|(t,_,_)|t==result.ticket)) else {continue};
            let (_,key,revision)=frame.bands.pending.take().unwrap();
            if std::env::var_os("MAKEPAD_WM_TRACE_BANDS").is_some() {
                log!("wm: band sample back after {:.1} ms, {} bytes",(now-frame.bands.asked_at.unwrap_or(now))*1000.0,result.data.as_ref().map_or(0,|d|d.len()));
            }
            let (rows,_)=strip_rows(key.dpi_milli as f64/1000.0);
            changed|=frame.bands.finish(key,revision,band_sample(&result,0,rows));
            // Changed again while this one was out: one more, later.
            if frame.bands.dirty() {
                let due=frame.bands.asked_at.map_or(now,|t|t+BAND_EVERY).max(now+BAND_SETTLE);
                arm_band_retry(cx,&mut frame.bands,due-now);
            }
        }
        if changed {self.area.redraw(cx);}
    }
    /// A band retry came due: draw once more, so the sampler takes the
    /// capture that waited.
    fn band_retry(&mut self,cx:&mut Cx,event:&Event) {
        let mut due=false;
        for frame in self.phone_frames.values_mut() {
            if frame.bands.retry.is_event(event).is_some() {
                frame.bands.retry=Timer::empty();
                due|=frame.bands.dirty();
            }
        }
        if due {
            self.phone_ui.band_redraws+=1;
            if std::env::var_os("MAKEPAD_WM_TRACE_BANDS").is_some() {log!("wm: band redraw {}",self.phone_ui.band_redraws);}
            self.area.redraw(cx);
        }
    }
    /// Show the part of `capture` (recorded over `full`) that lies under
    /// `window`, at `window`, pixel for pixel: the location-accurate
    /// crossfade of a tile app whose tile is a piece of its full face.
    /// `radius` is the corner the screen shows.
    fn present_capture_window(&mut self,cx:&mut Cx2d,capture:&Capture,full:Rect,window:Rect,opacity:f32,radius:f32) {
        self.draw_phone.draw_vars.set_texture(0,capture.frame.texture());
        self.draw_phone.opacity=opacity;
        // Sdf2d.box draws twice its radius argument.
        self.draw_phone.radius=radius*0.5;
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
        let opacity=(1.0-phone.openness*0.85) as f32;
        if opacity<0.01 || phone.drawer>=0.999 {return;}
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
                    let module=self.module_clients.contains(&client);
                    // Not confirmed, and a good tile capture of this size is
                    // there: drive the child into a scratch capture and keep
                    // showing the good one.
                    let good=stored.tile.as_ref().is_some_and(|c|!c.stale(slot.rect.size,style,dark));
                    if !ready && good && !module {
                        let mut pending=stored.tile_pending.take().unwrap_or_else(||Capture::new(cx,style,dark));
                        self.record_capture(cx,scope,client,&mut pending,slot.rect,false);
                        pending.settle(slot.rect.size,style,dark);
                        stored.tile_pending=Some(pending);
                        let capture=stored.tile.as_mut().unwrap();
                        capture.frame.freeze(cx);
                        self.present_capture(cx,capture,slot.rect,opacity,HomeMetrics::of(style).tile_radius as f32);
                        self.phone_frames.insert(client,stored);
                        continue;
                    }
                    // Promote the pending capture; the old tile capture is
                    // KEPT (as the next pending one), never dropped: the
                    // client's run view was drawn into it, and a dropped
                    // pass/draw list is reused while that view's area still
                    // points at it — its next frame then landed in whatever
                    // took the slot (the clock tile drawn over Weather).
                    let fresh=stored.tile_pending.take();
                    let forced=fresh.is_some();
                    let mut capture=match fresh {
                        Some(fresh)=>{stored.tile_pending=stored.tile.take();fresh}
                        None=>stored.tile.take().unwrap_or_else(||Capture::new(cx,style,dark)),
                    };
                    // Not confirmed yet: keep the child driven at the tile
                    // viewport every frame; confirmed: only when it drew.
                    // A MODULE draws inside this very pass — no swapchain
                    // delivers its later frames — so its tile is recorded
                    // on every home draw, the way any visible widget is.
                    if forced || !ready || self.client_arriving(client) || capture.stale(slot.rect.size,style,dark) || module {
                        self.record_capture(cx,scope,client,&mut capture,slot.rect,false);
                        capture.settle(slot.rect.size,style,dark);
                    } else {
                        capture.frame.freeze(cx);
                    }
                    if ready {
                        self.present_capture(cx,&capture,slot.rect,opacity,HomeMetrics::of(style).tile_radius as f32);
                        shown=true;
                    }
                    stored.tile=Some(capture);
                } else if let Some(capture)=stored.tile.as_mut() {
                    // Open, or still animating home: the last compact face
                    // stands in until the client is back in it.
                    capture.frame.freeze(cx);
                    if capture.size==slot.rect.size {
                        self.present_capture(cx,capture,slot.rect,opacity,HomeMetrics::of(style).tile_radius as f32);
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
                if let Some(strip)=frame.bands.strip {strip.forget(cx);}
                if !frame.bands.retry.is_empty() {cx.stop_timer(frame.bands.retry);}
            }
        }
        let phone=state.phone.clone();
        let launchable=state.launchable.clone();
        // Each client's app id and ground, for the zoom's solid card.
        let faces:HashMap<ClientId,(String,Option<Vec4f>)>=state.clients.iter().map(|(c,s)|(*c,(s.app.clone(),s.ground))).collect();
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
        // Android's drawer is a sheet over the page AND its live tiles.
        if phone.drawer>0.001 && style==crate::desktop::DesktopStyle::Android {
            let state=scope.data.get_mut::<WmState>().unwrap();
            self.phone_ui.draw_android_drawer_layer(cx,state,screen);
            self.compositor.as_mut().unwrap().content(screen);
        }
        if phone.overview>0.001 {
            let blur = (phone.overview.clamp(0.0, 1.0) * 3.0) as f32;
            self.phone_ui.overview_glass.set_blurriness(cx, blur);
            let backdrop=self.compositor.as_mut().unwrap().backdrop(cx,screen,blur as f64);
            self.phone_ui.overview_glass.draw_surface_with_backdrop(cx,screen,Some(backdrop),phone.overview as f32);
            self.phone_ui.draw_recents_scrim(cx,screen,style,dark,phone.overview as f32);
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
        // Its navigation band is its bottom rows stretched down, the same way.
        let open=phone.overview<0.001 && phone.screen==PhoneScreen::App;
        let band=open.then(|| Rect {pos:screen.pos,size:dvec2(screen.size.x,phone.chrome.top_reserve(screen))})
            .filter(|b|b.size.y>0.5);
        let nav_band=open.then(|| {
            let h=phone.chrome.bottom_reserve(screen);
            Rect {pos:dvec2(screen.pos.x,screen.pos.y+screen.size.y-h),size:dvec2(screen.size.x,h)}
        }).filter(|b|b.size.y>0.5);
        let mut band_painted=false;
        let mut band_luma:(Option<f32>,Option<f32>)=(None,None);
        let band_key=BandKey {size:((app.size.x*8.0) as i64,(app.size.y*8.0) as i64),dpi_milli:(cx.current_dpi_factor()*1000.0).round() as i64,style,dark};

        for client in order {
            // The client being pulled between its full rect and its card:
            // only while it is open at all. On Home (and a switcher entered
            // from Home) every client is a card, faded in by `overview`.
            let foreground=phone.client==Some(client) && phone.openness>0.001;
            if !foreground && phone.overview<0.001 {continue;}
            if foreground && phone.openness<0.001 {continue;}
            let index=phone.order.iter().position(|c|*c==client).unwrap_or(0);
            let card=mobile::card_rect_for(style,screen,phone.chrome,index as f64,phone.cards.page);
            let (app_id,ground)=faces.get(&client).cloned().unwrap_or_default();
            let mut lands=true;
            let mut zoom_from=Rect::default();
            let mut display=if foreground {
                let (icon,has_place)=self.phone_zoom_origin(&phone,&launchable,style,screen,app,client,&app_id);
                lands=has_place;
                zoom_from=icon;
                let springs=mobile::mix_rect(mobile::mix_rect(icon,app,phone.openness),card,phone.overview);
                // Held by a bottom swipe: exactly where the finger put it;
                // let go, it travels straight from there to where the
                // springs are heading (the icon, its card, or back to full)
                // — never through the springs' in-flight rect, which grows
                // back toward full size on its way home.
                match phone.lift {
                    Some(lift)=>{
                        let (open,overview)=phone.spring_targets();
                        let end=mobile::mix_rect(mobile::mix_rect(icon,app,open),card,overview);
                        mobile::mix_rect(if phone.gesture.is_some() {springs} else {end},lift.rect,lift.blend)
                    }
                    None=>springs,
                }
            }else{card};
            if phone.gesture.as_ref().is_some_and(|g|g.hit==Some(PhoneHit::Card(client))) {display.pos.y+=phone.dismiss_y;}
            if display.pos.x+display.size.x<screen.pos.x || display.pos.x>screen.pos.x+screen.size.x {continue;}
            // A tile client's window frames are trusted only in its full
            // face: while it shows (or switches to) the compact face the
            // card keeps the last full-screen capture, whatever arrives.
            let full_ready=phone.tiles.get(client).map_or(true,|t|t.full_ready());
            let mut stored=self.phone_frames.remove(&client).unwrap_or_default();
            let radius=((1.0-phone.openness).max(phone.overview)*HomeMetrics::of(style).card_radius)as f32;
            // Zooming between its icon and full screen the app is one solid
            // card: its own ground and icon from the first frame (the icon it
            // grew out of), the app's picture crossfading in over that
            // during the first half of the zoom, out during the last half
            // of the way back. Nothing of the home page shows through it.
            // A swipe home from an app raised `overview` with the finger; the
            // app still shrinks home as one solid card, not a fading card.
            let zooming=foreground && phone.screen!=PhoneScreen::Recents && phone.openness<0.999;
            let opacity=if zooming {((phone.openness-0.05)/0.45).clamp(0.0,1.0) as f32}
                else if foreground {phone.openness.max(phone.overview) as f32}else{phone.overview as f32};
            if zooming {
                let solid=(phone.openness/if lands {0.05}else{0.5}).clamp(0.0,1.0) as f32;
                // Zooming out of (or back into) its live home tile: the
                // tile's own last face is the card's ground, so the app
                // crossfades into the tile instead of landing on a blank
                // icon card that the tile then replaces.
                let tile_ground=stored.tile.as_ref().filter(|c|lands && !c.stale(zoom_from.size,style,dark));
                if let Some(tile)=tile_ground {
                    self.present_capture(cx,tile,display,solid,radius);
                } else {
                    self.phone_ui.draw_launch_card_ground(cx,display,&app_id,style,dark,ground,solid,radius);
                }
            }
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
                    // The app's content changed: a frame of its arrived, the
                    // capture's size or look changed, or the app asked for a
                    // redraw of what it draws (its area's draw list is due).
                    let content_changed=capture.stale(app.size,style,dark) || self.items.get(&client)
                        .and_then(|item|item.area().draw_list_id())
                        .map_or(true,|list|cx.draw_event.draw_list_will_redraw(cx,list));
                    if refresh {
                        self.record_capture(cx,scope,client,&mut capture,app,true);
                        capture.settle(app.size,style,dark);
                    }else{capture.frame.freeze(cx);}
                    if crossfade {self.present_capture_window(cx,&capture,app,display,opacity,radius);}
                    else {self.present_capture(cx,&capture,display,opacity,radius);}
                    if refresh {stored.bands.recorded(band_key,content_changed);}
                    self.present_bands(cx,&mut stored,&capture,app,foreground && phone.openness>0.999,band,nav_band,opacity,ground,band_key,&mut band_painted,&mut band_luma);
                    stored.full=Some(capture);
                    if foreground && (band_luma.0.is_some() || band_luma.1.is_some()) {self.ask_band_sample(cx,&mut stored,app,band_key);}
                }
                None if full_ready=>{
                    let mut capture=Capture::new(cx,style,dark);
                    self.record_capture(cx,scope,client,&mut capture,app,true);
                    capture.settle(app.size,style,dark);
                    if crossfade {self.present_capture_window(cx,&capture,app,display,opacity,radius);}
                    else {self.present_capture(cx,&capture,display,opacity,radius);}
                    stored.bands.recorded(band_key,true);
                    self.present_bands(cx,&mut stored,&capture,app,foreground && phone.openness>0.999,band,nav_band,opacity,ground,band_key,&mut band_painted,&mut band_luma);
                    stored.full=Some(capture);
                    if foreground && (band_luma.0.is_some() || band_luma.1.is_some()) {self.ask_band_sample(cx,&mut stored,app,band_key);}
                }
                None=>{
                    // Opened straight from its tile and no full-size frame
                    // yet: a plain launch card, never the squeezed tile.
                    self.phone_ui.draw_launch_card_ground(cx,display,&app_id,style,dark,ground,1.0,radius);
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
        // What the simulated bars pick their ink against.
        self.phone_ui.band_luma=band_luma;
        self.phone_ui.draw_overlay(cx,state,screen,backdrop);
        // The super-app's first run: what is streaming out of the APK,
        // above the dock while the home page shows.
        if let Some(text)=state.provision.clone() {
            if state.phone.home_visible() {
                let fade=(1.0-state.phone.openness).clamp(0.0,1.0) as f32;
                self.phone_ui.draw_provision_band(cx,screen,state.phone.chrome,state.style.target,state.style.dark,fade,&text);
            }
        }
    }
    pub(super) fn handle_phone_event(&mut self,cx:&mut Cx,event:&Event,scope:&mut Scope) {
        if let Event::Signal=event {self.take_band_readbacks(cx);}
        if let Event::Timer(_)=event {self.band_retry(cx,event);}
        let state=scope.data.get_mut::<WmState>().unwrap();
        let input=matches!(event,Event::TouchUpdate(_)|Event::MouseDown(_)|Event::MouseUp(_)|Event::MouseMove(_)|Event::Scroll(_)|Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_));
        let client=state.phone.client;
        let accepts=state.phone.accepts_app_input();
        // The desktop skin is a phone: the primary mouse button is a finger
        // to the app, so its lists drag and fling and its rows tell a tap
        // from a scroll the way they do on the device. Hover does not exist
        // on a touch screen; the wheel and the keyboard stay as they are.
        // The press starts here, once the shell has passed on it; its moves
        // and its lift are routed before the shell sees them
        // (`phone_finger_route`).
        let simulated=state.phone.chrome.fake_status() && client.is_some_and(|c|self.module_clients.contains(&c));
        if simulated && matches!(event,Event::MouseMove(_)|Event::MouseDown(_)|Event::MouseUp(_)) {
            if let (Event::MouseDown(e),Some(client),true,None)=(event,client,accepts,self.phone_finger.as_ref()) {
                if e.button.contains(MouseButton::PRIMARY) {
                    self.phone_finger=Some(PhoneFinger {client,abs:e.abs,time:e.time,window_id:e.window_id,modifiers:e.modifiers});
                    self.dispatch_phone_finger(cx,TouchState::Start);
                }
            }
            return;
        }
        if input && !accepts {return;}
        let items:Vec<_>=self.items.iter().filter(|(c,_)|!input || Some(**c)==client).map(|(_,w)|w.clone()).collect();
        for item in items {item.handle_event(cx,event,scope);}
    }
    pub fn phone_finger_active(&self)->bool {self.phone_finger.is_some()}
    /// The finger is down on `client`.
    pub fn phone_finger_on(&self,client:ClientId)->bool {self.phone_finger.as_ref().is_some_and(|f|f.client==client)}
    /// The simulated finger's moves and lift, taken before the shell's own
    /// pointer handling so nothing in between (the toolbar, a menu, a
    /// gesture zone) can swallow its release. A finger whose app is no
    /// longer the one in front taking input is cancelled instead. True when
    /// the event was the finger's.
    pub fn phone_finger_route(&mut self,cx:&mut Cx,event:&Event,front:Option<ClientId>,accepts:bool)->bool {
        let Some(finger)=self.phone_finger.as_ref() else {return false};
        if !matches!(event,Event::MouseMove(_)|Event::MouseDown(_)|Event::MouseUp(_)) {return false;}
        if front!=Some(finger.client) || !accepts || !self.items.contains_key(&finger.client) {
            self.cancel_phone_finger(cx);
            return true;
        }
        match event {
            Event::MouseMove(e)=>{
                let f=self.phone_finger.as_mut().unwrap();
                f.abs=e.abs;f.time=e.time;f.modifiers=e.modifiers;
                self.dispatch_phone_finger(cx,TouchState::Move);
            }
            Event::MouseUp(e) if e.button.contains(MouseButton::PRIMARY)=>{
                let f=self.phone_finger.as_mut().unwrap();
                f.abs=e.abs;f.time=e.time;f.modifiers=e.modifiers;
                self.dispatch_phone_finger(cx,TouchState::Stop);
                self.phone_finger=None;
            }
            // Another button while the finger is down belongs to nobody.
            _=>{}
        }
        true
    }
    /// The finger is taken away mid-press (the phone rotated, the app
    /// closed or left the front, the style changed, the window lost focus,
    /// the shell navigated): every capture of it is cancelled, the app gets
    /// `Event::FingerCancel` — each widget still holding the press ends it
    /// with a cancelled FingerUp (no click, no fling), wherever the finger
    /// is — and the digit is retired, even if nothing was left to hear it.
    pub fn cancel_phone_finger(&mut self,cx:&mut Cx) {
        let Some(f)=self.phone_finger.take() else {return};
        let digit_id:DigitId=live_id_num!(touch,PHONE_FINGER).into();
        cx.fingers.cancel_digit(digit_id);
        let cancel=Event::FingerCancel(FingerCancelEvent {
            window_id:f.window_id,digit_id,device:DigitDevice::Touch {uid:PHONE_FINGER},
            // Its own moment: the cancel is one dispatch of its own, never
            // the dispatch that already showed a claim's cancel to a widget.
            abs:f.abs,time:cx.seconds_since_app_start().max(f.time+1e-6),modifiers:f.modifiers,
        });
        if let Some(item)=self.items.get(&f.client).cloned() {
            item.handle_event(cx,&cancel,&mut Scope::empty());
        }
        cx.fingers.process_touch_update_end(&[TouchPoint {
            state:TouchState::Stop,abs:f.abs,time:f.time,uid:PHONE_FINGER,rotation_angle:0.0,force:1.0,radius:dvec2(1.0,1.0),
            handled:std::cell::Cell::new(Area::Empty),sweep_lock:std::cell::Cell::new(Area::Empty),
        }]);
    }
    fn dispatch_phone_finger(&mut self,cx:&mut Cx,state:TouchState) {
        let Some(f)=self.phone_finger.as_ref() else {return};
        let touch=TouchUpdateEvent {time:f.time,window_id:f.window_id,modifiers:f.modifiers,touches:vec![TouchPoint {
            state,abs:f.abs,time:f.time,uid:PHONE_FINGER,rotation_angle:0.0,force:1.0,radius:dvec2(1.0,1.0),
            handled:std::cell::Cell::new(Area::Empty),sweep_lock:std::cell::Cell::new(Area::Empty),
        }]};
        // The press's tap count is the mouse press's own (the platform
        // counted it); only the capture/hover bookkeeping runs after.
        let event=Event::TouchUpdate(touch);
        if let Some(item)=self.items.get(&f.client).cloned() {
            item.handle_event(cx,&event,&mut Scope::empty());
        }
        if let Event::TouchUpdate(touch)=&event {cx.fingers.process_touch_update_end(&touch.touches);}
    }
}

/// Until the GPU has said how light the band is: the app's theme ground.
fn ground_luma(ground:Option<Vec4f>)->f32 {
    ground.map_or(1.0,|g| {
        let lin=|c:f32| if c<=0.04045 {c/12.92} else {((c+0.055)/1.055).powf(2.4)};
        0.2126*lin(g.x)+0.7152*lin(g.y)+0.0722*lin(g.z)
    })
}

/// The simulated phone's one finger: far from any real touch id.
const PHONE_FINGER:u64=0x5157_0000_0001;

/// The primary mouse button held on the open app, presented to it as a
/// finger: which client it landed on and where it is now.
pub(super) struct PhoneFinger {
    client: ClientId,
    abs: Vec2d,
    time: f64,
    window_id: WindowId,
    modifiers: KeyModifiers,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A readback `width` wide whose rows are `row(y)`: RGBA bytes, top first.
    fn readback(width: usize, height: usize, row: impl Fn(usize, usize) -> [u8; 3]) -> TextureReadback {
        let mut data = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = row(x, y);
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        TextureReadback {
            ticket: ReadbackTicket(1), allocation_generation: 0, producer_serial: 0,
            width, height, stride: width * 4, channel_order: ReadbackChannelOrder::Rgba, origin: ReadbackOrigin::TopLeft,
            data: Ok::<Arc<[u8]>, ReadbackError>(data.into()),
        }
    }

    /// Both bands' rows fit the strip's texture at every density, the
    /// fractional ones included (the texture is the points times the
    /// density, truncated), and a sample of it reads back.
    #[test]
    fn band_strip_rows_fit_at_any_density() {
        for dpi in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 2.8125, 3.0, 3.5] {
            let (rows, height) = strip_rows(dpi);
            assert!(rows as f64 >= BAND_ROWS * dpi, "{dpi}");
            let texture = (height * dpi) as usize;
            assert!(texture >= rows * 2, "dpi {dpi}: {texture} px for 2 x {rows} rows");
            let strip = readback(40, texture, |_, _| [200, 200, 200]);
            assert!(band_sample(&strip, 0, rows).is_some(), "dpi {dpi}");
        }
    }

    /// A sample that failed or could not be used counts as taken: the
    /// bands are clean and nothing asks again until the app changes. One
    /// for a key the app has left changes nothing.
    #[test]
    fn a_failed_band_sample_does_not_retry() {
        let key = BandKey { size: (100, 100), dpi_milli: 2000, style: crate::desktop::DesktopStyle::Android, dark: false };
        let mut bands = Bands::default();
        bands.recorded(key, true);
        assert!(bands.dirty());
        assert!(!bands.finish(key, bands.revision, None));
        assert!(!bands.dirty(), "an unusable result is not asked for again");
        // Unchanged content recorded again: still clean.
        bands.recorded(key, false);
        assert!(!bands.dirty());
        // A sample for the old key after a rotation: ignored, still dirty.
        let turned = BandKey { size: (100, 50), ..key };
        bands.recorded(turned, false);
        assert!(bands.dirty());
        let rows = band_sample(&readback(40, 8, |_, _| [250, 250, 250]), 0, 4);
        assert!(!bands.finish(key, bands.revision, rows));
        assert!(bands.dirty());
        assert!(bands.finish(turned, bands.revision, rows));
        assert!(!bands.dirty() && bands.top.is_some());
    }

    /// A sky (a smooth gradient) is plain and dark: the band stretches it
    /// and draws light ink. A white bar with a line of text crossing it is
    /// light and busy: a solid band, dark ink.
    #[test]
    fn band_rows_tell_a_sky_from_text_and_light_from_dark() {
        let sky = readback(400, 40, |x, _| [40, 80 + (x / 8) as u8, 150]);
        let text = readback(400, 40, |x, y| if y > 30 && x % 12 < 3 { [30, 30, 30] } else { [250, 248, 255] });
        let (sky_top, _) = band_sample(&sky, 2, 4).unwrap();
        assert!(sky_top.plain && sky_top.luma < 0.2);
        assert_eq!(sky_top.solid(), None);
        let (text_top, text_bottom) = band_sample(&text, 2, 4).unwrap();
        assert!(text_top.plain && text_top.luma > 0.8);
        assert!(!text_bottom.plain, "text across the bottom rows");
        assert!(text_bottom.luma > 0.5);
        assert!(text_bottom.solid().is_some());
    }
}
