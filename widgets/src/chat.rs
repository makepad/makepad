//! ChatBubble and ChatList — one message, and a column of them.
//!
//! A message is not a label in a box. It has a side (yours or the other
//! party's), which decides both where it sits and what colour it is; a face
//! and a name; the time it was said; and, for the ones you sent, a mark for
//! how far it got. While an answer is still arriving it has to say so, and
//! the moment it stops saying so is the moment the answer is complete.
//!
//! **The part that is arithmetic is the grouping.** Consecutive messages
//! from the same sender are one run: only the first of a run carries the
//! avatar and the name, because repeating them down five lines is noise, and
//! the corner radii change so the run reads as a single block rather than
//! five separate slabs. What breaks a run is not just a different sender —
//! it is also a long enough silence, and a change of day. That rule is
//! [`plan`], a free function over [`ChatEntry`] with its own tests, and the
//! widget is only its drawing.
//!
//! **What this deliberately does NOT do.**
//!
//! It owns no calendar. The timestamp and the day label are strings the host
//! hands over already formatted, because a UI library that decides what
//! "yesterday" means in the reader's timezone is a library that gets it
//! wrong somewhere. The grouping reads seconds, and only ever differences
//! between them.
//!
//! It holds no history. `ChatList` draws every message it is given, so it
//! suits a conversation that fits in a screenful or three; a transcript of
//! ten thousand lines belongs in a `PortalList` drawing `ChatBubble`s, which
//! is why the bubble is usable entirely on its own.
//!
//! It carries no composer, no attachments, no reactions and no rich text.
//! The body is one run of plain text that wraps; a message that needs
//! headings and links is a `Markdown` in a bubble-shaped container, not a
//! bigger bubble.

use crate::{badge::measure, makepad_derive_widget::*, makepad_draw::*, widget::*, widget_tree::CxWidgetExt};
use std::collections::HashMap;

/// Where one message sits in a run of messages from the same sender.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ChatGroup {
    /// On its own: nothing above it and nothing below it belongs with it.
    #[pick]
    Single = 0,
    /// The first of a run, and so the one that carries the face and the name.
    First = 1,
    /// Inside a run, with a message from the same sender either side.
    Middle = 2,
    /// The last of a run.
    Last = 3,
}

impl ChatGroup {
    /// The name a test reads back.
    pub fn name(self) -> &'static str {
        match self {
            ChatGroup::Single => "single",
            ChatGroup::First => "first",
            ChatGroup::Middle => "middle",
            ChatGroup::Last => "last",
        }
    }

    /// Whether a message from the same sender sits directly above this one.
    pub fn has_prev(self) -> bool {
        matches!(self, ChatGroup::Middle | ChatGroup::Last)
    }

    /// Whether one sits directly below it.
    pub fn has_next(self) -> bool {
        matches!(self, ChatGroup::First | ChatGroup::Middle)
    }

    /// Whether this message carries the avatar and the name.
    ///
    /// Only the head of a run does. Five bubbles from one person each
    /// repeating the same face and the same name say the name five times
    /// and say who is speaking no better than once would.
    pub fn heads_run(self) -> bool {
        !self.has_prev()
    }

    /// The four corner radii, in the order `box_all` takes them: left-top,
    /// right-top, right-bottom, left-bottom.
    ///
    /// A run stands on ONE edge — the right for your own messages, the left
    /// for the other party's — and it is the corners on that edge that
    /// tighten, so the run reads as one block with a shaped outer profile.
    /// Tightening all four would just make every bubble in a run squarer,
    /// which says nothing about what belongs with what.
    pub fn radii(self, own: bool, round: f64, tight: f64) -> [f64; 4] {
        let top = if self.has_prev() { tight } else { round };
        let bottom = if self.has_next() { tight } else { round };
        if own {
            [round, top, bottom, round]
        } else {
            [top, round, round, bottom]
        }
    }
}

/// How far a message got. Only your own messages normally carry one.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ChatDelivery {
    /// No mark at all: what the other party's messages get.
    #[pick]
    Unmarked = 0,
    /// Handed to the network, not yet acknowledged.
    Sending = 1,
    /// The server has it.
    Sent = 2,
    /// The other device has it.
    Delivered = 3,
    /// A person has seen it.
    Read = 4,
    /// It did not go.
    Failed = 5,
}

impl ChatDelivery {
    /// The name a test waits on, and the name the markup accepts.
    pub fn name(self) -> &'static str {
        match self {
            ChatDelivery::Unmarked => "unmarked",
            ChatDelivery::Sending => "sending",
            ChatDelivery::Sent => "sent",
            ChatDelivery::Delivered => "delivered",
            ChatDelivery::Read => "read",
            ChatDelivery::Failed => "failed",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        [
            ChatDelivery::Unmarked,
            ChatDelivery::Sending,
            ChatDelivery::Sent,
            ChatDelivery::Delivered,
            ChatDelivery::Read,
            ChatDelivery::Failed,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

/// One message reduced to the three things the grouping reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatEntry<'a> {
    /// Who said it. Only equality is read, so any stable number will do.
    pub sender: u64,
    /// When, in seconds. Only differences between neighbours are read, so
    /// the origin does not matter as long as one list uses one clock.
    pub at: f64,
    /// The day this message belongs to, as the host names it. Compared, and
    /// never parsed: see the module note about owning no calendar.
    pub day: &'a str,
}

/// What a list draws around one message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatSlot {
    /// A day separator goes above this message.
    pub new_day: bool,
    /// Where the message sits in its run.
    pub group: ChatGroup,
}

/// Where every message sits in its run, and which of them a day separator
/// goes above.
///
/// Two neighbours are in the same run when the same sender sent them, on the
/// same day, within `gap` seconds of each other. All three conditions matter:
/// a different sender obviously breaks a run, but so does an hour of silence
/// — two messages an hour apart are two thoughts, not one — and so does a
/// change of day, because a run that reads as one block cannot have a
/// separator drawn through the middle of it.
///
/// A negative delta breaks a run too. Messages handed over out of order are
/// not a run in any order the reader can see, and pretending otherwise would
/// tighten corners against a neighbour that is not there.
///
/// The first message always gets a separator: a list that starts mid-day
/// without saying which day starts nowhere.
pub fn plan(entries: &[ChatEntry], gap: f64) -> Vec<ChatSlot> {
    // Whether each message continues the one above it. Worked out first for
    // the whole list, because a message's own position needs the answer for
    // the message BELOW it as well, and that is not known while walking.
    let joins: Vec<bool> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            if i == 0 {
                return false;
            }
            let prev = &entries[i - 1];
            let delta = entry.at - prev.at;
            prev.sender == entry.sender && prev.day == entry.day && delta >= 0.0 && delta <= gap
        })
        .collect();

    (0..entries.len())
        .map(|i| ChatSlot {
            new_day: i == 0 || entries[i].day != entries[i - 1].day,
            group: match (joins[i], joins.get(i + 1).copied().unwrap_or(false)) {
                (false, false) => ChatGroup::Single,
                (false, true) => ChatGroup::First,
                (true, true) => ChatGroup::Middle,
                (true, false) => ChatGroup::Last,
            },
        })
        .collect()
}

