//! The script tweens page: every motion on it is written in Splash script
//! with `tween` (`mod.tween`), the GSAP-style layer over the tween engine.
//! Button handlers call `tween.to` / `from` / `from_to` / `set`, build a
//! timeline with labels, a `call()` and callbacks, and stagger a list of
//! handles. No Rust drives the motion: the page is the script.
//!
//! The readouts are labels with ids, so `/snap?q=st_` on the `--remote`
//! surface reads what the callbacks wrote where no screenshot is available:
//! `st_state` is written by the timeline's `on_update` (only when the text
//! changes), `st_call` by its `call()`, `st_done` by `on_complete`,
//! `st_note`, `st_wave_note` and `st_path_note` by the tweens'
//! `on_complete`.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Readout = Label{
        width: Fill
        text: "-"
        draw_text +: {
            text_style: theme.font_code{font_size: theme.font_size_p}
            color: theme.color_text
        }
    }

    // A lane a box travels along: its left margin is what moves.
    let Lane = View{width: Fill height: 44 align: Align{y: 0.5}}

    let TweenBox = RoundedView{
        width: 40
        height: 36
        margin: Inset{left: 0.}
        draw_bg +: {color: #x5b6cff border_radius: 6.0}
    }

    // A lane a box follows a motion path in: in a `flow: Overlay` parent
    // margin.left / margin.top are the box's x / y.
    let PathLane = View{width: Fill height: 120 flow: Overlay}

    let Bar = RoundedView{
        width: 18
        height: 8
        draw_bg +: {color: theme.color_tertiary border_radius: 3.0}
    }

    // The page's script state: the timeline (built on first use) and the
    // last state line, so on_update writes the label only when it changes.
    let st = {tl: nil, go: nil, last: "", completes: 0}

    fn f2(x) {
        return "" + round(x * 100.0) / 100.0
    }

    fn state_of(tl) {
        let s = "t " + f2(tl.time()) + "/" + f2(tl.duration())
        s = s + " | total " + f2(tl.total_time()) + "/" + f2(tl.total_duration())
        s = s + " | progress " + f2(tl.progress()) + " | x" + tl.time_scale()
        if tl.paused() {
            s = s + " | paused"
        } else {
            s = s + " | playing"
        }
        if tl.reversed() {
            s = s + " reversed"
        }
        return s + " | label " + tl.current_label() + " | active " + tl.is_active()
    }

    fn show(ui, tl) {
        let s = state_of(tl)
        if s != st.last {
            st.last = s
            ui.st_state.set_text(s)
        }
    }

    fn build_tl(ui) {
        let tl = tween.timeline({
            defaults: {duration: 0.5, ease: Ease.OutQuad}
            repeat: 1
            yoyo: true
            repeat_delay: 0.2
            paused: true
        })
        // Builders chain (each returns the timeline); a chain stays on one line.
        tl.set(ui.st_tl_c, {draw_bg: {color: #x5b6cff}}, 0.0).to(ui.st_tl_a, {margin: {left: 260.0}})
        tl.to(ui.st_tl_b, {margin: {left: 260.0}}, "+=0.2").from(ui.st_tl_c, {width: 4.0}, "<").add_label(@shown)
        tl.call(|| ui.st_call.set_text("call(): fired at shown+0.1, t " + f2(tl.time())), "shown+=0.1")
        tl.to(ui.st_tl_c, {draw_bg: {color: #xff8a3d}}, "shown").add_label(@end, ">")
        tl.on_update(|| show(ui, tl))
        tl.on_complete(|| {
            st.completes = st.completes + 1
            ui.st_done.set_text("on_complete #" + st.completes)
        })
        tl.on_reverse_complete(|| ui.st_done.set_text("on_reverse_complete"))
        return tl
    }

    // The page's timeline, built on first use. A page instantiated again
    // (a story switch and back, a style reload) has new widgets: the old
    // timeline went with the old ones (is_alive() is false), so it is built
    // again for this page's widgets.
    fn the_tl(ui) {
        if st.tl == nil || !st.tl.is_alive() {
            if st.tl != nil {
                st.tl.kill()
            }
            st.tl = build_tl(ui)
            st.last = ""
        }
        return st.tl
    }

    mod.stories.FoundationsMotionScriptTweens = StoryPage{
        StoryNote{text: "Everything that moves on this page is Splash script. The buttons' on_click handlers call tween.to, tween.from, tween.from_to and tween.set, build a timeline with labels and a call(), and stagger a list of ui handles. Callbacks (on_update, on_complete, call) are script closures the engine calls after the frame that fired them; the lines under each section are what they wrote."}

        StoryHeading{text: "One call, one tween"}
        StoryRow{
            st_go := Button{text: "tween.to" on_click: || {
                st.go = tween.to(ui.st_box, {
                    duration: theme.motion_extra_long_2
                    ease: theme.motion_ease_standard
                    margin: {left: 300.0}
                    draw_bg: {color: #x3fb8af}
                    on_complete: || ui.st_note.set_text("tween.to: on_complete")
                    on_reverse_complete: || ui.st_note.set_text("tween.to: on_reverse_complete")
                })
            }}
            // The tween.to handle outlives the tween's completion while the
            // script holds it, as in GSAP: reverse() plays it back.
            st_rev := Button{text: "h.reverse()" on_click: || {
                if st.go != nil {
                    st.go.reverse()
                }
            }}
            st_back := Button{text: "back, power2.out" on_click: || {
                tween.to(ui.st_box, {duration: 0.4 ease: "power2.out" margin: {left: 0.0} draw_bg: {color: #x5b6cff}})
            }}
            st_from := Button{text: "tween.from" on_click: || tween.from(ui.st_box, {width: 4.0 duration: 0.6 ease: "back.out(1.7)"})}
            st_grow := Button{text: "width +=20" on_click: || tween.to(ui.st_box, {width: "+=20" duration: 0.25})}
        }
        StoryRow{
            st_fromto := Button{text: "tween.from_to" on_click: || {
                tween.from_to(ui.st_box, {margin: {left: 0.0}}, {margin: {left: 150.0} duration: 0.5 ease: "steps(5)"})
            }}
            st_set := Button{text: "tween.set" on_click: || {
                tween.set(ui.st_box, {width: 40.0 margin: {left: 0.0} draw_bg: {color: #x5b6cff}})
                ui.st_note.set_text("tween.set: applied")
            }}
            st_stop := Button{text: "kill_tweens_of" on_click: || tween.kill_tweens_of(ui.st_box)}
            st_clear := Button{text: "clear_props" on_click: || {
                tween.clear_props(ui.st_box)
                ui.st_note.set_text("clear_props: DSL values back")
            }}
        }
        Lane{st_box := TweenBox{}}
        st_note := Readout{text: "tween: -"}

        StoryHeading{text: "A timeline built in script"}
        StoryNote{text: "tween.timeline({defaults: {duration: 0.5, ease: Ease.OutQuad}, repeat: 1, yoyo: true, repeat_delay: 0.2, paused: true}), then set, to, to at \"+=0.2\", from at \"<\", the label @shown, a call() at \"shown+=0.1\", a colour tween at \"shown\" and the label @end. on_update writes the first line below, call() the second, on_complete the third."}
        StoryRow{
            st_play := Button{text: "play" on_click: || {
                let tl = the_tl(ui)
                tl.play()
                show(ui, tl)
            }}
            st_pause := Button{text: "pause" on_click: || {
                let tl = the_tl(ui)
                tl.pause()
                show(ui, tl)
            }}
            st_resume := Button{text: "resume" on_click: || {
                let tl = the_tl(ui)
                tl.resume()
                show(ui, tl)
            }}
            st_reverse := Button{text: "reverse" on_click: || {
                let tl = the_tl(ui)
                tl.reverse()
                show(ui, tl)
            }}
            st_restart := Button{text: "restart" on_click: || {
                let tl = the_tl(ui)
                tl.restart()
                show(ui, tl)
            }}
            st_seek := Button{text: "seek @shown" on_click: || {
                let tl = the_tl(ui)
                tl.pause()
                tl.seek(@shown)
                show(ui, tl)
            }}
            st_half := Button{text: "progress 0.5" on_click: || {
                let tl = the_tl(ui)
                tl.progress(0.5)
                show(ui, tl)
            }}
            st_speed := Button{text: "time_scale 2 / 1" on_click: || {
                let tl = the_tl(ui)
                if tl.time_scale() == 1.0 {
                    tl.time_scale(2.0)
                } else {
                    tl.time_scale(1.0)
                }
                show(ui, tl)
            }}
            st_kill := Button{text: "kill" on_click: || {
                if st.tl != nil {
                    st.tl.kill()
                    st.tl = nil
                    st.last = ""
                }
                ui.st_state.set_text("killed: play builds a new timeline")
            }}
        }
        st_state := Readout{text: "timeline: press play"}
        st_call := Readout{text: "call(): -"}
        st_done := Readout{text: "on_complete: -"}
        Lane{st_tl_a := TweenBox{}}
        Lane{st_tl_b := TweenBox{}}
        Lane{st_tl_c := TweenBox{width: 60}}

        StoryHeading{text: "Stagger over a list of handles, and the ticker"}
        StoryRow{
            st_wave := Button{text: "stagger from @center" on_click: || {
                tween.to([ui.st_b1, ui.st_b2, ui.st_b3, ui.st_b4, ui.st_b5, ui.st_b6, ui.st_b7, ui.st_b8], {
                    height: 56.0
                    duration: 0.35
                    ease: "power2.inOut"
                    yoyo: true
                    repeat: 1
                    stagger: {each: 0.06, from: @center, grid: [1, 8], axis: @x, ease: Ease.OutQuad}
                    on_complete: || ui.st_wave_note.set_text("stagger: on_complete, bars back at 8")
                })
            }}
            st_slow := Button{text: "ticker 0.25 / 1" on_click: || {
                if tween.ticker().time_scale == 1.0 {
                    tween.ticker({time_scale: 0.25})
                } else {
                    tween.ticker({time_scale: 1.0})
                }
                ui.st_ticker.set_text("ticker time_scale " + tween.ticker().time_scale)
            }}
        }
        st_bars := View{
            width: Fit
            height: 64
            flow: Right
            spacing: 6
            align: Align{y: 1.0}
            st_b1 := Bar{}
            st_b2 := Bar{}
            st_b3 := Bar{}
            st_b4 := Bar{}
            st_b5 := Bar{}
            st_b6 := Bar{}
            st_b7 := Bar{}
            st_b8 := Bar{}
        }
        st_wave_note := Readout{text: "stagger: -"}
        st_ticker := Readout{text: "ticker time_scale 1"}

        StoryHeading{text: "Motion path"}
        StoryRow{
            st_path_go := Button{text: "motion_path" on_click: || {
                // Back to the lane's start, so the button replays: align @start
                // moves the path onto wherever the box is.
                tween.set(ui.st_path_box, {margin: {left: 20.0 top: 48.0}})
                tween.to(ui.st_path_box, {
                    duration: 2.0
                    ease: "power1.inOut"
                    motion_path: {path: [[0, 0], [120, -40], [240, 40], [360, 0]], curviness: 1.25, align: @start, auto_rotate: true}
                    on_complete: || ui.st_path_note.set_text("motion_path: on_complete")
                })
            }}
        }
        PathLane{
            st_path_box := View{
                width: 40
                height: 24
                margin: Inset{left: 20 top: 48}
                show_bg: true
                draw_bg +: {
                    rotation: instance(0.0)
                    color: #x5b6cff
                    pixel: fn() {
                        let c = self.rect_size * 0.5
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.rotate(self.rotation * 0.017453292, c.x, c.y)
                        sdf.box(4.0, 6.0, self.rect_size.x - 8.0, self.rect_size.y - 12.0, 3.0)
                        sdf.fill(self.color)
                        return sdf.result
                    }
                }
            }
        }
        st_path_note := Readout{text: "motion_path: -"}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "foundations/motion/script-tweens",
    category: "Foundations",
    component: "Motion",
    also: &[],
    name: "Script tweens",
    dsl: "FoundationsMotionScriptTweens",
    added: "2026-09-25",
    tags: &["new", "tween", "timeline", "gsap", "script", "splash", "stagger", "ticker", "callbacks"],
    doc: "# Script tweens\n\n`tween` (`mod.tween`) is the tween engine from Splash script, shaped like GSAP. Every widget script has it:\n\n```\ntween.to(ui.card, {duration: 0.4, ease: Ease.OutCubic, draw_bg: {color: #x3fb8af}})\nlet tl = tween.timeline({defaults: {duration: 0.3}, repeat: 1, yoyo: true, paused: true})\ntl.to(ui.a, {margin: {left: 100.0}}).to(ui.b, {width: 80.0}, \"+=0.2\").add_label(@shown)\n  .call(|| ui.status.set_text(\"shown\"), \"shown+=0.1\")\ntl.on_complete(|| ui.status.set_text(\"done\"))\ntl.play()\n```\n\n- **Targets** are `ui` handles (`ui`, `ui.name`) or an array of them, in stagger order.\n- **vars**: `duration, delay, ease, repeat, repeat_delay, yoyo, yoyo_ease, stagger, overwrite, paused, reversed, time_scale, color_space, id` and `on_start, on_update, on_repeat, on_complete, on_reverse_complete, on_interrupt` are options; every other key is a property, and a nested object is a path (`draw_bg: {color: ..}`). Values are numbers, colours, vec2/vec3/vec4 and `\"+=n\"` / `\"-=n\"`.\n- **Eases**: `Ease.OutCubic`, `theme.motion_ease_*`, a GSAP string (`\"power2.out\"`, `\"back.out(1.7)\"`, `\"steps(5)\"`, `\"cubic-bezier(.2,0,0,1)\"`) or a CSS preset name.\n- **Positions**: numbers, `@labels`, or GSAP strings (`\"<\"`, `\">\"`, `\"+=0.2\"`, `\"shown+=0.1\"`).\n- **Handles** from `tween.to` and `tween.timeline` answer `play, pause, resume, reverse, restart, seek, kill`, the getters/setters `time, total_time, progress, total_progress, time_scale, paused, reversed, duration` and `total_duration, iteration, is_active, current_label, is_alive`, and `on_*(fn)`. A root tween stays addressable after it completes while script holds its handle (`h.restart()` works); an animation whose widgets are all gone is killed with them.\n- **Motion paths**: `motion_path` in `to()` vars makes the tween follow a path: an SVG path string, `[[x, y], ..]` (a smooth curve through the points) or `{path, type: @thru | @cubic, curviness, align: @start, auto_rotate: true | degrees, radians, start, end, offset, x, y, z, rotation}`. It drives `margin.left` / `margin.top` (an x / y in a `flow: Overlay` parent) and, with `auto_rotate`, `draw_bg.rotation` in degrees, unless `x`, `y`, `rotation` name other keys.\n- **Globals**: `tween.kill_tweens_of(targets, props?)`, `tween.clear_props(targets, props?)` (forgets what the tween layer holds for those keys and puts the DSL values back), `tween.is_tweening(target)`, `tween.ticker({time_scale, paused, reduced_motion, lag_smoothing})` (the app-wide ticker, main VM only; `tween.ticker()` reads it; lag smoothing in seconds).\n\nCallbacks run after the frame that fired them (GSAP runs them inside the render). A `to()` starts from the value the tween layer last wrote, else from the widget's DSL value at that path: a value the Animator changed is not seen, and a key the DSL never set needs `from_to`.\n\n## This page\n\nThe first row tweens one box (`clear_props` puts its DSL values back); the timeline row plays a timeline built on first use and kept in a script variable, built again when the page was instantiated anew; the stagger row staggers eight bars from the centre and toggles the ticker's time scale; the last button sends a box along a curve through four points with `motion_path` (`align: @start`, `auto_rotate: true`), from the lane's start. The readouts (`st_state`, `st_call`, `st_done`, `st_note`, `st_wave_note`, `st_ticker`, `st_path_note`) are written by the script's callbacks, so `/snap?q=st_` reads them.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];
