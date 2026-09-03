//! `shell/Ui/*.qml` — the widget kit every omarchy surface is built from,
//! ported to splash.
//!
//! The QML kit is a set of `BorderSurface`-based controls that all paint
//! themselves out of the same four state tokens (normal / hover-cursor /
//! selected / focus) at the alphas in `Commons/Style.qml`. Here that kit is
//! `ShellDraw`: one registered splash component carrying the shaders, the
//! two type faces and the icon sheet, with one method per QML component
//! (`button`, `toggle_switch`, `panel_slider`, `text_field`, `popup_card`,
//! `panel_section_header`, `panel_separator`, `panel_hero`,
//! `panel_action_button`, `cursor_surface`, `bar_widget`…). Every surface in
//! `shell/` draws through it, so they share one look exactly like the
//! original shares `Ui/`.
//!
//! Two things every port here keeps from the source:
//!  * hard square corners (`Style.cornerRadius` is 0), and
//!  * flat fills plus a 1px border — no bevels, no glows, no gradients
//!    except the hyprland border gradient a theme may name.

use makepad_widgets::*;

use super::{alpha, ControlTokens, CtrlState, ShellTokens, SurfaceTokens};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // A flat fill. Premultiplied so a translucent card composites over the
    // wallpaper the way the QML `Rectangle{color: Util.alpha(...)}` does.
    set_type_default() do #(DrawShellFill::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: #ffffff
        pixel: fn() {
            return vec4(self.color.rgb * self.color.w, self.color.w)
        }
    }

    // `BorderSurface`: a fill plus a hard square ring measured straight off
    // the quad edges. The ring takes two stops and an angle so a theme's
    // hyprland `active-border` gradient (what `[popups] border` resolves to)
    // draws as a gradient, exactly like `BorderOverlay`'s shape path.
    set_type_default() do #(DrawShellChrome::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: #00000000
        border_color: #ffffff
        border_color_end: #ffffff
        border_angle: 0.0
        border_width: 1.0
        pixel: fn() {
            let p = self.pos * self.rect_size
            let d = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
            let cov = clamp((self.border_width - d) * 3.0 + 0.5, 0.0, 1.0)
            let rad = self.border_angle * 0.017453292
            let dir = vec2(cos(rad), sin(rad))
            let half = self.rect_size * 0.5
            let extent = max(abs(dir.x) * half.x + abs(dir.y) * half.y, 0.001)
            let t = clamp(0.5 + dot(p - half, dir) / (2.0 * extent), 0.0, 1.0)
            let bc = mix(self.border_color, self.border_color_end, t)
            let c = mix(self.color, bc, cov * self.border_color.w)
            return vec4(c.rgb * c.w, c.w)
        }
    }

    // The icon sheet: its SVG defaults are the theme; nothing here reads mod.wm_theme.
    set_type_default() do #(ShellIcons::script_component(vm)) {}

    // ------------------------------------------------------------------
    // The kit. `Style.font.family` is "monospace", which on an omarchy box
    // is JetBrains Mono — the variable cut is the one makepad ships, so
    // `bold` is the same face at weight 700 (never a different family).
    // ------------------------------------------------------------------
    set_type_default() do #(ShellDraw::script_component(vm)) {
        fill +: {}
        chrome +: {}
        text +: {
            text_style: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{
                        res: crate_resource("self:../../widgets/resources/jetbrains_mono_variable.ttf")
                        asc: 0.0 desc: 0.0 weight: 400.0
                    }
                    emoji := FontMember{
                        res: crate_resource("self:../../widgets/resources/NotoColorEmoji.ttf")
                        asc: 0.0 desc: 0.0
                    }
                }
                font_size: 9.0
                line_spacing: 1.2
            }
            color: #ffffff
        }
        text_bold +: {
            text_style: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{
                        res: crate_resource("self:../../widgets/resources/jetbrains_mono_variable.ttf")
                        asc: 0.0 desc: 0.0 weight: 700.0
                    }
                    emoji := FontMember{
                        res: crate_resource("self:../../widgets/resources/NotoColorEmoji.ttf")
                        asc: 0.0 desc: 0.0
                    }
                }
                font_size: 9.0
                line_spacing: 1.2
            }
            color: #ffffff
        }
        icons +: {
            menu +: {svg: crate_resource("self:resources/icons/menu.svg")}
            dot +: {svg: crate_resource("self:resources/icons/dot.svg")}
            volume_0 +: {svg: crate_resource("self:resources/icons/volume-0.svg")}
            volume_1 +: {svg: crate_resource("self:resources/icons/volume-1.svg")}
            volume_2 +: {svg: crate_resource("self:resources/icons/volume-2.svg")}
            volume_3 +: {svg: crate_resource("self:resources/icons/volume-3.svg")}
            mic +: {svg: crate_resource("self:resources/icons/mic.svg")}
            mic_off +: {svg: crate_resource("self:resources/icons/mic-off.svg")}
            brightness +: {svg: crate_resource("self:resources/icons/brightness.svg")}
            bluetooth +: {svg: crate_resource("self:resources/icons/bluetooth.svg")}
            bluetooth_off +: {svg: crate_resource("self:resources/icons/bluetooth-off.svg")}
            wifi +: {svg: crate_resource("self:resources/icons/wifi.svg")}
            wifi_off +: {svg: crate_resource("self:resources/icons/wifi-off.svg")}
            monitor +: {svg: crate_resource("self:resources/icons/monitor.svg")}
            battery +: {svg: crate_resource("self:resources/icons/battery.svg")}
            power +: {svg: crate_resource("self:resources/icons/power.svg")}
            calendar +: {svg: crate_resource("self:resources/icons/calendar.svg")}
            keyboard +: {svg: crate_resource("self:resources/icons/keyboard.svg")}
            refresh +: {svg: crate_resource("self:resources/icons/refresh.svg")}
            bell +: {svg: crate_resource("self:resources/icons/bell.svg")}
            bell_off +: {svg: crate_resource("self:resources/icons/bell-off.svg")}
            moon +: {svg: crate_resource("self:resources/icons/moon.svg")}
            record +: {svg: crate_resource("self:resources/icons/record.svg")}
            chevron_left +: {svg: crate_resource("self:resources/icons/chevron-left.svg")}
            chevron_right +: {svg: crate_resource("self:resources/icons/chevron-right.svg")}
            chevron_down +: {svg: crate_resource("self:resources/icons/chevron-down.svg")}
            check +: {svg: crate_resource("self:resources/icons/check.svg")}
            close +: {svg: crate_resource("self:resources/icons/close.svg")}
            search +: {svg: crate_resource("self:resources/icons/search.svg")}
            cpu +: {svg: crate_resource("self:resources/icons/cpu.svg")}
            globe +: {svg: crate_resource("self:resources/icons/globe.svg")}
            play +: {svg: crate_resource("self:resources/icons/play.svg")}
            shirt +: {svg: crate_resource("self:resources/icons/shirt.svg")}
            pulse +: {svg: crate_resource("self:resources/icons/pulse.svg")}
            photo +: {svg: crate_resource("self:resources/icons/photo.svg")}
            window_min +: {svg: crate_resource("self:resources/icons/window-min.svg")}
            window_max +: {svg: crate_resource("self:resources/icons/window-max.svg")}
            window_restore +: {svg: crate_resource("self:resources/icons/window-restore.svg")}
            speaker +: {svg: crate_resource("self:resources/icons/speaker.svg")}
            headphone +: {svg: crate_resource("self:resources/icons/headphone.svg")}
            lock +: {svg: crate_resource("self:resources/icons/lock.svg")}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawShellFill {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawShellChrome {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub border_color_end: Vec4f,
    #[live(0.0)]
    pub border_angle: f32,
    #[live(1.0)]
    pub border_width: f32,
}

/// Our own SVGs on `DrawVector` — omarchy draws Nerd-Font glyphs, we draw
/// vectors (never an SDF icon shader).
#[derive(Script, ScriptHook)]
pub struct ShellIcons {
    #[live]
    pub menu: DrawSvg,
    #[live]
    pub dot: DrawSvg,
    #[live]
    pub volume_0: DrawSvg,
    #[live]
    pub volume_1: DrawSvg,
    #[live]
    pub volume_2: DrawSvg,
    #[live]
    pub volume_3: DrawSvg,
    #[live]
    pub mic: DrawSvg,
    #[live]
    pub mic_off: DrawSvg,
    #[live]
    pub brightness: DrawSvg,
    #[live]
    pub bluetooth: DrawSvg,
    #[live]
    pub bluetooth_off: DrawSvg,
    #[live]
    pub wifi: DrawSvg,
    #[live]
    pub wifi_off: DrawSvg,
    #[live]
    pub monitor: DrawSvg,
    #[live]
    pub battery: DrawSvg,
    #[live]
    pub power: DrawSvg,
    #[live]
    pub calendar: DrawSvg,
    #[live]
    pub keyboard: DrawSvg,
    #[live]
    pub refresh: DrawSvg,
    #[live]
    pub bell: DrawSvg,
    #[live]
    pub bell_off: DrawSvg,
    #[live]
    pub moon: DrawSvg,
    #[live]
    pub record: DrawSvg,
    #[live]
    pub chevron_left: DrawSvg,
    #[live]
    pub chevron_right: DrawSvg,
    #[live]
    pub chevron_down: DrawSvg,
    #[live]
    pub check: DrawSvg,
    #[live]
    pub close: DrawSvg,
    #[live]
    pub search: DrawSvg,
    #[live]
    pub cpu: DrawSvg,
    #[live]
    pub globe: DrawSvg,
    #[live]
    pub play: DrawSvg,
    #[live]
    pub shirt: DrawSvg,
    #[live]
    pub pulse: DrawSvg,
    #[live]
    pub photo: DrawSvg,
    #[live]
    pub window_min: DrawSvg,
    #[live]
    pub window_max: DrawSvg,
    #[live]
    pub window_restore: DrawSvg,
    #[live]
    pub speaker: DrawSvg,
    #[live]
    pub headphone: DrawSvg,
    #[live]
    pub lock: DrawSvg,
}

/// Which glyph a module wants. Named for what it MEANS, so the bar can
/// pick a battery level or a volume step without knowing about files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ico {
    Menu,
    Dot,
    Volume0,
    Volume1,
    Volume2,
    Volume3,
    Mic,
    MicOff,
    Brightness,
    Bluetooth,
    BluetoothOff,
    Wifi,
    WifiOff,
    Monitor,
    Battery,
    Power,
    Calendar,
    Keyboard,
    Refresh,
    Bell,
    BellOff,
    Moon,
    Record,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    Check,
    Close,
    Search,
    Cpu,
    Speaker,
    Headphone,
    Lock,
    Globe,
    Play,
    Shirt,
    Pulse,
    Photo,
    WindowMin,
    WindowMax,
    WindowRestore,
}

impl ShellIcons {
    fn get(&mut self, ico: Ico) -> &mut DrawSvg {
        match ico {
            Ico::Menu => &mut self.menu,
            Ico::Dot => &mut self.dot,
            Ico::Volume0 => &mut self.volume_0,
            Ico::Volume1 => &mut self.volume_1,
            Ico::Volume2 => &mut self.volume_2,
            Ico::Volume3 => &mut self.volume_3,
            Ico::Mic => &mut self.mic,
            Ico::MicOff => &mut self.mic_off,
            Ico::Brightness => &mut self.brightness,
            Ico::Bluetooth => &mut self.bluetooth,
            Ico::BluetoothOff => &mut self.bluetooth_off,
            Ico::Wifi => &mut self.wifi,
            Ico::WifiOff => &mut self.wifi_off,
            Ico::Monitor => &mut self.monitor,
            Ico::Battery => &mut self.battery,
            Ico::Power => &mut self.power,
            Ico::Calendar => &mut self.calendar,
            Ico::Keyboard => &mut self.keyboard,
            Ico::Refresh => &mut self.refresh,
            Ico::Bell => &mut self.bell,
            Ico::BellOff => &mut self.bell_off,
            Ico::Moon => &mut self.moon,
            Ico::Record => &mut self.record,
            Ico::ChevronLeft => &mut self.chevron_left,
            Ico::ChevronRight => &mut self.chevron_right,
            Ico::ChevronDown => &mut self.chevron_down,
            Ico::Check => &mut self.check,
            Ico::Close => &mut self.close,
            Ico::Search => &mut self.search,
            Ico::Cpu => &mut self.cpu,
            Ico::Globe => &mut self.globe,
            Ico::Play => &mut self.play,
            Ico::Shirt => &mut self.shirt,
            Ico::Pulse => &mut self.pulse,
            Ico::Photo => &mut self.photo,
            Ico::WindowMin => &mut self.window_min,
            Ico::WindowMax => &mut self.window_max,
            Ico::WindowRestore => &mut self.window_restore,
            Ico::Speaker => &mut self.speaker,
            Ico::Headphone => &mut self.headphone,
            Ico::Lock => &mut self.lock,
        }
    }
}

/// Horizontal placement of a label inside its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

/// The kit — see the module note. One of these per surface widget.
#[derive(Script, ScriptHook)]
pub struct ShellDraw {
    #[live]
    pub fill: DrawShellFill,
    #[live]
    pub chrome: DrawShellChrome,
    #[live]
    pub text: DrawText,
    #[live]
    pub text_bold: DrawText,
    #[live]
    pub icons: ShellIcons,
}

/// Makepad sizes text in POINTS; the QML scale is in pixels.
pub fn px_to_pt(px: f64) -> f32 {
    (px * 0.75) as f32
}

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        pos: dvec2(x, y),
        size: dvec2(w.max(0.0), h.max(0.0)),
    }
}