/// One message, as a host hands it over.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    /// Stable across refreshes: it names the bubble in the widget tree, so a
    /// message that keeps its id keeps its widget.
    pub id: LiveId,
    /// Who said it; see [`ChatEntry::sender`].
    pub sender: u64,
    /// Shown on the head of a run only.
    pub name: String,
    /// The letters on the avatar disc when no picture is supplied.
    pub initials: String,
    /// The time, already formatted.
    pub time: String,
    /// The day, already formatted; the separator's text.
    pub day: String,
    /// When, in seconds; see [`ChatEntry::at`].
    pub at: f64,
    pub text: String,
    /// Your side of the conversation, or the other party's.
    pub own: bool,
    pub delivery: ChatDelivery,
    /// The text is still arriving.
    pub streaming: bool,
}

impl Default for ChatMessage {
    fn default() -> Self {
        Self {
            id: LiveId(0),
            sender: 0,
            name: String::new(),
            initials: String::new(),
            time: String::new(),
            day: String::new(),
            at: 0.0,
            text: String::new(),
            own: false,
            delivery: ChatDelivery::Unmarked,
            streaming: false,
        }
    }
}

impl ChatMessage {
    /// What the grouping reads.
    pub fn entry(&self) -> ChatEntry<'_> {
        ChatEntry {
            sender: self.sender,
            at: self.at,
            day: &self.day,
        }
    }
}

/// A sender number for a name, so markup that has only names still groups.
///
/// FNV-1a, because it has to be the same number every run and for every list
/// that spells the name the same way; nothing here depends on it being hard
/// to collide.
pub fn sender_of(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The letters for a face with no picture: the first of each of the first
/// two words, in capitals.
pub fn bubble_initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// `hh:mm` as seconds since midnight, or nothing when it is not a clock.
///
/// Twenty-four hour only. This exists so a list written in markup groups by
/// the times it already shows, rather than needing a second column of
/// seconds that says the same thing.
pub fn clock_seconds(text: &str) -> Option<f64> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let hours: f64 = hours.trim().parse().ok()?;
    let minutes: f64 = minutes.trim().parse().ok()?;
    if !(0.0..24.0).contains(&hours) || !(0.0..60.0).contains(&minutes) {
        return None;
    }
    Some(hours * 3600.0 + minutes * 60.0)
}

