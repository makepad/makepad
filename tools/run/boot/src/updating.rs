//! Display-only wait card, shaped by the boot renderer. Its face and progress track are
//! document quads; it creates no hit targets and reads no compiler state itself.
use crate::{
    document::{Program, Role},
    text::Text,
    Layout, RingWant, TextLine,
};

pub struct Card {
    pub rect: [f32; 4],
    pub track: [f32; 4],
    pub progress: f32,
}

pub fn layout(
    text: &mut Text,
    program: &Program,
    work: RingWant,
    viewport: [f32; 2],
    layout: &mut Layout,
) -> Card {
    layout.fixed_text.clear();
    layout.cards.clear();
    let width = 420.0_f32.min((viewport[0] - 32.0).max(1.0));
    let padding = 20.0_f32.min(width * 0.1);
    let inner = (width - padding * 2.0).max(1.0);
    let left = (viewport[0] - width) * 0.5;
    let total = work.done.saturating_add(work.pending);
    let progress = if total > 0 { work.done as f32 / total as f32 } else { 0.0 };
    let title = if work.rebuild != 0 { "Rebuilding makepad…" } else { "Updating…" };
    let doing = if work.rebuild == 2 {
        "Restarting into the rebuilt makepad"
    } else if work.linking {
        "Installing cached functions"
    } else if work.settling {
        "Finishing the settle rebuild"
    } else {
        "Compiling functions for the UI"
    };
    let percent = if total > 0 {
        format!("{:.0}% complete", progress * 100.0)
    } else {
        "Progress pending…".to_string()
    };
    let queues = format!("O2 queue {} · O3 queue {} · {:.0} fns/s", work.o2, work.o3, work.rate);
    let elapsed = format!("{:.1} s elapsed", work.busy.as_secs_f32());
    let mut y = padding;
    let mut track_y = 0.0;
    for (index, (value, role)) in [
        (title, Role::UpdatingTitle),
        (doing, Role::UpdatingStatus),
        (percent.as_str(), Role::UpdatingStatus),
        (queues.as_str(), Role::UpdatingDetail),
        (elapsed.as_str(), Role::UpdatingDetail),
    ]
    .into_iter()
    .enumerate()
    {
        let style = program.line(role);
        let run = text.shape(value, style.size, inner, style.height);
        let height = run.rows as f32 * style.height;
        layout.fixed_text.push(TextLine {
            run,
            position: [left + padding, y],
            style,
            fixed: true,
        });
        y += height + style.gap;
        if index == 2 {
            track_y = y;
            y += 18.0;
        }
    }
    let height = y + padding;
    let top = (viewport[1] - height) * 0.5;
    for line in &mut layout.fixed_text {
        line.position[1] += top;
    }
    Card {
        rect: [left, top, width, height],
        track: [left + padding, top + track_y, inner, 4.0],
        progress,
    }
}