/// Shrink a rect on every side (QML `anchors.margins`).
pub fn inset(r: Rect, by: f64) -> Rect {
    rect(
        r.pos.x + by,
        r.pos.y + by,
        (r.size.x - by * 2.0).max(0.0),
        (r.size.y - by * 2.0).max(0.0),
    )
}

pub fn contains(r: Rect, p: Vec2d) -> bool {
    p.x >= r.pos.x && p.x < r.pos.x + r.size.x && p.y >= r.pos.y && p.y < r.pos.y + r.size.y
}

/// Cut `h` off the top of `r`, returning (the strip, the rest).
pub fn cut_top(r: Rect, h: f64) -> (Rect, Rect) {
    let h = h.min(r.size.y);
    (
        rect(r.pos.x, r.pos.y, r.size.x, h),
        rect(r.pos.x, r.pos.y + h, r.size.x, r.size.y - h),
    )
}

impl ShellDraw {
    // ------------------------------------------------------------- text

    fn face(&mut self, bold: bool) -> &mut DrawText {
        if bold {
            &mut self.text_bold
        } else {
            &mut self.text
        }
    }

    /// Width of one line at a px size — QML's `Text.implicitWidth`.
    pub fn measure(&mut self, cx: &mut Cx2d, bold: bool, px: f64, s: &str) -> f64 {
        if s.is_empty() {
            return 0.0;
        }
        let face = self.face(bold);
        face.text_style.font_size = px_to_pt(px);
        face.prepare_single_line_run(cx, s)
            .map(|r| r.width_in_lpxs as f64)
            .unwrap_or(0.0)
    }