/// One line of the `lines` markup as a message.
///
/// The shape is `side|name|time|day|text`, where `side` is `own` or anything
/// else for the other party, optionally followed by `:` and a comma-separated
/// list of flags: a delivery name, and `streaming`. The text keeps any
/// further bars, so a message may contain one.
///
/// `at` comes from the clock in the `time` column when there is one, and
/// otherwise from the line's position — so a list either dates every line or
/// none of them, and a list that dates none still groups neighbours.
///
/// This is for markup and for the catalogue. [`ChatListRef::set_messages`] is
/// the real way in.
pub fn parse_line(index: usize, line: &str) -> Option<ChatMessage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut parts = line.splitn(5, '|');
    let side = parts.next()?.trim();
    let name = parts.next().unwrap_or("").trim();
    let time = parts.next().unwrap_or("").trim();
    let day = parts.next().unwrap_or("").trim();
    let text = parts.next().unwrap_or("").trim();

    let (side, flags) = side.split_once(':').unwrap_or((side, ""));
    let mut delivery = ChatDelivery::Unmarked;
    let mut streaming = false;
    for flag in flags.split(',').map(str::trim).filter(|f| !f.is_empty()) {
        if flag.eq_ignore_ascii_case("streaming") {
            streaming = true;
        } else if let Some(kind) = ChatDelivery::from_name(flag) {
            delivery = kind;
        }
    }

    Some(ChatMessage {
        id: LiveId(index as u64 + 1),
        sender: sender_of(name),
        name: name.to_string(),
        initials: bubble_initials(name),
        time: time.to_string(),
        day: day.to_string(),
        at: clock_seconds(time).unwrap_or(index as f64),
        text: text.to_string(),
        own: side.trim().eq_ignore_ascii_case("own"),
        delivery,
        streaming,
    })
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Both enums stay QUALIFIED rather than splatted into mod.widgets:
    // `Single`, `First`, `Read` and `Failed` are words other widgets will
    // want for themselves, and a splat here would take them. A call site
    // writes `group: ChatGroup.First`.
    let ChatGroup = set_type_default() do #(ChatGroup::script_api(vm))
    mod.widgets.ChatGroup = ChatGroup
    let ChatDelivery = set_type_default() do #(ChatDelivery::script_api(vm))
    mod.widgets.ChatDelivery = ChatDelivery

    use mod.widgets.*

    mod.widgets.DrawChatBoxBase = #(DrawChatBox::script_component(vm))
    set_type_default() do #(DrawChatBox::script_shader(vm)){
        ..mod.draw.DrawQuad

        // One filled box with four independent corner radii, used three
        // ways: the bubble, the avatar disc (all four radii at half the
        // size), and the hairline either side of a day. Every value on it
        // is a plain field on the Rust struct, set per draw call, because
        // a bubble's colour and corners change with the message and a
        // shader recompile per message would be absurd.
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            // The box fills the whole quad rather than sitting half a pixel
            // inside it: the day hairline is one point tall, and half a point
            // off each edge leaves nothing at all to paint.
            //
            // Halved radii: box_all's visual radius is twice the number
            // handed to it, the same as sdf.box's.
            sdf.box_all(
                0.0,
                0.0,
                self.rect_size.x,
                self.rect_size.y,
                self.r_lt * 0.5,
                self.r_rt * 0.5,
                self.r_rb * 0.5,
                self.r_lb * 0.5
            )
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.DrawChatMarkBase = #(DrawChatMark::script_component(vm))
    set_type_default() do #(DrawChatMark::script_shader(vm)){
        ..mod.draw.DrawQuad

        /** The delivery mark: a ring, one tick, two ticks, or a warned ring.
         *
         * Every arm is rectangles and circles. A tick drawn as an SDF PATH
         * does not paint reliably here — two mirrored identical paths, and
         * only the second ever appeared — so the tick is two rectangles in
         * a frame rotated a quarter turn back, which is analytic and always
         * paints. */
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let h = self.rect_size.y
            let mid_x = self.rect_size.x * 0.5
            let mid_y = self.rect_size.y * 0.5
            let t = max(1.0, h * 0.13)
            let long = h * 0.50
            let short = h * 0.25
            // How far the second tick trails the first, along the long arm.
            let trail = long * 0.55
            // The tick's bounding box once rotated: both arms lie at 45
            // degrees, so each contributes its length times cos 45.
            let diag = 0.70710678
            let bw = (long + short) * diag
            let bh = long * diag

            // Read as floats rather than compared as enums, so every test
            // below is a float test and no arm depends on enum comparison
            // being available in the shader language.
            let ticks = match self.mark {
                ChatDelivery.Sent => 1.0
                ChatDelivery.Delivered => 2.0
                ChatDelivery.Read => 2.0
                _ => 0.0
            }
            let ring = match self.mark {
                ChatDelivery.Sending => 1.0
                ChatDelivery.Failed => 1.0
                _ => 0.0
            }
            let warn = match self.mark {
                ChatDelivery.Failed => 1.0
                _ => 0.0
            }

            if ticks > 0.0 {
                let span = bw + (ticks - 1.0) * trail * diag
                // The vertex of the first tick: the point both arms meet at.
                let vx = mid_x - span * 0.5 + short * diag
                let vy = mid_y + bh * 0.5
                // Rotating the frame back a quarter turn puts the long arm
                // on the frame's +x and the short arm on its -y, so both are
                // axis-aligned rectangles from the vertex.
                sdf.rotate(-0.78539816, vx, vy)
                sdf.rect(vx, vy - t * 0.5, long, t)
                sdf.rect(vx - t * 0.5, vy - short, t, short)
                if ticks > 1.5 {
                    sdf.rect(vx + trail, vy - t * 0.5, long, t)
                    sdf.rect(vx + trail - t * 0.5, vy - short, t, short)
                }
                sdf.fill(self.color)
                sdf.rotate(0.78539816, vx, vy)
            }
            if ring > 0.0 {
                sdf.circle(mid_x, mid_y, h * 0.32)
                sdf.stroke(self.color, t)
            }
            if warn > 0.0 {
                sdf.rect(mid_x - t * 0.5, mid_y - h * 0.17, t, h * 0.20)
                sdf.rect(mid_x - t * 0.5, mid_y + h * 0.09, t, t)
                sdf.fill(self.color)
            }
            return sdf.result
        }
    }

    mod.widgets.DrawChatPulseBase = #(DrawChatPulse::script_component(vm))
    set_type_default() do #(DrawChatPulse::script_shader(vm)){
        ..mod.draw.DrawQuad

        /** The mark that says the text is still arriving: three dots while
         * nothing has come yet, a blinking caret once something has.
         *
         * This is the only shader here that reads the pass clock, which pins
         * the window at display rate, so the widget draws it ONLY while a
         * message is actually streaming. A resting conversation costs
         * nothing. */
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let mid_x = self.rect_size.x * 0.5
            let mid_y = self.rect_size.y * 0.5
            if self.dots > 0.5 {
                let r = max(1.5, self.rect_size.y * 0.12)
                let pitch = r * 3.2
                let turn = 6.2831853
                // A third of a turn apart, so the three read as one wave
                // travelling rather than three lamps flashing together.
                sdf.circle(mid_x - pitch, mid_y, r)
                sdf.fill(self.color * (0.30 + 0.70 * (0.5 + 0.5 * sin(self.draw_pass.time * turn - 2.1))))
                sdf.circle(mid_x, mid_y, r)
                sdf.fill(self.color * (0.30 + 0.70 * (0.5 + 0.5 * sin(self.draw_pass.time * turn - 1.05))))
                sdf.circle(mid_x + pitch, mid_y, r)
                sdf.fill(self.color * (0.30 + 0.70 * (0.5 + 0.5 * sin(self.draw_pass.time * turn))))
            }
            if self.dots < 0.5 {
                let phase = fract(self.draw_pass.time * 1.4)
                // The caret fills its rect across: the widget hands it the
                // bar's own width, so the bar's thickness is one decision in
                // one place rather than a fraction guessed at both ends.
                sdf.rect(0.0, self.rect_size.y * 0.15, self.rect_size.x, self.rect_size.y * 0.7)
                // On slightly longer than off: a caret that spends half its
                // life invisible reads as a fault rather than as a cursor.
                sdf.fill(self.color * (0.10 + 0.90 * step(phase, 0.6)))
            }
            return sdf.result
        }
    }

    mod.widgets.ChatBubbleBase = #(ChatBubble::register_widget(vm))

    /** One message: which side it is on, who said it, when, how far it got,
     * and where it sits in a run of messages from the same sender. */
    mod.widgets.ChatBubble = set_type_default() do mod.widgets.ChatBubbleBase{
        width: Fill
        height: Fit

        /** what was said */
        text: ""
        /** your side of the conversation, or the other party's 0..1 step 1 */
        own: false
        /** who said it; drawn on the head of a run only */
        name: ""
        /** the letters on the disc when no picture is supplied */
        initials: ""
        /** when it was said, already formatted by the host */
        time: ""
        /** where it sits in its run: ChatGroup.Single First Middle Last */
        group: ChatGroup.Single
        /** how far it got: ChatDelivery.Unmarked Sending Sent Delivered Read Failed */
        delivery: ChatDelivery.Unmarked
        /** the text is still arriving 0..1 step 1 */
        streaming: false

        /** the widest the bubble may get; 0 lets it take the room 0..800 step 10 */
        max_width: 420.
        /** room kept clear on the far side, so a bubble never spans the row 0..240 step 4 */
        min_gutter: 56.
        /** the face's size; 0 takes the whole column away 0..64 step 2 */
        avatar_size: 28.
        /** the gap between the face and the bubble 0..24 step 1 */
        avatar_gap: 8.
        /** padding inside the bubble, across 2..24 step 1 */
        pad_x: 10.
        /** padding inside the bubble, down 2..24 step 1 */
        pad_y: 7.
        /** the gap between the name, the message and the meta line 0..12 step 1 */
        line_gap: 3.
        /** the radius of a corner nothing joins on 0..24 step 0.5 */
        radius: theme.radius_l
        /** the radius of the corners a run joins on 0..12 step 0.5 */
        radius_tight: 4.
        /** how much of the meta ink the time and the mark keep 0..1 step 0.05 */
        meta_opacity: 0.75

        /** the fill of your own messages */
        color_own: theme.color_primary
        /** the message ink on that fill */
        ink_own: theme.color_on_primary
        /** the name, time and mark ink on that fill */
        meta_own: theme.color_on_primary
        /** the fill of the other party's messages */
        color_other: theme.color_surface_container_high
        /** the message ink on that fill */
        ink_other: theme.color_text
        /** the name, time and mark ink on that fill */
        meta_other: theme.color_text_meta
        /** the face's disc when no picture is supplied */
        color_avatar: theme.color_surface_container_highest
        /** the letters on it */
        ink_avatar: theme.color_text_meta
        /** the read mark, the one mark that is not the meta ink */
        color_read: theme.color_info
        /** the failed mark */
        color_failed: theme.color_error

        // A slot, drawn over the disc: `avatar: Image{...}`. An
        // `avatar :=` would make a named child instead and leave the slot
        // empty, and the face would be a blank disc for ever.
        avatar: View{}

        draw_text +: {
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.3}
        }
        // The name and the time are given literal sizes because the type
        // ladder has no rung below the paragraph size, and a name set at the
        // paragraph size competes with the message underneath it.
        draw_name +: {
            text_style: theme.font_bold{font_size: 9.5 line_spacing: 1.0}
        }
        draw_meta +: {
            text_style: theme.font_regular{font_size: 9.0 line_spacing: 1.0}
        }
    }

    mod.widgets.ChatListBase = #(ChatList::register_widget(vm))

    /** A column of messages: runs from one sender grouped into blocks, and a
     * day separator wherever the day changes. */
    mod.widgets.ChatList = set_type_default() do mod.widgets.ChatListBase{
        width: Fill
        height: Fit
        flow: Down
        /** the gap between two messages inside one run 0..16 step 1 */
        spacing: 2.

        /** messages for markup, one per line: `side|name|time|day|text` */
        lines: []
        /** how long a silence breaks a run, in seconds 0..3600 step 30 */
        run_gap: 300.
        /** the extra gap above a message that starts a run 0..40 step 1 */
        spacing_run: 12.
        /** the height of a day separator row 0..64 step 2 */
        day_height: 30.
        /** the hairline either side of the day; 0 leaves the day alone 0..4 step 0.5 */
        rule_size: 1.
        /** the room between the day and its hairlines 0..24 step 1 */
        day_gap: 10.
        /** the hairline's colour */
        color_rule: theme.color_outline_variant

        draw_day +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: 9.0 line_spacing: 1.0}
        }

        // A named entry, the way a list declares its row template: it lands
        // in the template table and is instanced per message rather than
        // drawn as a child.
        bubble := mod.widgets.ChatBubble{
            width: Fill
            height: Fit
        }
    }
}

