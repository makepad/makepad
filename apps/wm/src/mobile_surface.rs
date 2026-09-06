//! Phone chrome drawn around compositor-owned application surfaces.
use crate::{desktop::DesktopStyle, desk::WmState, mobile::*, mobile_tiles::{self, HomeLayout, TileSlot, TILE_RADIUS}, shell::{alpha, rgb, ui::{rect, HAlign, Ico, ShellDraw}}};
use makepad_widgets::{app_icon::AppIconDraw, gauss_view::{GaussRoundedView, GaussBlurSnapshot}, *};
use crate::desktop::DrawDesktopChrome;
mod search;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.PhoneSurfaceBase = #(PhoneSurface::register_widget(vm))
    mod.widgets.PhoneSurface = set_type_default() do mod.widgets.PhoneSurfaceBase {
        width: Fill height: Fill
        d +: {text.text_style: theme.font_regular text_bold.text_style: theme.font_bold}
        ios_font: theme.font_regular{font_family: FontFamily{latin := FontMember{res: crate_resource("self:../../widgets/resources/Inter.ttf") weight: 400.0 asc: 0.0 desc: 0.0}}}
        ios_bold: theme.font_bold{font_family: FontFamily{latin := FontMember{res: crate_resource("self:../../widgets/resources/Inter.ttf") weight: 600.0 asc: 0.0 desc: 0.0}}}
        android_font: theme.font_regular{font_family: FontFamily{latin := FontMember{res: crate_resource("self:../../widgets/resources/RobotoFlex.ttf") weight: 400.0 asc: 0.0 desc: 0.0}}}
        android_bold: theme.font_bold{font_family: FontFamily{latin := FontMember{res: crate_resource("self:../../widgets/resources/RobotoFlex.ttf") weight: 600.0 asc: 0.0 desc: 0.0}}}
        chrome +: {}
        key_shift +: {svg: crate_resource("self:resources/icons/key-shift.svg")}
        key_backspace +: {svg: crate_resource("self:resources/icons/key-backspace.svg")}
        search: View {
            width: Fill height: Fill
            input := TextInputFlat {
                width: Fill height: Fill margin: 0
                padding: Inset{left: 38 right: 32 top: 0 bottom: 0}
                label_align: Align{y: 0.5}
                empty_text: "App Library"
                return_key_type: Search
                draw_bg +: {pixel: fn() {return vec4(0.0)}}
                draw_text +: {text_style: theme.font_regular{font_size: 14.0}}
                draw_cursor +: {color: #007aff}
                draw_selection +: {color: #007aff40}
            }
        }
        glass: GlassPanel {
            draw_bg +: {
                blur_level: 4.0 corner_radius: 30.0
                tint_color: #eeeeff tint_alpha: 0.22 surface_alpha: 0.88
                lensing_strength: 0.4 specular_strength: 0.10
                border_alpha: 0.18 border_width: 0.7
            }
        }
        overview_glass: GlassPanel {
            draw_bg +: {
                blur_level: 3.0 corner_radius: 0.0
                tint_color: #101329 tint_alpha: 0.20 surface_alpha: 1.0
                lensing_strength: 0.0 specular_strength: 0.0 border_alpha: 0.0
            }
        }
        keyboard_glass: GlassPanel {
            draw_bg +: {
                blur_level: 4.0 corner_radius: 0.0
                tint_color: #d7d8dd tint_alpha: 0.86 surface_alpha: 1.0
                lensing_strength: 0.0 specular_strength: 0.0 border_alpha: 0.0
            }
        }
        wallpaper +: {
            android: instance(0.0)
            dark: instance(0.0)
            phase: instance(0.0)
            pixel: fn() {
                let p=self.pos
                let t=self.phase
                let aspect=self.rect_size.x/max(self.rect_size.y,1.0)
                let q=(p-0.5)*vec2(aspect,1.0)
                // Slowly drifting translucent ribbons; no per-pixel loop.
                let bend=sin(p.y*4.2+t*0.11)*0.19+sin(p.y*8.0-t*0.07)*0.045
                let ribbon=exp(-pow((p.x+bend-0.30-sin(t*0.08)*0.12)*3.3,2.0))
                let edge=exp(-pow((p.x+bend-0.52)*17.0,2.0))
                let bloom=exp(-length(q-vec2(sin(t*0.06)*0.22,cos(t*0.09)*0.30))*3.0)
                let warm=exp(-pow((p.x-bend-0.84+sin(t*0.05)*0.12)*3.8,2.0))
                let ios=vec3(0.025,0.085,0.24)+vec3(0.05,0.48,0.57)*ribbon
                    +vec3(0.17,0.30,0.33)*edge+vec3(0.34,0.04,0.25)*warm+vec3(0.06,0.10,0.14)*bloom
                // Material-style cut-paper petals with soft depth and living color.
                let angle=atan2(q.y,q.x)+t*0.035
                let petals=0.31+0.065*cos(angle*4.0+sin(t*0.07)*0.5)
                let radius=length(q-vec2(sin(t*0.055)*0.08,cos(t*0.04)*0.06))
                let shape=1.0-smoothstep(petals-0.012,petals+0.012,radius)
                let shadow=1.0-smoothstep(petals,petals+0.07,radius)
                let inner=1.0-smoothstep(0.12,0.16,length(q+vec2(0.06,0.09)))
                let paper=mix(vec3(0.88,0.82,0.96),vec3(0.63,0.79,0.89),p.y)*mix(1.0,0.90,shadow)
                let petal=mix(vec3(0.39,0.43,0.72),vec3(0.66,0.54,0.79),clamp(p.y+sin(t*0.08)*0.15,0.0,1.0))
                let android=mix(mix(paper,petal,shape),vec3(0.95,0.71,0.63),inner)
                let color=mix(ios,android,self.android)*(1.0-self.dark*0.64)
                return vec4(color,1.0)
            }
        }
    }
}

/// The dock's four apps, left to right.
const PINNED: [&str; 4] = ["browser", "files", "photos", "terminal"];

#[derive(Script, ScriptHook, Widget)]
pub struct PhoneSurface {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[visible] #[live(true)] visible: bool,
    #[live] d: ShellDraw,
    #[live] ios_font: TextStyle,
    #[live] ios_bold: TextStyle,
    #[live] android_font: TextStyle,
    #[live] android_bold: TextStyle,
    #[live] chrome: DrawDesktopChrome,
    #[live] key_shift: DrawSvg,
    #[live] key_backspace: DrawSvg,
    #[live] glass: GaussRoundedView,
    #[live] keyboard_glass: GaussRoundedView,
    #[live] pub overview_glass: GaussRoundedView,
    #[rust] pressed: Option<PhoneHit>,
    #[live] wallpaper: DrawQuad,
    #[rust] icons: AppIconDraw,
    #[rust] hits: Vec<(Rect, PhoneHit)>,
    #[find] #[live] search: WidgetRef,
    #[rust] search_style: Option<(bool, bool)>,
    #[rust] search_rect: Rect,
    #[rust] search_pointer: bool,
    #[rust] pub search_scroll_max: f64,
    #[rust] pub pad_left: f64,
    #[redraw] #[rust] area: Area,
}
impl PhoneSurface {
    pub fn hit(&self, p: Vec2d) -> Option<PhoneHit> {
        self.hits.iter().rev().find(|(r,_)| r.contains(p)).map(|(_,h)|h.clone())
    }
    pub fn hit_rect(&self, hit: &PhoneHit) -> Option<Rect> {
        self.hits.iter().find(|(_, h)| h == hit).map(|(r, _)| *r)
    }
    pub fn begin(&mut self) { self.hits.clear(); }
    fn rounded(&mut self, cx: &mut Cx2d, r: Rect, radius: f32, color: Vec4f) {
        self.chrome.radius = radius*2.0;
        self.chrome.bevel = 0.0; self.chrome.color = color;
        self.chrome.draw_abs(cx,r);
    }
    fn label(&mut self, cx: &mut Cx2d, r: Rect, label: &str, size: f64, bold: bool, color: Vec4f) {
        self.d.label_elided(cx,r,bold,size,color,HAlign::Center,label);
    }
    fn use_fonts(&mut self, ios: bool) {
        self.d.text.text_style=if ios {self.ios_font.clone()}else{self.android_font.clone()};
        self.d.text_bold.text_style=if ios {self.ios_bold.clone()}else{self.android_bold.clone()};
    }
    pub fn draw_wallpaper(&mut self, cx: &mut Cx2d, screen: Rect, style: DesktopStyle, dark: bool, phase: f64) {
        self.wallpaper.draw_vars.set_dyn_instance(cx, live_id!(android), &[if style==DesktopStyle::Android {1.0}else{0.0}]);
        self.wallpaper.draw_vars.set_dyn_instance(cx, live_id!(dark), &[if dark {1.0}else{0.0}]);
        self.wallpaper.draw_vars.set_dyn_instance(cx, live_id!(phase), &[phase as f32]);
        self.wallpaper.draw_abs(cx,screen);
    }
    pub fn home_dock(screen: Rect) -> Rect {
        let landscape=screen.size.x>screen.size.y;
        let w=screen.size.x.min(if landscape {380.0}else{1000.0})-24.0;
        rect(screen.pos.x+(screen.size.x-w)*0.5,screen.pos.y+screen.size.y-112.0,w,82.0)
    }
    /// Where the home page's content starts: under the status bar, and on
    /// Android's portrait home under the big clock.
    fn home_top(style: DesktopStyle, screen: Rect) -> f64 {
        let landscape=screen.size.x>screen.size.y;
        screen.pos.y + if landscape {44.0} else if style==DesktopStyle::Ios {70.0} else {156.0}
    }
    /// The home page's regions for this screen: tiles, favorites, dock.
    pub fn home_layout(style: DesktopStyle, screen: Rect) -> HomeLayout {
        mobile_tiles::home_layout(screen, Self::home_top(style, screen), Self::home_dock(screen))
    }
    /// Where a window zooms out of and back into: its tile for a tile app,
    /// the dock's centre otherwise.
    pub fn launch_origin(style: DesktopStyle, screen: Rect, app: Option<&str>) -> Rect {
        if let Some(app)=app {
            if let Some(slot)=Self::home_layout(style,screen).tiles.into_iter().find(|s|s.app==app) {
                return slot.rect;
            }
        }
        Rect {pos:screen.pos+dvec2(screen.size.x*0.5-30.0,screen.size.y-96.0),size:dvec2(60.0,60.0)}
    }
    fn app_label(id: &str) -> String {
        crate::clients::find_app(id).map(|a| a.label).unwrap_or_else(|| id.to_string())
    }
    fn card_colors(style: DesktopStyle, dark: bool) -> (Vec4f, Vec4f) {
        let ios=style==DesktopStyle::Ios;
        let face=if dark {rgb(30,32,46)} else if ios {rgb(246,247,252)} else {rgb(255,251,255)};
        let ink=if dark {rgb(240,240,248)} else {rgb(28,27,36)};
        (face, ink)
    }
    /// A home tile without a live capture yet: the app's identity and what
    /// the launcher is doing for it (compiling, starting, could not start).
    pub fn draw_tile_placeholder(&mut self, cx: &mut Cx2d, slot: TileSlot, style: DesktopStyle, dark: bool, opacity: f32, headline: &str, detail: &str) {
        self.use_fonts(style==DesktopStyle::Ios);
        let (face, ink)=Self::card_colors(style, dark);
        let r=slot.rect;
        self.rounded(cx, r, TILE_RADIUS as f32, alpha(face, 0.82*opacity));
        let wide=slot.kind==mobile_tiles::TileKind::Wide;
        let icon=if wide {52.0} else {46.0};
        let ink=alpha(ink, opacity);
        if wide {
            // Icon on the left, the text beside it.
            let ix=r.pos.x+22.0;
            self.icons.draw(cx,slot.app,style,rect(ix,r.pos.y+(r.size.y-icon)*0.5,icon,icon),opacity,ink);
            let text=rect(ix+icon+18.0,r.pos.y,r.size.x-(icon+58.0),r.size.y);
            let mid=text.pos.y+text.size.y*0.5;
            self.d.label_elided(cx,rect(text.pos.x,mid-34.0,text.size.x,24.0),true,15.0,ink,HAlign::Left,&Self::app_label(slot.app));
            self.d.label_elided(cx,rect(text.pos.x,mid-8.0,text.size.x,22.0),false,13.0,alpha(ink,0.8*opacity),HAlign::Left,headline);
            self.d.label_elided(cx,rect(text.pos.x,mid+14.0,text.size.x,20.0),false,10.5,alpha(ink,0.55*opacity),HAlign::Left,detail);
        } else {
            let top=r.pos.y+r.size.y*0.5-icon*0.5-26.0;
            self.icons.draw(cx,slot.app,style,rect(r.pos.x+(r.size.x-icon)*0.5,top,icon,icon),opacity,ink);
            self.label(cx,rect(r.pos.x+10.0,top+icon+8.0,r.size.x-20.0,22.0),&Self::app_label(slot.app),14.0,true,ink);
            self.label(cx,rect(r.pos.x+10.0,top+icon+30.0,r.size.x-20.0,20.0),headline,12.0,false,alpha(ink,0.8*opacity));
            if !detail.is_empty() && r.size.y>150.0 {
                self.label(cx,rect(r.pos.x+10.0,top+icon+50.0,r.size.x-20.0,18.0),detail,9.5,false,alpha(ink,0.55*opacity));
            }
        }
    }
    /// A window opened straight from its tile, before its first full-size
    /// frame: the launch card the zoom-in plays over.
    pub fn draw_launch_card(&mut self, cx: &mut Cx2d, r: Rect, app: &str, style: DesktopStyle, dark: bool, opacity: f32, radius: f32) {
        let (face, ink)=Self::card_colors(style, dark);
        self.rounded(cx, r, radius, alpha(face, opacity));
        let size=(r.size.x.min(r.size.y)*0.3).clamp(24.0,72.0);
        self.icons.draw(cx,app,style,rect(r.pos.x+(r.size.x-size)*0.5,r.pos.y+(r.size.y-size)*0.5,size,size),opacity,alpha(ink,opacity));
    }
    pub fn draw_home(&mut self, cx: &mut Cx2d, state: &WmState, screen: Rect, backdrop: Option<GaussBlurSnapshot>) {
        let phone=&state.phone;
        let style=state.style.target;
        let ios=style==DesktopStyle::Ios;
        self.use_fonts(ios);
        let opacity=(1.0-phone.openness*0.85) as f32;
        if opacity<0.01 {return;}
        let landscape=screen.size.x>screen.size.y;
        let apps=crate::shell::launcher::apps();
        let ids: Vec<(String,String)>=apps.iter().map(|a|(a.id.trim_start_matches("apps.").to_string(),a.label.clone())).collect();
        if phone.screen==PhoneScreen::Drawer {
            if ios {self.draw_app_library(cx,state,screen,&ids);} else {self.draw_android_drawer(cx,state,screen,&ids);}
            return;
        }
        let ink=if !ios && !state.style.dark {rgb(31,27,38)}else{rgb(255,255,255)};
        if !ios && !landscape {
            self.label(cx,rect(screen.pos.x+24.0,screen.pos.y+48.0,screen.size.x-48.0,58.0),&phone.clock,48.0,false,alpha(ink,opacity));
            self.label(cx,rect(screen.pos.x+24.0,screen.pos.y+110.0,screen.size.x-48.0,26.0),"Makepad",15.0,false,alpha(ink,opacity*0.8));
        }
        let layout=Self::home_layout(style,screen);
        let home=phone.screen==PhoneScreen::Home;
        // The tiles themselves are composited by the desk (their captures
        // or placeholders); the page owns their hit regions.
        if home {
            for slot in &layout.tiles {self.hits.push((slot.rect,PhoneHit::App(slot.app.into())));}
        }
        // Favorites: the launcher's order minus the dock, as many as fit
        // above it. The rest live in the App Library / the drawer.
        let favorites: Vec<&(String,String)>=ids.iter().filter(|(id,_)|!PINNED.contains(&id.as_str())).take(layout.capacity).collect();
        let cell=layout.favorites.size.x/layout.columns as f64;
        let size=if landscape {44.0}else{60.0};
        for (index,(id,label)) in favorites.iter().enumerate() {
            let r=rect(layout.favorites.pos.x+(index%layout.columns)as f64*cell,layout.favorites.pos.y+(index/layout.columns)as f64*layout.row_height,cell,layout.row_height);
            self.icons.draw(cx,id,style,rect(r.pos.x+(cell-size)*0.5,r.pos.y,size,size),opacity,ink);
            self.label(cx,rect(r.pos.x,r.pos.y+size+4.0,cell,20.0),label,11.0,false,alpha(ink,opacity));
            if home {self.hits.push((r,PhoneHit::App(id.clone())));}
        }
        let dock=Self::home_dock(screen);
        if ios {self.glass.draw_surface_with_backdrop(cx,dock,backdrop,opacity);}
        let cell=dock.size.x/4.0;
        for (index,id) in PINNED.iter().enumerate() {
            let r=rect(dock.pos.x+index as f64*cell,dock.pos.y,cell,dock.size.y);
            self.icons.draw(cx,id,style,rect(r.pos.x+(cell-58.0)*0.5,r.pos.y+12.0,58.0,58.0),opacity,ink);
            if home {self.hits.push((r,PhoneHit::App((*id).into())));}
        }
        if ios {
            // The page indicator: this page, then the App Library's dot.
            // Tapping it (or swiping left) opens the library.
            let r=rect(screen.pos.x+(screen.size.x-60.0)*0.5,dock.pos.y-30.0,60.0,24.0);
            self.rounded(cx,rect(r.pos.x+15.0,r.pos.y+8.0,8.0,8.0),4.0,alpha(ink,opacity));
            let lib=rect(r.pos.x+34.0,r.pos.y+6.0,12.0,12.0);
            self.rounded(cx,lib,3.0,alpha(ink,0.45*opacity));
            for (dx,dy) in [(2.5,2.5),(6.5,2.5),(2.5,6.5),(6.5,6.5)] {
                self.rounded(cx,rect(lib.pos.x+dx,lib.pos.y+dy,3.0,3.0),1.0,alpha(ink,0.9*opacity));
            }
            if home {self.hits.push((rect(r.pos.x-20.0,r.pos.y-6.0,100.0,36.0),PhoneHit::Drawer));}
        } else if home {
            let r=rect(screen.pos.x+(screen.size.x-100.0)*0.5,dock.pos.y-32.0,100.0,28.0);
            self.label(cx,r,"All apps  ↑",12.0,false,ink);self.hits.push((r,PhoneHit::Drawer));
        }
    }
    /// Android's app drawer: a sheet with every launchable app on one grid.
    fn draw_android_drawer(&mut self, cx: &mut Cx2d, state: &WmState, screen: Rect, ids: &[(String,String)]) {
        let style=state.style.target;
        let landscape=screen.size.x>screen.size.y;
        self.rounded(cx,screen,0.0,if state.style.dark {rgb(24,22,31)}else{rgb(249,245,255)});
        let ink=if state.style.dark {rgb(255,255,255)}else{rgb(31,27,38)};
        let pill=self.draw_search(cx,state,screen,ink);
        if state.phone.searching() {self.draw_search_results(cx,state,screen,pill,ids,ink);return;}
        let top=pill.pos.y+pill.size.y+18.0;
        let columns=if landscape {7}else{4};
        let cell=(screen.size.x-24.0)/columns as f64;
        let rows=(ids.len()+columns-1)/columns;
        let bottom=screen.pos.y+screen.size.y-38.0;
        let row_h=((bottom-top)/rows.max(1) as f64).clamp(64.0,104.0);
        let size=if landscape {44.0}else{60.0};
        for (index,(id,label)) in ids.iter().enumerate() {
            let r=rect(screen.pos.x+12.0+(index%columns)as f64*cell,top+(index/columns)as f64*row_h,cell,row_h);
            self.icons.draw(cx,id,style,rect(r.pos.x+(cell-size)*0.5,r.pos.y,size,size),1.0,ink);
            self.label(cx,rect(r.pos.x,r.pos.y+size+4.0,cell,20.0),label,11.0,false,ink);
            self.hits.push((r,PhoneHit::App(id.clone())));
        }
    }
    /// iOS's App Library: a search field over category cards, each card a
    /// folder with three large icons and a mini grid of the rest. Every
    /// icon launches; nothing is only decorative.
    fn draw_app_library(&mut self, cx: &mut Cx2d, state: &WmState, screen: Rect, ids: &[(String,String)]) {
        let style=state.style.target;
        let dark=state.style.dark;
        let landscape=screen.size.x>screen.size.y;
        // The library sits on a dimmed wallpaper; the cards are frosted.
        self.rounded(cx,screen,0.0,alpha(if dark {rgb(8,9,16)}else{rgb(228,231,242)},0.86));
        let ink=if dark {rgb(255,255,255)}else{rgb(26,26,32)};
        let pill=self.draw_search(cx,state,screen,ink);
        if state.phone.searching() {self.draw_search_results(cx,state,screen,pill,ids,ink);return;}
        let names: Vec<&str>=ids.iter().map(|(id,_)|id.as_str()).collect();
        let groups=mobile_tiles::app_library_groups(&names);
        // A card holds three large icons and a 2x2 mini grid: seven apps.
        let mut cards: Vec<(String, Vec<&str>)>=Vec::new();
        for (name, members) in groups {
            for (n, chunk) in members.chunks(7).enumerate() {
                cards.push((if n==0 {name.to_string()} else {format!("{name} {}", n+1)}, chunk.to_vec()));
            }
        }
        let columns=if landscape {4}else{2};
        let gap=16.0;
        let left=screen.pos.x+20.0;
        let cw=(screen.size.x-40.0-gap*(columns as f64-1.0))/columns as f64;
        let top=pill.pos.y+pill.size.y+18.0;
        let bottom=screen.pos.y+screen.size.y-30.0;
        let rows=(cards.len()+columns-1)/columns;
        let ch=cw.min(((bottom-top)/rows.max(1) as f64-24.0).max(72.0));
        let big=((cw-36.0)/2.0).min(64.0);
        let mini=((big-10.0)/2.0).max(12.0);
        for (index,(name,members)) in cards.iter().enumerate() {
            let x=left+(index%columns)as f64*(cw+gap);
            let y=top+(index/columns)as f64*(ch+24.0);
            let card=rect(x,y,cw,ch);
            self.rounded(cx,card,18.0,alpha(rgb(255,255,255),if dark {0.10}else{0.55}));
            let pad=12.0;
            let cell=((cw-pad*2.0)/2.0).min((ch-pad*2.0)/2.0);
            let ox=card.pos.x+(cw-cell*2.0)*0.5;
            let oy=card.pos.y+(ch-cell*2.0)*0.5;
            for (n,app) in members.iter().enumerate() {
                if n<3 {
                    let slot=rect(ox+(n%2)as f64*cell,oy+(n/2)as f64*cell,cell,cell);
                    let r=rect(slot.pos.x+(cell-big)*0.5,slot.pos.y+(cell-big)*0.5,big,big);
                    self.icons.draw(cx,app,style,r,1.0,ink);
                    self.hits.push((slot,PhoneHit::App((*app).to_string())));
                } else {
                    // The fourth cell: up to four more, small but tappable.
                    let k=n-3;
                    let slot=rect(ox+cell,oy+cell,cell,cell);
                    let inner=rect(slot.pos.x+(cell-big)*0.5,slot.pos.y+(cell-big)*0.5,big,big);
                    let step=big-mini;
                    let r=rect(inner.pos.x+(k%2)as f64*step,inner.pos.y+(k/2)as f64*step,mini,mini);
                    self.icons.draw(cx,app,style,r,1.0,ink);
                    self.hits.push((rect(r.pos.x-2.0,r.pos.y-2.0,mini+4.0,mini+4.0),PhoneHit::App((*app).to_string())));
                }
            }
            self.label(cx,rect(card.pos.x,card.pos.y+ch+2.0,cw,20.0),name,12.0,false,alpha(ink,0.85));
        }
    }
    pub fn draw_overlay(&mut self, cx: &mut Cx2d, state: &WmState, screen: Rect, backdrop: Option<GaussBlurSnapshot>) {
        let phone=&state.phone;
        self.pressed=phone.gesture.as_ref().and_then(|g|g.hit.clone());
        let ios=state.style.target==DesktopStyle::Ios;
        let ink=if (phone.screen==PhoneScreen::App || phone.screen==PhoneScreen::Drawer || !ios) && !state.style.dark {rgb(25,25,30)}else{rgb(255,255,255)};
        let status_h=if screen.size.x>screen.size.y {24.0}else{42.0};
        if phone.screen==PhoneScreen::App {self.rounded(cx,rect(screen.pos.x,screen.pos.y,screen.size.x,status_h),0.0,if state.style.dark {rgb(24,24,28)}else{rgb(248,248,252)});}
        self.label(cx,rect(screen.pos.x+16.0,screen.pos.y,62.0,status_h),&phone.clock,13.0,true,ink);
        if ios && screen.size.x<screen.size.y {self.rounded(cx,rect(screen.pos.x+screen.size.x*0.5-45.0,screen.pos.y+7.0,90.0,23.0),12.0,rgb(0,0,0));}
        if !ios && screen.size.x<screen.size.y {self.rounded(cx,rect(screen.pos.x+screen.size.x*0.5-5.0,screen.pos.y+13.0,10.0,10.0),5.0,rgb(0,0,0));}
        self.d.icon_centered(cx,Ico::Wifi,rect(screen.pos.x+screen.size.x-69.0,screen.pos.y,22.0,status_h),14.0,ink);
        self.rounded(cx,rect(screen.pos.x+screen.size.x-40.0,screen.pos.y+(status_h-11.0)*0.5,23.0,11.0),3.0,alpha(ink,0.45));
        self.rounded(cx,rect(screen.pos.x+screen.size.x-38.0,screen.pos.y+(status_h-7.0)*0.5,16.0,7.0),1.5,ink);
        if phone.overview>0.01 {
            for (index,client) in phone.order.iter().enumerate() {
                if let Some(slot)=state.clients.get(client) {
                    let card=card_rect(screen,index as f64,phone.page);
                    if card.pos.x+card.size.x<screen.pos.x || card.pos.x>screen.pos.x+screen.size.x {continue;}
                    self.icons.draw(cx,&slot.app,state.style.target,rect(card.pos.x+2.0,card.pos.y-36.0,26.0,26.0),phone.overview as f32,ink);
                    self.d.label_elided(cx,rect(card.pos.x+36.0,card.pos.y-36.0,card.size.x-36.0,26.0),true,13.0,alpha(rgb(255,255,255),phone.overview as f32),HAlign::Left,slot.display_title());
                    if phone.screen==PhoneScreen::Recents {self.hits.push((card,PhoneHit::Card(*client)));}
                }
            }
            if phone.order.is_empty() {self.label(cx,screen,"No recent apps",20.0,false,ink);}
        }
        if phone.keyboard>0.5 {self.draw_keyboard(cx,state,screen,backdrop);}
        let bottom=rect(screen.pos.x,screen.pos.y+screen.size.y-24.0,screen.size.x,24.0);
        if phone.screen==PhoneScreen::App || phone.keyboard>0.5 {
            self.rounded(cx,bottom,0.0,if state.style.dark {rgb(28,28,31)}else{rgb(244,244,248)});
        }
        let nav_ink=if phone.screen==PhoneScreen::App || phone.keyboard>0.5 {
            if state.style.dark {rgb(238,238,242)}else{rgb(30,30,34)}
        }else if phone.screen==PhoneScreen::Drawer && !state.style.dark {rgb(30,30,34)}else{rgb(255,255,255)};
        self.rounded(cx,rect(bottom.pos.x+bottom.size.x*0.5-60.0,bottom.pos.y+12.0,120.0,4.0),2.0,nav_ink);
        self.hits.push((bottom,PhoneHit::Home));
        if !ios && phone.keyboard>0.5 {
            let back=rect(bottom.pos.x+12.0,bottom.pos.y-10.0,40.0,34.0);
            self.d.icon_centered(cx,Ico::ChevronLeft,back,16.0,nav_ink);self.hits.push((back,PhoneHit::Back));
        }
    }
    fn draw_keyboard(&mut self, cx: &mut Cx2d, state: &WmState, screen: Rect, backdrop: Option<GaussBlurSnapshot>) {
        let phone=&state.phone;
        let ios=state.style.target==DesktopStyle::Ios;
        let dark=state.style.dark;
        let height=phone.keyboard_height();
        let r=rect(screen.pos.x,screen.pos.y+screen.size.y-phone.keyboard-24.0,screen.size.x,height);
        if ios && !dark {self.keyboard_glass.draw_surface_with_backdrop(cx,r,backdrop,1.0);}
        else {self.rounded(cx,r,0.0,if dark {rgb(34,32,40)}else{rgb(232,225,242)});}
        let ink=if dark {rgb(250,248,255)}else{rgb(30,28,36)};
        let hide=rect(r.pos.x+r.size.x-48.0,r.pos.y,44.0,30.0);
        self.d.icon_centered(cx,Ico::ChevronDown,hide,18.0,ink);self.hits.push((hide,PhoneHit::HideKeyboard));
        self.label(cx,rect(r.pos.x+48.0,r.pos.y,r.size.x-96.0,30.0),if phone.symbols {"Numbers & symbols"}else{"English"},12.0,false,alpha(ink,0.6));
        let mode=phone.keyboard_client.and_then(|c|phone.ime.get(&c)).map(|i|i.input_mode).unwrap_or_default();
        if matches!(mode,makepad_platform::ime::InputMode::Numeric|makepad_platform::ime::InputMode::Decimal|makepad_platform::ime::InputMode::Tel) {
            let rh=(height-36.0)/4.0;
            let unit=(r.size.x-12.0)/3.0;
            for (index,key) in ["1","2","3","4","5","6","7","8","9",if mode==makepad_platform::ime::InputMode::Tel {"+"}else{"."},"0","⌫"].iter().enumerate() {
                self.key(cx,rect(r.pos.x+6.0+(index%3)as f64*unit,r.pos.y+32.0+(index/3)as f64*rh,unit,rh),key,PhoneHit::Key(if index==11 {"backspace".into()}else{(*key).into()}),ios,dark,false);
            }
            return;
        }
        let rows=if phone.symbols {["1234567890","-/:;()$&@\"",".,?!'" ]}else{["qwertyuiop","asdfghjkl","zxcvbnm"]};
        let rh=(height-36.0)/4.0;
        let unit=(r.size.x-6.0)/10.0;
        for (row,text) in rows.iter().enumerate() {
            let count=text.chars().count();
            let left=r.pos.x+(r.size.x-unit*count as f64)*0.5;
            for (col,ch) in text.chars().enumerate() {
                let key=if phone.shift && !phone.symbols {ch.to_uppercase().to_string()}else{ch.to_string()};
                self.key(cx,rect(left+col as f64*unit,r.pos.y+32.0+row as f64*rh,unit,rh),&key,PhoneHit::Key(key.clone()),ios,dark,false);
            }
        }
        self.key(cx,rect(r.pos.x+3.0,r.pos.y+32.0+rh*2.0,unit*1.2,rh),"⇧",PhoneHit::Shift,ios,dark,phone.shift);
        self.key(cx,rect(r.pos.x+r.size.x-unit*1.3,r.pos.y+32.0+rh*2.0,unit*1.2,rh),"⌫",PhoneHit::Key("backspace".into()),ios,dark,false);
        let y=r.pos.y+32.0+rh*3.0;
        self.key(cx,rect(r.pos.x+3.0,y,unit*1.8,rh),if phone.symbols {"ABC"}else{"123"},PhoneHit::Symbols,ios,dark,false);
        self.key(cx,rect(r.pos.x+unit*1.9,y,unit*5.8,rh),"space",PhoneHit::Key(" ".into()),ios,dark,false);
        let action=phone.keyboard_client.and_then(|c|phone.ime.get(&c)).map(|i|match i.return_key {makepad_platform::ime::ReturnKeyType::Search=>"search",makepad_platform::ime::ReturnKeyType::Send=>"send",makepad_platform::ime::ReturnKeyType::Go=>"go",_=>"return"}).unwrap_or(if phone.search_focused {"search"}else{"return"});
        self.key(cx,rect(r.pos.x+unit*7.8,y,unit*2.1,rh),action,PhoneHit::Key("return".into()),ios,dark,true);
    }
    fn key(&mut self, cx: &mut Cx2d, r: Rect, label: &str, hit: PhoneHit, ios: bool, dark: bool, accent: bool) {
        let mut face=if accent {if ios {rgb(0,122,255)}else{rgb(103,80,164)}}else if dark {rgb(75,72,83)}else{rgb(255,255,255)};
        if self.pressed.as_ref()==Some(&hit) {face=if ios {rgb(180,185,196)}else{rgb(190,165,235)};}
        let inside=rect(r.pos.x+3.0,r.pos.y+3.0,(r.size.x-6.0).max(1.0),(r.size.y-8.0).max(1.0));
        self.rounded(cx,rect(inside.pos.x,inside.pos.y+1.0,inside.size.x,inside.size.y),if ios {6.0}else{12.0},alpha(rgb(0,0,0),0.22));
        self.rounded(cx,inside,if ios {6.0}else{12.0},face);
        let ink=if dark || accent {rgb(255,255,255)}else{rgb(22,20,28)};
        // Control keys are icons: mobile text fonts need not contain the
        // desktop keyboard's Unicode shift/delete symbols.
        let icon=match &hit {
            PhoneHit::Shift=>Some(&mut self.key_shift),
            PhoneHit::Key(key) if key=="backspace"=>Some(&mut self.key_backspace),
            _=>None,
        };
        if let Some(icon)=icon {
            let size=inside.size.y.min(22.0);
            icon.color=ink;
            icon.draw_abs(cx,rect(inside.pos.x+(inside.size.x-size)*0.5,inside.pos.y+(inside.size.y-size)*0.5,size,size));
        }else{
            self.label(cx,inside,label,if label.chars().count()>1 {13.0}else{21.0},false,ink);
        }
        self.hits.push((r,hit));
    }
}
impl Widget for PhoneSurface {
    fn draw_walk(&mut self,cx:&mut Cx2d,scope:&mut Scope,walk:Walk)->DrawStep {
        if !self.visible {self.hits.clear();self.area=Area::Empty;return DrawStep::done();}
        let r=cx.walk_turtle_with_area(&mut self.area,walk);
        self.hits.clear();
        if let Some(state)=scope.data.get_mut::<WmState>() {
            if state.style.target.mobile() {
                self.d.solid(cx,r,rgb(25,27,38));
                let left = r.pos.x + self.pad_left.max(8.0);
                let label = state.style.target.label();
                if r.size.x - self.pad_left >= 280.0 {
                    let slot = rect(left+98.0,r.pos.y,72.0,r.size.y);
                    self.label(cx,slot,"Desktop",12.0,false,rgb(201,210,237));
                    self.hits.push((slot,PhoneHit::Desktop));
                }
                for (slot,label,hit) in [
                    (rect(left,r.pos.y,82.0,r.size.y),label,PhoneHit::Style),
                    (rect(r.pos.x+r.size.x-92.0,r.pos.y,44.0,r.size.y),if state.style.dark {"Dark"}else{"Light"},PhoneHit::Appearance),
                    (rect(r.pos.x+r.size.x-46.0,r.pos.y,42.0,r.size.y),"↻",PhoneHit::Rotate),
                ] {self.label(cx,slot,label,12.0,false,rgb(201,210,237));self.hits.push((slot,hit));}
                self.d.icon_centered(cx,Ico::ChevronDown,rect(left+78.0,r.pos.y,14.0,r.size.y),9.0,rgb(201,210,237));
            }
        }
        DrawStep::done()
    }
    fn handle_event(&mut self,_cx:&mut Cx,_event:&Event,_scope:&mut Scope) {}
}