    /// `elide: Text.ElideRight`.
    pub fn elide(&mut self, cx: &mut Cx2d, bold: bool, px: f64, s: &str, max_w: f64) -> String {
        if max_w <= 0.0 {
            return String::new();
        }
        if self.measure(cx, bold, px, s) <= max_w {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        let mut lo = 0usize;
        let mut hi = chars.len();
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let candidate: String = chars[..mid].iter().collect::<String>() + "\u{2026}";
            if self.measure(cx, bold, px, &candidate) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
            if lo == hi {
                break;
            }
        }
        chars[..lo].iter().collect::<String>() + "\u{2026}"
    }

    /// `wrapMode: WordWrap` with `elide: ElideRight` on the last line and a
    /// `maximumLineCount` cap — the notification card's summary (2) and
    /// body (3).
    pub fn wrap(
        &mut self,
        cx: &mut Cx2d,
        bold: bool,
        px: f64,
        text: &str,
        max_w: f64,
        max_lines: usize,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split_whitespace() {
                let candidate = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{} {}", line, word)
                };
                if self.measure(cx, bold, px, &candidate) <= max_w || line.is_empty() {
                    line = candidate;
                } else {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_string();
                    if lines.len() == max_lines {
                        break;
                    }
                }
            }
            if !line.is_empty() && lines.len() < max_lines {
                lines.push(line);
            }
            if lines.len() >= max_lines {
                break;
            }
        }
        // Anything that did not fit is elided onto the last line.
        let overflowed = {
            let joined = lines.join(" ");
            joined.split_whitespace().count() < text.split_whitespace().count()
        };
        if overflowed {
            if let Some(last) = lines.last_mut() {
                let s = format!("{}\u{2026}", last);
                *last = s;
            }
        }
        if lines.is_empty() && !text.is_empty() {
            lines.push(self.elide(cx, bold, px, text, max_w));
        }
        lines
    }

    /// One line, placed at an absolute top-left.
    pub fn text_at(
        &mut self,
        cx: &mut Cx2d,
        pos: Vec2d,
        bold: bool,
        px: f64,
        color: Vec4f,
        s: &str,
    ) {
        if s.is_empty() {
            return;
        }
        let face = self.face(bold);
        face.text_style.font_size = px_to_pt(px);
        face.color = color;
        face.draw_abs(cx, pos, s);
    }

    /// One line inside a box: horizontally per `align`, vertically centered
    /// on the ink — what every QML label in the kit does.
    pub fn label(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        bold: bool,
        px: f64,
        color: Vec4f,
        align: HAlign,
        s: &str,
    ) {
        if s.is_empty() {
            return;
        }
        let w = self.measure(cx, bold, px, s);
        let x = match align {
            HAlign::Left => r.pos.x,
            HAlign::Center => r.pos.x + (r.size.x - w) * 0.5,
            HAlign::Right => r.pos.x + r.size.x - w,
        };
        let y = r.pos.y + (r.size.y - px * 1.2) * 0.5;
        self.text_at(cx, dvec2(x.floor(), y.floor()), bold, px, color, s);
    }

    /// As `label`, elided to the box first.
    pub fn label_elided(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        bold: bool,
        px: f64,
        color: Vec4f,
        align: HAlign,
        s: &str,
    ) {
        let s = self.elide(cx, bold, px, s, r.size.x);
        self.label(cx, r, bold, px, color, align, &s);
    }

    // ---------------------------------------------------------- surfaces

    /// A flat fill.
    pub fn solid(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f) {
        if color.w <= 0.0 || r.size.x <= 0.0 || r.size.y <= 0.0 {
            return;
        }
        self.fill.color = color;
        self.fill.draw_abs(cx, r);
    }

    /// `BorderSurface`: fill + ring. `border_end`/`angle` let a theme's
    /// hyprland gradient through.
    pub fn bordered(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        fill: Vec4f,
        border: Vec4f,
        border_end: Vec4f,
        angle: f32,
        width: f64,
    ) {
        if r.size.x <= 0.0 || r.size.y <= 0.0 {
            return;
        }
        self.chrome.color = fill;
        self.chrome.border_color = if width > 0.0 {
            border
        } else {
            alpha(border, 0.0)
        };
        self.chrome.border_color_end = if width > 0.0 {
            border_end
        } else {
            alpha(border_end, 0.0)
        };
        self.chrome.border_angle = angle;
        self.chrome.border_width = width as f32;
        self.chrome.draw_abs(cx, r);
    }

    /// A themed card: `[popups]` / `[menu]` / `[notifications]` chrome.
    pub fn card(&mut self, cx: &mut Cx2d, r: Rect, s: &SurfaceTokens) {
        self.bordered(
            cx,
            r,
            s.bg(),
            s.border_start(),
            s.border_stop(),
            s.border_angle,
            s.border_width,
        );
    }

    /// A control face in one of the shared states (`Style.controlFill` +
    /// `Border.controlSpec`).
    pub fn control(&mut self, cx: &mut Cx2d, r: Rect, c: &ControlTokens, state: CtrlState) {
        let border = c.border(state);
        self.bordered(
            cx,
            r,
            c.fill(state),
            border,
            border,
            0.0,
            c.border_width(state),
        );
    }

    /// `CursorSurface`: nothing at rest, the hover fill under the cursor,
    /// the selected fill for the current row.
    pub fn cursor_surface(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        c: &ControlTokens,
        has_cursor: bool,
        current: bool,
    ) {
        let fill = c.cursor_fill(has_cursor, current);
        if fill.w > 0.0 {
            self.solid(cx, r, fill);
        }
    }

    /// `PanelSeparator` — a 1px rule at `foreground` × strength (0.12).
    pub fn separator(&mut self, cx: &mut Cx2d, r: Rect, foreground: Vec4f, strength: f32) {
        self.solid(
            cx,
            rect(r.pos.x, r.pos.y, r.size.x, 1.0),
            alpha(foreground, strength),
        );
    }

    // ------------------------------------------------------------- icons

    /// One SVG, fitted into `r` and tinted.
    pub fn icon(&mut self, cx: &mut Cx2d, ico: Ico, r: Rect, color: Vec4f) {
        if r.size.x <= 0.0 || r.size.y <= 0.0 || color.w <= 0.0 {
            return;
        }
        let svg = self.icons.get(ico);
        svg.color = color;
        svg.draw_abs(cx, r);
    }

    /// An icon centered in a slot at `size` px square (the bar's
    /// `iconCanvas` inside its `iconSlot`).
    pub fn icon_centered(&mut self, cx: &mut Cx2d, ico: Ico, slot: Rect, size: f64, color: Vec4f) {
        let r = rect(
            (slot.pos.x + (slot.size.x - size) * 0.5).floor(),
            (slot.pos.y + (slot.size.y - size) * 0.5).floor(),
            size,
            size,
        );
        self.icon(cx, ico, r, color);
    }

    // --------------------------------------------------------- controls

    /// `Ui/Button.qml`: `[icon] [label]` centered, `controlPaddingX/Y`
    /// padding, `controlGap` between them. Returns the rect it drew into.
    #[allow(clippy::too_many_arguments)]
    pub fn button(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        tok: &ShellTokens,
        state: CtrlState,
        icon: Option<Ico>,
        text: &str,
        px: f64,
        foreground: Vec4f,
        bordered: bool,
    ) {
        let c = &tok.controls;
        if bordered || !matches!(state, CtrlState::Normal) {
            self.control(cx, r, c, state);
        } else if state == CtrlState::Selected {
            self.solid(cx, r, c.fill(CtrlState::Selected));
        }
        let fg = if state == CtrlState::Disabled {
            super::darker(foreground, 2.0)
        } else {
            foreground
        };
        let gap = tok.spacing.control_gap;
        let icon_w = if icon.is_some() { tok.font.icon } else { 0.0 };
        let text_w = self.measure(cx, state == CtrlState::Selected, px, text);
        let total = icon_w + if icon.is_some() && !text.is_empty() { gap } else { 0.0 } + text_w;
        let mut x = r.pos.x + (r.size.x - total) * 0.5;
        if let Some(ico) = icon {
            self.icon_centered(
                cx,
                ico,
                rect(x, r.pos.y, icon_w, r.size.y),
                icon_w,
                fg,
            );
            x += icon_w + if text.is_empty() { 0.0 } else { gap };
        }
        if !text.is_empty() {
            self.label(
                cx,
                rect(x, r.pos.y, text_w, r.size.y),
                state == CtrlState::Selected,
                px,
                fg,
                HAlign::Left,
                text,
            );
        }
    }

    /// The implicit width of a `Button` — label + `controlPaddingX * 2`.
    pub fn button_width(
        &mut self,
        cx: &mut Cx2d,
        tok: &ShellTokens,
        icon: bool,
        text: &str,
        px: f64,
    ) -> f64 {
        let text_w = self.measure(cx, false, px, text);
        let icon_w = if icon { tok.font.icon } else { 0.0 };
        let gap = if icon && !text.is_empty() {
            tok.spacing.control_gap
        } else {
            0.0
        };
        text_w + icon_w + gap + tok.spacing.control_padding_x * 2.0
    }

    /// `Ui/ToggleSwitch.qml`: a track `max(22, round(controlHeight*0.55))`
    /// high, `1.9x` as wide, with a `0.72x` knob inset by the remainder.
    pub fn toggle_switch(
        &mut self,
        cx: &mut Cx2d,
        at: Vec2d,
        tok: &ShellTokens,
        checked: bool,
        foreground: Vec4f,
    ) -> Rect {
        let c = &tok.controls;
        let track_h = (tok.spacing.control_height * 0.55).round().max(22.0);
        let track_w = (track_h * 1.9).round();
        let knob = (track_h * 0.72).round().max(6.0);
        let inset_px = ((track_h - knob) / 2.0).round().max(1.0);
        let track = rect(at.x, at.y, track_w, track_h);
        let state = if checked {
            CtrlState::Selected
        } else {
            CtrlState::Normal
        };
        self.control(cx, track, c, state);
        let knob_x = if checked {
            track.pos.x + track.size.x - knob - inset_px
        } else {
            track.pos.x + inset_px
        };
        let knob_color = if checked {
            c.selected_color
        } else {
            super::darker(foreground, 1.25)
        };
        self.solid(
            cx,
            rect(knob_x, track.pos.y + inset_px, knob, knob),
            knob_color,
        );
        track
    }

    /// `Ui/PanelSlider.qml`: a `max(4, round(controlHeight*0.11))` track
    /// with a `max(14, round(controlHeight*0.38))` knob ringed in the panel
    /// background — the one place the kit uses a flat ring instead of a
    /// state token.
    #[allow(clippy::too_many_arguments)]
    pub fn panel_slider(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        tok: &ShellTokens,
        progress: f64,
        foreground: Vec4f,
        background: Vec4f,
        hot: bool,
    ) {
        let c = &tok.controls;
        let track_h = (tok.spacing.control_height * 0.11).round().max(4.0);
        let knob = (tok.spacing.control_height * 0.38).round().max(14.0);
        let track = rect(
            r.pos.x,
            r.pos.y + (r.size.y - track_h) * 0.5,
            r.size.x,
            track_h,
        );
        self.solid(cx, track, alpha(foreground, c.selected_fill_alpha));
        let p = progress.clamp(0.0, 1.0);
        self.solid(
            cx,
            rect(track.pos.x, track.pos.y, track.size.x * p, track.size.y),
            foreground,
        );
        let scale = if hot { 1.15 } else { 1.0 };
        let ks = (knob * scale).round();
        let kx = (track.pos.x + track.size.x * p - ks * 0.5)
            .clamp(track.pos.x, track.pos.x + track.size.x - ks);
        let ky = r.pos.y + (r.size.y - ks) * 0.5;
        self.bordered(
            cx,
            rect(kx, ky, ks, ks),
            foreground,
            background,
            background,
            0.0,
            2.0,
        );
    }

    /// `Ui/TextField.qml`: the control face plus the text (or the
    /// placeholder at `darker(fg, 1.6)`) inset by `controlPaddingX` /
    /// `inputPaddingY`, with a 1px caret when focused.
    #[allow(clippy::too_many_arguments)]
    pub fn text_field(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        tok: &ShellTokens,
        text: &str,
        placeholder: &str,
        focused: bool,
        hot: bool,
        foreground: Vec4f,
    ) {
        let state = if focused {
            CtrlState::Focus
        } else if hot {
            CtrlState::Hover
        } else {
            CtrlState::Normal
        };
        self.control(cx, r, &tok.controls, state);
        let inner = rect(
            r.pos.x + tok.spacing.control_padding_x,
            r.pos.y,
            (r.size.x - tok.spacing.control_padding_x * 2.0).max(0.0),
            r.size.y,
        );
        let px = tok.font.body;
        if text.is_empty() {
            self.label_elided(
                cx,
                inner,
                false,
                px,
                super::darker(foreground, 1.6),
                HAlign::Left,
                placeholder,
            );
        } else {
            self.label_elided(cx, inner, false, px, foreground, HAlign::Left, text);
            if focused {
                let w = self.measure(cx, false, px, text).min(inner.size.x);
                self.solid(
                    cx,
                    rect(
                        inner.pos.x + w + 1.0,
                        inner.pos.y + (inner.size.y - px * 1.1) * 0.5,
                        1.0,
                        px * 1.1,
                    ),
                    foreground,
                );
            }
        }
    }

    /// `Ui/PanelSectionHeader.qml`: bold caption in `darker(fg, 1.4)`.
    pub fn section_header(&mut self, cx: &mut Cx2d, r: Rect, tok: &ShellTokens, fg: Vec4f, s: &str) {
        self.label(
            cx,
            r,
            true,
            tok.font.caption,
            super::darker(fg, 1.4),
            HAlign::Left,
            s,
        );
    }

    /// `Ui/PanelHero.qml`: the icon, a bold `title` line and an uppercase
    /// `caption` meta line at `darker(fg, 1.4)`, with an optional trailing
    /// control kept clear on the right.
    #[allow(clippy::too_many_arguments)]
    pub fn panel_hero(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        tok: &ShellTokens,
        fg: Vec4f,
        icon: Ico,
        title: &str,
        meta: &str,
        trailing_inset: f64,
    ) {
        let icon_size = tok.font.display;
        self.icon_centered(
            cx,
            icon,
            rect(r.pos.x, r.pos.y, icon_size, r.size.y),
            icon_size,
            fg,
        );
        let x = r.pos.x + icon_size + 14.0;
        let w = (r.size.x - (x - r.pos.x) - trailing_inset).max(0.0);
        let dim = super::darker(fg, 1.4);
        let title_h = tok.font.title * 1.4;
        let meta_h = tok.font.caption * 1.4;
        let total = title_h + 2.0 + meta_h;
        let top = r.pos.y + (r.size.y - total) * 0.5;
        self.label_elided(
            cx,
            rect(x, top, w, title_h),
            true,
            tok.font.title,
            fg,
            HAlign::Left,
            title,
        );
        self.label_elided(
            cx,
            rect(x, top + title_h + 2.0, w, meta_h),
            true,
            tok.font.caption,
            dim,
            HAlign::Left,
            &meta.to_uppercase(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_are_three_quarters_of_pixels() {
        // The QML scale is px; makepad's font_size is pt.
        assert_eq!(px_to_pt(12.0), 9.0);
        assert_eq!(px_to_pt(28.0), 21.0);
    }

    #[test]
    fn rect_helpers_never_invert() {
        let r = rect(10.0, 10.0, 20.0, 20.0);
        assert_eq!(inset(r, 30.0).size.x, 0.0);
        let (top, rest) = cut_top(r, 5.0);
        assert_eq!(top.size.y, 5.0);
        assert_eq!(rest.pos.y, 15.0);
        assert_eq!(rest.size.y, 15.0);
        assert!(contains(r, dvec2(10.0, 10.0)));
        assert!(!contains(r, dvec2(30.0, 10.0)));
    }
}