/// A filled box with four independent corner radii. Every value is set by
/// the widget per draw call, so a message's colour and corners cost nothing
/// but instance data.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawChatBox {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    r_lt: f32,
    #[live]
    r_rt: f32,
    #[live]
    r_rb: f32,
    #[live]
    r_lb: f32,
}

/// The delivery mark.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawChatMark {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    mark: ChatDelivery,
    #[live]
    color: Vec4f,
}

/// The still-arriving mark: three dots, or a caret.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawChatPulse {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    /// 1 for the three dots, 0 for the caret.
    #[live]
    dots: f32,
}

/// What a drawn run overhangs its measured advance by: the last glyph's side
/// bearing and the anti-alias pad. Without it a bubble sized to the
/// measurement crowds its final letter against the padding.
const TEXT_SLACK: f64 = 2.0;

/// The size `text` takes in this style, in layout points.
fn text_size(cx: &mut Cx2d, draw_text: &DrawText, text: &str) -> DVec2 {
    if text.is_empty() {
        return dvec2(0.0, 0.0);
    }
    let laid = draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    let scale = draw_text.font_scale as f64;
    dvec2(
        laid.size_in_lpxs.width as f64 * scale,
        laid.size_in_lpxs.height as f64 * scale,
    )
}

/// Draw one line at an absolute position, measured first so the box it is
/// placed in is the box it fills.
///
/// Measured and walked rather than `draw_abs`ed: `draw_abs` takes the top of
/// the LINE box, which sits about a third of the font size above the ink, so
/// anything centred against it rides high.
fn draw_text_at(cx: &mut Cx2d, draw_text: &mut DrawText, pos: DVec2, text: &str) {
    if text.is_empty() {
        return;
    }
    let laid = draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    let scale = draw_text.font_scale as f64;
    let size = dvec2(
        laid.size_in_lpxs.width as f64 * scale,
        laid.size_in_lpxs.height as f64 * scale,
    );
    draw_text.draw_walk_laidout(cx, Walk::fixed(size.x, size.y).with_abs_pos(pos), &laid);
}

