//! A song rendered offline: the mix, and stems by role (drums, bass,
//! vocals, the rest), each the same score with the other loops muted, so
//! the stems carry the same FX, buses and master as the mix. A sung song
//! also renders its lead dry (no reverb or delay) for the pitch check.

use makepad_score_live::compose::Played;
use makepad_score_live::model::{InstKind, ScoreDoc};
use makepad_score_live::{render, source, NoSamples, RenderOpts, Rendered};
use makepad_score_player::FxKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Drums,
    Bass,
    Vocals,
    Other,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Drums, Role::Bass, Role::Vocals, Role::Other];

    pub fn name(self) -> &'static str {
        match self {
            Role::Drums => "drums",
            Role::Bass => "bass",
            Role::Vocals => "vocals",
            Role::Other => "other",
        }
    }
}

pub struct Stem {
    pub role: Role,
    pub loops: Vec<String>,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

pub struct Song {
    pub doc: ScoreDoc,
    pub mix: Rendered,
    pub roles: Vec<(String, Role)>,
    pub stems: Vec<Stem>,
    /// The sung lead alone and dry, with its loop's name.
    pub lead_vocal: Option<(String, Vec<f32>)>,
}

impl Song {
    pub fn stem(&self, role: Role) -> Option<&Stem> {
        self.stems.iter().find(|s| s.role == role)
    }
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

/// Every loop's role: by its instrument (a kit plays drums, a voice
/// sings), else by name and register (a loop called bass or sub, or one
/// whose notes sit below D3, is bass).
pub fn roles(doc: &ScoreDoc, played: &[Played]) -> Vec<(String, Role)> {
    doc.loops
        .iter()
        .map(|l| {
            let kind = l.play.as_deref().and_then(|p| doc.inst(p)).map(|i| &i.kind);
            let drum_hits = l.hits.as_ref().is_some_and(|h| h.drum.is_some());
            let role = match kind {
                Some(InstKind::Drums { .. }) => Role::Drums,
                Some(InstKind::Native { native, .. }) if native == "drums" => Role::Drums,
                Some(InstKind::Voice { .. }) => Role::Vocals,
                _ if drum_hits => Role::Drums,
                _ => {
                    let notes: Vec<f32> = played.iter().filter(|p| p.loop_name == l.name && !p.hit).map(|p| p.note).collect();
                    let low = median(notes).is_some_and(|n| n < 50.0);
                    let named = { let n = l.name.to_lowercase(); n.contains("bass") || n.contains("sub") || n.contains("808") };
                    if named || low { Role::Bass } else { Role::Other }
                }
            };
            (l.name.clone(), role)
        })
        .collect()
}

/// `src` with every loop not in `keep` muted.
pub fn only(src: &str, doc: &ScoreDoc, keep: &[String]) -> Result<String, String> {
    let mut out = src.to_string();
    for l in &doc.loops {
        if !keep.contains(&l.name) && !l.mute {
            out = source::apply(&out, &source::set_field(&out, &l.name, "mute", "true")?);
        }
    }
    Ok(out)
}

fn diag_text(d: &[makepad_score_live::Diag]) -> String {
    d.iter().map(|d| format!("{d:?}")).collect::<Vec<_>>().join("; ")
}

pub fn render_mix(src: &str, opts: &RenderOpts) -> Result<Rendered, String> {
    render(src, opts, &NoSamples).map_err(|d| diag_text(&d))
}

/// Render the mix, then each role's stem and a sung lead.
pub fn render_song(src: &str, opts: &RenderOpts, with_stems: bool) -> Result<Song, String> {
    let doc = makepad_score_live::model::load(src).map_err(|d| diag_text(&d))?.doc;
    let mix = render_mix(src, opts)?;
    let roles = roles(&doc, &mix.played);
    let seconds = Some(mix.seconds());
    let opts = RenderOpts { seconds, ..opts.clone() };
    let mut stems = Vec::new();
    if with_stems {
        for role in Role::ALL {
            let loops: Vec<String> = roles.iter().filter(|(_, r)| *r == role).map(|(n, _)| n.clone()).collect();
            if loops.is_empty() || !mix.played.iter().any(|p| loops.contains(&p.loop_name)) {
                continue;
            }
            let r = render_mix(&only(src, &doc, &loops)?, &opts)?;
            stems.push(Stem { role, loops, left: r.left, right: r.right });
        }
    }
    let lead = roles
        .iter()
        .filter(|(_, r)| *r == Role::Vocals)
        .map(|(n, _)| (n.clone(), mix.played.iter().filter(|p| &p.loop_name == n && !p.hit).count()))
        .filter(|(n, count)| *count > 0 && doc.loop_def(n).and_then(|l| l.play.as_deref()).and_then(|p| doc.inst(p)).is_some_and(|i| matches!(i.kind, InstKind::Voice { singers: 0 | 1, .. })))
        .max_by_key(|(_, count)| *count)
        .map(|(n, _)| n);
    let lead_vocal = match lead {
        Some(name) => {
            let mut dry = only(src, &doc, std::slice::from_ref(&name))?;
            for fx in doc.fx.iter().filter(|f| matches!(f.kind, FxKind::Reverb | FxKind::Delay)) {
                if let Ok(s) = source::set_field(&dry, &fx.name, "mix", "0") {
                    dry = source::apply(&dry, &s);
                }
            }
            let r = render_mix(&dry, &opts)?;
            Some((name, r.left.iter().zip(&r.right).map(|(l, r)| 0.5 * (l + r)).collect()))
        }
        None => None,
    };
    Ok(Song { doc, mix, roles, stems, lead_vocal })
}

/// 16-bit stereo WAV bytes.
pub fn wav(rate: f32, left: &[f32], right: &[f32]) -> Vec<u8> {
    makepad_score_voice::render::wav_bytes(rate as u32, &[left, right])
}
