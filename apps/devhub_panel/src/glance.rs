//! The panel at a glance: what its menu-bar item shows (the glyph, the tooltip, the menu's
//! status lines), from the fleet it holds and that fleet's needs. Nothing in it is a clock or
//! an age the panel counts itself, so it changes only when the fleet does and the tray is set
//! again rarely.

use crate::model::{Fleet, Link};
use crate::needs::{Need, Sev};
use makepad_devhub_conf::tray_glyph::Glyph;

/// How many needs the menu names, worst first; the rest are counted.
pub const NEEDS: usize = 3;
/// A menu line's longest text, characters.
const LINE: usize = 72;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glance {
    pub glyph: Glyph,
    pub tooltip: String,
    /// the menu's status lines, above its actions
    pub lines: Vec<String>,
}

fn clip(s: &str) -> String {
    if s.chars().count() <= LINE {
        return s.to_string();
    }
    format!("{}\u{2026}", s.chars().take(LINE - 1).collect::<String>())
}

impl Glance {
    /// Off while the master is not read (connecting, lost, without the panel service);
    /// attention while a red need stands; busy while leases run; quiet else.
    pub fn of(f: &Fleet, needs: &[Need]) -> Glance {
        let off = match &f.link {
            Link::Connecting => Some("connecting to the master".to_string()),
            Link::Lost { why, .. } => Some(format!("master not answering ({why})")),
            Link::Older => Some("this master has no panel service yet".to_string()),
            Link::Live | Link::Demo => None,
        };
        if let Some(word) = off {
            return Glance { glyph: Glyph::Off, tooltip: format!("devhub panel \u{b7} {word}"), lines: vec![clip(&format!("The panel is {word}"))] };
        }
        let (running, waiting) = (f.running(), f.queued().len());
        let red = needs.iter().filter(|n| n.sev == Sev::Red).count();
        let (glyph, word) = if red > 0 {
            (Glyph::Attention, format!("{red} need{} a look", if red == 1 { "s" } else { "" }))
        } else if running > 0 {
            (Glyph::Busy, "busy".to_string())
        } else {
            (Glyph::Quiet, "idle".to_string())
        };
        let mut lines = Vec::new();
        if f.link == Link::Demo {
            lines.push("Demo fleet (synthetic)".to_string());
        }
        let up = f.nodes.iter().filter(|n| n.up).count();
        lines.push(format!("Nodes {up}/{} up \u{b7} leases {running} running, {waiting} waiting", f.nodes.len()));
        lines.extend(needs.iter().take(NEEDS).map(|n| clip(&format!("{} \u{b7} {}", n.what.tag(), n.title))));
        match needs.len() {
            0 => lines.push("Nothing needs a look".to_string()),
            n if n > NEEDS => lines.push(format!("\u{2026} and {} more", n - NEEDS)),
            _ => {}
        }
        Glance { glyph, tooltip: format!("devhub panel \u{b7} {word}"), lines }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::needs::needs;

    /// The demo fleet's planted red needs make the glyph ask for a look, and the menu names the
    /// worst of them; without needs, running leases make it busy; an unread master is off.
    #[test]
    fn the_glyph_follows_needs_then_work_then_the_link() {
        let start = 1_800_000_000_000;
        let f = crate::demo::Demo::new(start).fleet(start);
        let v = needs(&f);
        let g = Glance::of(&f, &v);
        assert_eq!(g.glyph, Glyph::Attention);
        assert!(g.tooltip.contains("a look"), "{}", g.tooltip);
        assert_eq!(g.lines[0], "Demo fleet (synthetic)");
        assert!(g.lines[2].starts_with(v[0].what.tag()), "{:?}", g.lines);
        assert!(v.len() <= NEEDS || g.lines.last().is_some_and(|l| l.contains("more")), "{:?}", g.lines);
        assert!(g.lines.iter().all(|l| l.chars().count() <= LINE));
        assert_eq!(Glance::of(&f, &v), g, "the same fleet, the same glance");

        let calm = Glance::of(&f, &[]);
        assert_eq!(calm.glyph, if f.running() > 0 { Glyph::Busy } else { Glyph::Quiet });
        assert_eq!(calm.lines.last().map(String::as_str), Some("Nothing needs a look"));

        let lost = Fleet { link: Link::Lost { since_ms: start, why: "link lost".into() }, ..f.clone() };
        assert_eq!(Glance::of(&lost, &v).glyph, Glyph::Off);
        assert_eq!(Glance::of(&Fleet::default(), &[]).glyph, Glyph::Off);
    }
}