fn faded(color: Vec4f, opacity: f64) -> Vec4f {
    Vec4f {
        w: color.w * opacity as f32,
        ..color
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ChatBubble {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    /// The bubble itself. Also the widget's redraw handle: every part of a
    /// message is placed absolutely, so this is the one drawn thing whose
    /// area is worth anything.
    #[redraw]
    #[live]
    draw_bg: DrawChatBox,
    /// The face's disc, under whatever the avatar slot holds.
    #[live]
    draw_avatar: DrawChatBox,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_name: DrawText,
    #[live]
    pub draw_meta: DrawText,
    #[live]
    draw_mark: DrawChatMark,
    #[live]
    draw_pulse: DrawChatPulse,

    /// A picture for the face, drawn over the disc. Anything opaque hides
    /// the disc and its letters; anything empty lets them through.
    #[find]
    #[live]
    pub avatar: WidgetRef,

    #[live]
    pub text: String,
    #[live]
    pub own: bool,
    #[live]
    pub name: String,
    #[live]
    pub initials: String,
    #[live]
    pub time: String,
    #[live]
    pub group: ChatGroup,
    #[live]
    pub delivery: ChatDelivery,
    #[live]
    pub streaming: bool,

    #[live(420.0)]
    pub max_width: f64,
    #[live(56.0)]
    pub min_gutter: f64,
    #[live(28.0)]
    pub avatar_size: f64,
    #[live(8.0)]
    pub avatar_gap: f64,
    #[live(10.0)]
    pub pad_x: f64,
    #[live(7.0)]
    pub pad_y: f64,
    #[live(3.0)]
    pub line_gap: f64,
    #[live(12.0)]
    pub radius: f64,
    #[live(4.0)]
    pub radius_tight: f64,
    #[live(0.75)]
    pub meta_opacity: f64,

    #[live]
    pub color_own: Vec4f,
    #[live]
    pub ink_own: Vec4f,
    #[live]
    pub meta_own: Vec4f,
    #[live]
    pub color_other: Vec4f,
    #[live]
    pub ink_other: Vec4f,
    #[live]
    pub meta_other: Vec4f,
    #[live]
    pub color_avatar: Vec4f,
    #[live]
    pub ink_avatar: Vec4f,
    #[live]
    pub color_read: Vec4f,
    #[live]
    pub color_failed: Vec4f,

    #[live(true)]
    #[visible]
    visible: bool,
}

impl ChatBubble {
    /// The width the face's column takes, whether or not this message is the
    /// one that draws a face.
    ///
    /// Reserved down the whole run: a continuation that started at the row's
    /// edge would sit further out than the message above it, and a run whose
    /// left edge moves does not read as a block.
    fn column(&self) -> f64 {
        if self.avatar_size > 0.0 {
            self.avatar_size + self.avatar_gap
        } else {
            0.0
        }
    }

    /// The room the delivery mark takes: nothing when there is none, and
    /// wider for the two-tick marks than the one-tick and ring marks.
    fn mark_size(&self) -> DVec2 {
        let height = (self.draw_meta.text_style.font_size as f64 * 1.2).max(8.0);
        let width = match self.delivery {
            ChatDelivery::Unmarked => return dvec2(0.0, 0.0),
            ChatDelivery::Delivered | ChatDelivery::Read => height * 1.45,
            _ => height,
        };
        dvec2(width, height)
    }

    /// The message's fill, and the ink on it.
    fn palette(&self) -> (Vec4f, Vec4f, Vec4f) {
        if self.own {
            (self.color_own, self.ink_own, self.meta_own)
        } else {
            (self.color_other, self.ink_other, self.meta_other)
        }
    }
}

impl Widget for ChatBubble {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        cx.begin_turtle(walk, Layout::flow_down());

        let mut row_w = cx.turtle().rect().size.x;
        if !row_w.is_finite() {
            // A parent that is itself Fit leaves the row width unknown. The
            // bubble's own cap plus its columns is the only honest answer,
            // and it is what the caller asked for anyway.
            row_w = self.max_width.max(160.0) + self.column() + self.min_gutter;
        }

        let column = self.column();
        let mut cap = (row_w - column - self.min_gutter).max(60.0);
        if self.max_width > 0.0 {
            cap = cap.min(self.max_width);
        }
        let inner_cap = (cap - self.pad_x * 2.0).max(24.0);
        let scale = (self.draw_text.font_scale as f64).max(0.0001);

        // The message, wrapped, laid out ONCE: the height the row claims and
        // the height that is drawn come from the same result, so they cannot
        // disagree.
        let body = if self.text.is_empty() {
            None
        } else {
            Some(self.draw_text.layout(
                cx,
                0.0,
                0.0,
                Some((inner_cap / scale) as f32),
                true,
                Align::default(),
                &self.text,
            ))
        };
        let body_size = body
            .as_ref()
            .map(|laid| {
                dvec2(
                    laid.size_in_lpxs.width as f64 * scale + TEXT_SLACK,
                    laid.size_in_lpxs.height as f64 * scale,
                )
            })
            .unwrap_or(dvec2(0.0, 0.0));
        // Where the last row ends, for the caret to follow. The block's own
        // width is the WIDEST row, which for a wrapped message is usually
        // not the last one.
        let last_end = body
            .as_ref()
            .and_then(|laid| laid.rows.last())
            .map(|row| (row.origin_in_lpxs.x + row.width_in_lpxs) as f64 * scale)
            .unwrap_or(0.0);
        let line_h = text_size(cx, &self.draw_text, "Ag").y;

        let heads = self.group.heads_run();
        let show_name = heads && !self.name.is_empty();
        let name_w = if show_name {
            measure(&self.draw_name, cx, &self.name)
        } else {
            0.0
        };
        let name_h = if show_name {
            text_size(cx, &self.draw_name, &self.name).y
        } else {
            0.0
        };

        let time_w = measure(&self.draw_meta, cx, &self.time);
        let time_size = text_size(cx, &self.draw_meta, &self.time);
        let mark = self.mark_size();
        let meta_w = if time_w > 0.0 && mark.x > 0.0 {
            time_w + self.line_gap + mark.x
        } else {
            time_w.max(mark.x)
        };
        let meta_h = time_size.y.max(mark.y);

        // Three dots stand in while nothing has arrived; once there is text,
        // a caret follows the last glyph.
        let dots = self.streaming && self.text.is_empty();
        let caret = self.streaming && !self.text.is_empty();
        let caret_bar = (line_h * 0.16).max(1.5);
        let caret_w = self.line_gap + caret_bar;
        let dots_w = line_h * 2.2;

        let mut content_w = body_size.x.max(name_w).max(meta_w);
        if caret {
            content_w = content_w.max(last_end + caret_w);
        }
        if dots {
            content_w = content_w.max(dots_w);
        }
        let bubble_w = (content_w + self.pad_x * 2.0).min(cap).max(self.pad_x * 2.0 + 8.0);

        let body_h = if body.is_some() {
            body_size.y
        } else if dots {
            line_h
        } else {
            0.0
        };
        let mut inner_h = body_h;
        if show_name {
            inner_h += name_h + self.line_gap;
        }
        if meta_h > 0.0 {
            inner_h += meta_h + self.line_gap;
        }
        let bubble_h = inner_h + self.pad_y * 2.0;
        let row_h = bubble_h.max(if heads { self.avatar_size } else { 0.0 });

        // Claim the row before painting: everything below is placed
        // absolutely, which contributes nothing for a turtle to measure, so
        // without this a Fit bubble would collapse to nothing.
        let row = cx.walk_turtle(Walk::new(Size::Fixed(row_w), Size::Fixed(row_h)));

        let bubble = Rect {
            pos: dvec2(
                if self.own {
                    row.pos.x + row.size.x - column - bubble_w
                } else {
                    row.pos.x + column
                },
                row.pos.y,
            ),
            size: dvec2(bubble_w, bubble_h),
        };

        let (fill, ink, meta_ink) = self.palette();
        let dim_ink = faded(meta_ink, self.meta_opacity);
        let [lt, rt, rb, lb] = self.group.radii(self.own, self.radius, self.radius_tight);
        self.draw_bg.color = fill;
        self.draw_bg.r_lt = lt as f32;
        self.draw_bg.r_rt = rt as f32;
        self.draw_bg.r_rb = rb as f32;
        self.draw_bg.r_lb = lb as f32;
        self.draw_bg.draw_abs(cx, bubble);

        if heads && self.avatar_size > 0.0 {
            let disc = Rect {
                pos: dvec2(
                    if self.own {
                        row.pos.x + row.size.x - self.avatar_size
                    } else {
                        row.pos.x
                    },
                    row.pos.y,
                ),
                size: dvec2(self.avatar_size, self.avatar_size),
            };
            let radius = (self.avatar_size * 0.5) as f32;
            self.draw_avatar.color = self.color_avatar;
            self.draw_avatar.r_lt = radius;
            self.draw_avatar.r_rt = radius;
            self.draw_avatar.r_rb = radius;
            self.draw_avatar.r_lb = radius;
            self.draw_avatar.draw_abs(cx, disc);
            if !self.initials.is_empty() {
                let size = text_size(cx, &self.draw_name, &self.initials);
                self.draw_name.color = self.ink_avatar;
                let at = dvec2(
                    disc.pos.x + (disc.size.x - size.x) * 0.5,
                    disc.pos.y + (disc.size.y - size.y) * 0.5,
                );
                draw_text_at(cx, &mut self.draw_name, at, &self.initials);
            }
            // Drawn here rather than by a container, so nothing else puts it
            // in the tree: without this a host could not reach ids!(avatar)
            // at all.
            cx.widget_tree_insert_child(self.uid, live_id!(avatar), self.avatar.clone());
            let _ = self.avatar.draw_walk(
                cx,
                scope,
                Walk::fixed(self.avatar_size, self.avatar_size).with_abs_pos(disc.pos),
            );
        }

        let left = bubble.pos.x + self.pad_x;
        let mut y = bubble.pos.y + self.pad_y;
        if show_name {
            self.draw_name.color = meta_ink;
            draw_text_at(cx, &mut self.draw_name, dvec2(left, y), &self.name);
            y += name_h + self.line_gap;
        }
        if let Some(laid) = &body {
            self.draw_text.color = ink;
            self.draw_text.draw_walk_laidout(
                cx,
                Walk::fixed(body_size.x, body_size.y).with_abs_pos(dvec2(left, y)),
                laid,
            );
        }
        if caret {
            self.draw_pulse.dots = 0.0;
            self.draw_pulse.color = ink;
            self.draw_pulse.draw_abs(
                cx,
                Rect {
                    pos: dvec2(left + last_end + self.line_gap, y + body_size.y - line_h),
                    size: dvec2(caret_bar, line_h),
                },
            );
        }
        if dots {
            self.draw_pulse.dots = 1.0;
            self.draw_pulse.color = ink;
            self.draw_pulse.draw_abs(
                cx,
                Rect {
                    pos: dvec2(left, y),
                    size: dvec2(dots_w, line_h),
                },
            );
        }

        if meta_h > 0.0 {
            let meta_y = bubble.pos.y + bubble.size.y - self.pad_y - meta_h;
            let mut right = bubble.pos.x + bubble.size.x - self.pad_x;
            if mark.x > 0.0 {
                right -= mark.x;
                self.draw_mark.mark = self.delivery;
                self.draw_mark.color = match self.delivery {
                    ChatDelivery::Read => self.color_read,
                    ChatDelivery::Failed => self.color_failed,
                    _ => dim_ink,
                };
                self.draw_mark.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(right, meta_y + (meta_h - mark.y) * 0.5),
                        size: mark,
                    },
                );
                right -= self.line_gap;
            }
            if !self.time.is_empty() {
                self.draw_meta.color = dim_ink;
                let at = dvec2(
                    right - time_size.x,
                    meta_y + (meta_h - time_size.y) * 0.5,
                );
                draw_text_at(cx, &mut self.draw_meta, at, &self.time);
            }
        }

        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.avatar.handle_event(cx, event, scope);
    }

    /// What was said, so a test reads the message the way a reader does.
    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.redraw(cx);
        }
    }

    /// Where it sits in its run: the one thing about a bubble that is not
    /// visible in its text and is worth waiting on from a test.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.group.name().to_string())
    }
}

impl ChatBubbleRef {
    /// Put one message into this bubble, including where it sits in its run.
    ///
    /// It takes no `Cx` and raises no redraw on purpose: the only caller is a
    /// list that is drawing this bubble in the same pass, and a redraw from
    /// here would schedule a whole pass to repeat work already under way.
    /// A host that changes the messages redraws the list instead.
    pub fn apply_message(&self, message: &ChatMessage, group: ChatGroup) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.own = message.own;
        inner.name = message.name.clone();
        inner.initials = message.initials.clone();
        inner.time = message.time.clone();
        inner.text = message.text.clone();
        inner.delivery = message.delivery;
        inner.streaming = message.streaming;
        inner.group = group;
    }

    pub fn set_group(&self, cx: &mut Cx, group: ChatGroup) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.group != group {
                inner.group = group;
                inner.redraw(cx);
            }
        }
    }

    pub fn group(&self) -> ChatGroup {
        self.borrow().map(|inner| inner.group).unwrap_or(ChatGroup::Single)
    }

    pub fn set_delivery(&self, cx: &mut Cx, delivery: ChatDelivery) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.delivery != delivery {
                inner.delivery = delivery;
                inner.redraw(cx);
            }
        }
    }

    /// Turn the still-arriving mark on or off. Off is what says the answer
    /// is finished, so a host that forgets this leaves a caret blinking for
    /// ever — and, because the mark reads the pass clock, leaves the window
    /// repainting at display rate with it.
    pub fn set_streaming(&self, cx: &mut Cx, streaming: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.streaming != streaming {
                inner.streaming = streaming;
                inner.redraw(cx);
            }
        }
    }
}

#[derive(Script, Widget)]
pub struct ChatList {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The list's own rect, painted as nothing.
    ///
    /// It is the redraw handle, and it has to be a thing that is drawn on
    /// every pass: hanging that off the day hairline instead would leave a
    /// list with no day separators — or one whose `rule_size` is zero —
    /// unable to redraw itself at all.
    #[redraw]
    #[live]
    draw_bg: DrawChatBox,
    /// The hairline either side of a day.
    #[live]
    draw_rule: DrawChatBox,
    #[live]
    pub draw_day: DrawText,

    /// Messages for markup; see [`parse_line`] for the shape of a line.
    #[live]
    pub lines: Vec<String>,
    #[live(300.0)]
    pub run_gap: f64,
    #[live(12.0)]
    pub spacing_run: f64,
    #[live(30.0)]
    pub day_height: f64,
    #[live(1.0)]
    pub rule_size: f64,
    #[live(10.0)]
    pub day_gap: f64,
    #[live]
    pub color_rule: Vec4f,

    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    rows: Vec<(LiveId, WidgetRef)>,
    #[rust]
    messages: Vec<ChatMessage>,
    /// Whether `lines` has been turned into messages yet.
    #[rust]
    seeded: bool,
}

impl ScriptHook for ChatList {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                self.templates
                                    .insert(id, vm.bx.heap.new_object_ref(template_obj));
                            }
                        }
                    }
                });
            }
        }
        if apply.is_reload() {
            // The bubbles come from a template that may have just changed,
            // so they are rebuilt rather than patched; and the markup may
            // have changed with it, so it is read again.
            self.rows.clear();
            self.seeded = false;
        }
    }
}

impl ChatList {
    /// The messages, in the order they were said.
    pub fn set_messages(&mut self, cx: &mut Cx, messages: Vec<ChatMessage>) {
        // Handing over the same list again is what a host does on every
        // refresh; rebuilding for it would throw away the bubbles and cost a
        // redraw a pass.
        if self.messages == messages {
            return;
        }
        // A message that keeps its id keeps its widget, so only a list whose
        // ids actually changed pays for new bubbles.
        let same_ids = self.messages.len() == messages.len()
            && self
                .messages
                .iter()
                .zip(messages.iter())
                .all(|(old, new)| old.id == new.id);
        if !same_ids {
            self.rows.clear();
        }
        self.messages = messages;
        self.redraw(cx);
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn push(&mut self, cx: &mut Cx, message: ChatMessage) {
        self.messages.push(message);
        self.redraw(cx);
    }

    /// Turn `lines` into messages once, so a list written in markup shows
    /// something without a host saying anything.
    fn seed(&mut self, cx: &mut Cx) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        if !self.messages.is_empty() || self.lines.is_empty() {
            return;
        }
        let messages = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| parse_line(i, line))
            .collect();
        self.set_messages(cx, messages);
    }

    fn row(&mut self, cx: &mut Cx, index: usize, id: LiveId) -> Option<WidgetRef> {
        if let Some((_, widget)) = self.rows.get(index) {
            return Some(widget.clone());
        }
        let template = self.templates.get(&live_id!(bubble))?;
        let value: ScriptValue = template.as_object().into();
        let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, value));
        // A tree node under the list, so the design overlay can pick a
        // message and style the template it came from.
        cx.widget_tree_insert_child(self.uid, id, widget.clone());
        self.rows.push((id, widget.clone()));
        Some(widget)
    }

    /// A day label with a hairline running out to both edges.
    fn draw_day_row(&mut self, cx: &mut Cx2d, day: &str) {
        let row = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(self.day_height)));
        let size = text_size(cx, &self.draw_day, day);
        // A list inside a Fit parent has no width to centre in; the day goes
        // at the edge rather than at NaN.
        let centred = row.size.x.is_finite();
        let left = if centred {
            row.pos.x + (row.size.x - size.x) * 0.5
        } else {
            row.pos.x
        };
        let top = row.pos.y + (row.size.y - size.y) * 0.5;
        draw_text_at(cx, &mut self.draw_day, dvec2(left, top), day);
        if self.rule_size <= 0.0 || !centred {
            return;
        }
        self.draw_rule.color = self.color_rule;
        self.draw_rule.r_lt = 0.0;
        self.draw_rule.r_rt = 0.0;
        self.draw_rule.r_rb = 0.0;
        self.draw_rule.r_lb = 0.0;
        let y = row.pos.y + (row.size.y - self.rule_size) * 0.5;
        let before = left - self.day_gap - row.pos.x;
        if before > 1.0 {
            self.draw_rule.draw_abs(
                cx,
                Rect {
                    pos: dvec2(row.pos.x, y),
                    size: dvec2(before, self.rule_size),
                },
            );
        }
        let after_x = left + size.x + self.day_gap;
        let after = row.pos.x + row.size.x - after_x;
        if after > 1.0 {
            self.draw_rule.draw_abs(
                cx,
                Rect {
                    pos: dvec2(after_x, y),
                    size: dvec2(after, self.rule_size),
                },
            );
        }
    }
}

impl Widget for ChatList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.seed(cx.cx.cx);
        self.draw_bg.begin(cx, walk, self.layout);

        // The plan is worked out for the whole list before anything is
        // drawn, because a message's own corners depend on the message BELOW
        // it as well as the one above.
        let slots = {
            let entries: Vec<_> = self.messages.iter().map(|m| m.entry()).collect();
            plan(&entries, self.run_gap)
        };
        let messages = self.messages.clone();

        for (i, message) in messages.iter().enumerate() {
            let slot = slots[i];
            if slot.new_day && !message.day.is_empty() {
                self.draw_day_row(cx, &message.day);
            }
            let Some(widget) = self.row(cx.cx.cx, i, message.id) else {
                continue;
            };
            widget.as_chat_bubble().apply_message(message, slot.group);
            // The gap inside a run is the list's own spacing; a message that
            // STARTS a run gets more room above it, which is what separates
            // one block from the next. A message under a day separator gets
            // none of it: the separator is already the gap.
            let extra = if i > 0 && slot.group.heads_run() && !slot.new_day {
                self.spacing_run
            } else {
                0.0
            };
            let _ = widget.draw_walk(
                cx,
                scope,
                Walk {
                    margin: Inset {
                        top: extra,
                        ..Inset::default()
                    },
                    width: Size::fill(),
                    height: Size::fit(),
                    ..Walk::default()
                },
            );
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let rows = self.rows.clone();
        for (_, widget) in &rows {
            widget.handle_event(cx, event, scope);
        }
    }

    /// How many messages, so a test reads the list in one line.
    fn text(&self) -> String {
        format!("{} messages", self.messages.len())
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.messages.len().to_string())
    }
}

impl ChatListRef {
    pub fn set_messages(&self, cx: &mut Cx, messages: Vec<ChatMessage>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_messages(cx, messages);
        }
    }

    pub fn push(&self, cx: &mut Cx, message: ChatMessage) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.push(cx, message);
        }
    }

    pub fn count(&self) -> usize {
        self.borrow().map(|inner| inner.messages.len()).unwrap_or(0)
    }

    /// The last message, for a host appending to a conversation: the new
    /// one's clock and day have to follow it or the run breaks for no
    /// reason a reader can see.
    pub fn last(&self) -> Option<ChatMessage> {
        self.borrow().and_then(|inner| inner.messages.last().cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(sender: u64, at: f64, day: &str) -> ChatEntry<'_> {
        ChatEntry { sender, at, day }
    }

    fn groups(entries: &[ChatEntry], gap: f64) -> Vec<ChatGroup> {
        plan(entries, gap).into_iter().map(|slot| slot.group).collect()
    }

    /// One message on its own is a run of one, and carries its own face and
    /// name because there is nothing above it to have carried them.
    #[test]
    fn a_lone_message_is_a_run_of_one() {
        let entries = [said(1, 0.0, "Monday")];
        assert_eq!(groups(&entries, 300.0), vec![ChatGroup::Single]);
        assert!(ChatGroup::Single.heads_run());
    }

    /// Three from one sender, close together: first, middle, last. Only the
    /// first heads the run.
    #[test]
    fn a_run_reads_first_middle_last() {
        let entries = [
            said(1, 0.0, "Monday"),
            said(1, 10.0, "Monday"),
            said(1, 20.0, "Monday"),
        ];
        assert_eq!(
            groups(&entries, 300.0),
            vec![ChatGroup::First, ChatGroup::Middle, ChatGroup::Last]
        );
        assert!(ChatGroup::First.heads_run());
        assert!(!ChatGroup::Middle.heads_run());
        assert!(!ChatGroup::Last.heads_run());
    }

    /// A long enough silence breaks a run even though the sender has not
    /// changed: two messages an hour apart are two thoughts, not one.
    #[test]
    fn a_long_gap_breaks_the_run() {
        let entries = [
            said(1, 0.0, "Monday"),
            said(1, 10.0, "Monday"),
            said(1, 4000.0, "Monday"),
            said(1, 4010.0, "Monday"),
        ];
        assert_eq!(
            groups(&entries, 300.0),
            vec![
                ChatGroup::First,
                ChatGroup::Last,
                ChatGroup::First,
                ChatGroup::Last
            ]
        );
    }

    /// The other party speaking breaks it too, and the sender who comes back
    /// starts a fresh run.
    #[test]
    fn the_other_side_speaking_breaks_the_run() {
        let entries = [
            said(1, 0.0, "Monday"),
            said(2, 10.0, "Monday"),
            said(1, 20.0, "Monday"),
        ];
        assert_eq!(
            groups(&entries, 300.0),
            vec![ChatGroup::Single, ChatGroup::Single, ChatGroup::Single]
        );
    }

    /// A change of day breaks the run whatever the clock says, because a
    /// separator is about to be drawn through the middle of it.
    #[test]
    fn a_new_day_breaks_the_run_and_asks_for_a_separator() {
        let entries = [
            said(1, 0.0, "Monday"),
            said(1, 5.0, "Monday"),
            said(1, 10.0, "Tuesday"),
        ];
        let slots = plan(&entries, 300.0);
        assert_eq!(
            slots.iter().map(|s| s.group).collect::<Vec<_>>(),
            vec![ChatGroup::First, ChatGroup::Last, ChatGroup::Single]
        );
        assert_eq!(
            slots.iter().map(|s| s.new_day).collect::<Vec<_>>(),
            vec![true, false, true],
            "the first message always names its day, and so does the one that changes it"
        );
    }

    /// Messages handed over out of order are not a run in any order the
    /// reader can see.
    #[test]
    fn a_backwards_clock_breaks_the_run() {
        let entries = [said(1, 100.0, "Monday"), said(1, 40.0, "Monday")];
        assert_eq!(
            groups(&entries, 300.0),
            vec![ChatGroup::Single, ChatGroup::Single]
        );
    }

    /// An empty list plans nothing rather than panicking on the lookahead.
    #[test]
    fn an_empty_conversation_plans_nothing() {
        assert!(plan(&[], 300.0).is_empty());
    }

    /// Only the edge the run stands on tightens: the right for your own
    /// messages, the left for the other party's. Tightening all four would
    /// make every bubble squarer and say nothing about what belongs with
    /// what.
    #[test]
    fn only_the_joining_edge_tightens() {
        // [left-top, right-top, right-bottom, left-bottom]
        assert_eq!(ChatGroup::Single.radii(false, 12.0, 4.0), [12.0, 12.0, 12.0, 12.0]);
        assert_eq!(ChatGroup::First.radii(false, 12.0, 4.0), [12.0, 12.0, 12.0, 4.0]);
        assert_eq!(ChatGroup::Middle.radii(false, 12.0, 4.0), [4.0, 12.0, 12.0, 4.0]);
        assert_eq!(ChatGroup::Last.radii(false, 12.0, 4.0), [4.0, 12.0, 12.0, 12.0]);

        assert_eq!(ChatGroup::First.radii(true, 12.0, 4.0), [12.0, 12.0, 4.0, 12.0]);
        assert_eq!(ChatGroup::Middle.radii(true, 12.0, 4.0), [12.0, 4.0, 4.0, 12.0]);
        assert_eq!(ChatGroup::Last.radii(true, 12.0, 4.0), [12.0, 4.0, 12.0, 12.0]);
    }

    #[test]
    fn a_clock_is_read_and_anything_else_is_not() {
        assert_eq!(clock_seconds("09:41"), Some(9.0 * 3600.0 + 41.0 * 60.0));
        assert_eq!(clock_seconds(" 23:59 "), Some(23.0 * 3600.0 + 59.0 * 60.0));
        assert_eq!(clock_seconds("24:00"), None);
        assert_eq!(clock_seconds("09:60"), None);
        assert_eq!(clock_seconds("later"), None);
    }

    #[test]
    fn a_markup_line_carries_the_side_the_flags_and_the_bars_in_the_text() {
        let message = parse_line(0, "own:read|Ada Byron|09:41|Monday|yes | and no").unwrap();
        assert!(message.own);
        assert_eq!(message.delivery, ChatDelivery::Read);
        assert!(!message.streaming);
        assert_eq!(message.name, "Ada Byron");
        assert_eq!(message.initials, "AB");
        assert_eq!(message.text, "yes | and no");
        assert_eq!(message.at, clock_seconds("09:41").unwrap());

        let other = parse_line(1, "other:streaming|Ada|09:42|Monday|").unwrap();
        assert!(!other.own);
        assert!(other.streaming);
        assert_eq!(other.delivery, ChatDelivery::Unmarked);
        assert_eq!(other.text, "");
        assert!(parse_line(2, "   ").is_none());
    }

    /// The same name is the same speaker, which is all markup has to go on.
    #[test]
    fn a_name_is_a_sender() {
        assert_eq!(sender_of("Ada"), sender_of("Ada"));
        assert_ne!(sender_of("Ada"), sender_of("ada"));
    }

    /// Lines with no clock still group: their position stands in for one, a
    /// second apart, which is inside any sane run gap.
    #[test]
    fn undated_lines_still_group_by_sender() {
        let lines = [
            "other|Ada||Monday|one",
            "other|Ada||Monday|two",
            "own|You||Monday|three",
        ];
        let messages: Vec<ChatMessage> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| parse_line(i, line))
            .collect();
        let entries: Vec<_> = messages.iter().map(|m| m.entry()).collect();
        assert_eq!(
            groups(&entries, 300.0),
            vec![ChatGroup::First, ChatGroup::Last, ChatGroup::Single]
        );
    }
}
