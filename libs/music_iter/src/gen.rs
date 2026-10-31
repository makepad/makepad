//! The model's side of the loop: writing a song as a whole Score from a
//! request (one reply, one ```splash block, repaired with the loader's and
//! the loops' errors), and learning: reading the rated songs' Scores and
//! verdicts and rewriting MUSIC-AGENTS.md's learned rules.
//!
//! A [`Writer`] is one blocking chat turn; [`ClaudeCode`] is Stage's chat
//! provider (the person's `claude` CLI, Opus 5.5 unless
//! `STAGE_MUSIC_MODEL` names another). Each song runs on its own thread, so
//! a batch's songs are written and rendered in parallel.

use crate::render::{render_song, Song};
use makepad_ai_hub::chat_wire::{ChatMessage, ChatRole};
use makepad_ai_hub::providers::claude::ClaudeCodeChatProvider;
use makepad_ai_hub::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_score_live::RenderOpts;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// Activity of the turns a panel owns. Silence changes the status only;
/// cancellation remains the person's choice through the turn's flag.
#[derive(Clone, Default)]
pub struct TurnMonitor {
    turns: Arc<Mutex<Vec<Weak<TurnActivity>>>>,
}

/// Held for one running turn. Dropping it removes that turn from the
/// monitor, including when its worker unwinds.
pub struct TurnActivity {
    last_progress: Mutex<Instant>,
}

impl TurnMonitor {
    pub fn begin(&self) -> Arc<TurnActivity> {
        let activity = Arc::new(TurnActivity { last_progress: Mutex::new(Instant::now()) });
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        turns.retain(|turn| turn.strong_count() > 0);
        turns.push(Arc::downgrade(&activity));
        activity
    }

    pub fn not_responding(&self) -> bool {
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let mut quiet = false;
        turns.retain(|turn| {
            let Some(turn) = turn.upgrade() else { return false };
            quiet |= turn.last_progress.lock().unwrap_or_else(|e| e.into_inner()).elapsed() >= Duration::from_secs(60);
            true
        });
        quiet
    }
}

impl TurnActivity {
    /// Tokens, reasoning, status changes and tool events are all progress.
    pub fn progress(&self) {
        *self.last_progress.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }
}

/// One blocking chat turn: the system text and the conversation so far
/// (user, assistant, user, …), the reply's text.
pub trait Writer: Send + Sync {
    fn chat(&self, system: &str, turns: &[String], cancel: &AtomicBool) -> Result<String, String>;
    fn label(&self) -> String;

    /// A turn with the tools of one MCP server (`mcp`: the
    /// `{"mcpServers":{…}}` config and the `--allowedTools` pattern), which
    /// the model calls as often as it likes within the turn; a writer
    /// without tools answers as [`Writer::chat`].
    fn chat_with(&self, system: &str, turns: &[String], cancel: &AtomicBool, mcp: Option<&(String, String)>) -> Result<String, String> {
        let _ = mcp;
        self.chat(system, turns, cancel)
    }
}

pub const DEFAULT_MODEL: &str = "claude-opus-5-5[1m]";

pub struct ClaudeCode {
    pub model: String,
    pub monitor: TurnMonitor,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        ClaudeCode { model: std::env::var("STAGE_MUSIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()), monitor: TurnMonitor::default() }
    }
}

impl Writer for ClaudeCode {
    fn chat(&self, system: &str, turns: &[String], cancel: &AtomicBool) -> Result<String, String> {
        self.chat_with(system, turns, cancel, None)
    }

    fn chat_with(&self, system: &str, turns: &[String], cancel: &AtomicBool, mcp: Option<&(String, String)>) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let activity = self.monitor.begin();
        let mut provider = ClaudeCodeChatProvider::new(Some(self.model.clone()));
        if let Some((config, allowed)) = mcp {
            provider = provider.with_mcp(config.clone(), allowed.clone());
        }
        let messages = turns.iter().enumerate().map(|(i, t)| ChatMessage::new(if i % 2 == 0 { ChatRole::User } else { ChatRole::Assistant }, t.clone())).collect();
        let mut input = TurnInput::new(system.to_string(), messages);
        input.tools_enabled = false;
        provider.begin_turn(&input)?;
        let mut text = String::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                provider.cancel();
                return Err("cancelled".into());
            }
            let events = provider.poll();
            if !events.is_empty() {
                activity.progress();
            }
            for event in events {
                match event {
                    ProviderEvent::Delta(d) => text.push_str(&d),
                    ProviderEvent::Done { text: all } => return Ok(if all.is_empty() { text } else { all }),
                    ProviderEvent::Error(e) => return Err(e),
                    _ => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn label(&self) -> String {
        self.model.clone()
    }
}

/// What the person asks a batch for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub prompt: String,
    /// The style markers switched on ("deep house", "melancholic", …).
    pub markers: Vec<String>,
    /// About how long each song runs.
    pub seconds: u32,
}

impl Request {
    pub fn text(&self) -> String {
        let mut out = String::new();
        if !self.prompt.trim().is_empty() {
            out.push_str(&format!("Brief: {}\n", self.prompt.trim()));
        }
        if !self.markers.is_empty() {
            out.push_str(&format!("Style: {}\n", self.markers.join(", ")));
        }
        out.push_str(&format!("Length: about {} seconds\n", self.seconds));
        out
    }
}

const SYSTEM_HEAD: &str = "You are the composer of a music generator. You write one complete, finished song as a Score (the Splash document described in the guide below): its instruments, loops, FX, mix and master chain, sections and arrangement. The person listens to several songs per batch and rates them; what they liked and disliked is distilled into the guide's \"Learned from listening\" rules, which override everything else: follow them.\n\nReply with exactly:\nTITLE: <a short title>\nIDEA: <one sentence: the form, the sound palette, the groove, the harmony and the hook you chose>\nthen the whole score in one ```splash code block, and nothing after it. Never use comment directives (//@...). The song must end (its last section has `next: @stop`).\n\nProduction rules (a song that breaks them sounds amateur):\n- Form: sections of 4 or 8 bars (a 1-bar break or fill is the only exception); a thin intro, development, a fuller peak, and an ending that resolves. Something changes every 4 bars: a layer enters or leaves, a fill, a filter or level move, a variation of the hook.\n- Transitions: mark every section boundary: a drum fill, a riser or reverse swell, a crash on the downbeat, or a 1-beat drop-out.\n- Sounds: use the named presets (`Synth{preset: @saw_lead}` …), kits and Ironfish programs as they are; do not set a node-level `cutoff:` on leads, pads, plucks or stabs (it replaces the patch's filter sweep and dulls it). Use the General MIDI SoundFont only for acoustic instruments (piano, guitars, orchestral, sax, flute), never for synth roles.\n- Top end: hats, shakers or rides play through the peak sections at amp 0.4-0.7 with accents; no lowpass below 6 kHz on leads; reverb `damp` 9000 or more; no high-shelf cuts on the lead.\n- Low end: duck the bass under the kick, or keep bass notes off the kick.\n\n";

pub fn system() -> String {
    format!("{SYSTEM_HEAD}{}", crate::context::score_guide())
}

/// One song's brief from the director: its title and the whole card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Concept {
    pub title: String,
    pub card: String,
}

/// The fields a concept card has, in order.
pub const CONCEPT_FIELDS: [&str; 12] = ["TITLE", "SUBGENRE", "TEMPO", "KEY", "PROGRESSION", "PALETTE", "FORM", "FEEL", "MOOD", "LEAD", "HOOK", "DIFFERS"];

const DIRECTOR_SYSTEM: &str = "You are the director of a music generator. A person asks for a batch of short songs; five composers will each write one song, in parallel, from a concept card you give them, and the person rates them to teach the generator their taste. Your job is VARIETY WITH INTENT: five clearly different, each a strong, idiomatic take on the brief. A batch where the songs share a key, a tempo band, a progression family, a palette or a title is a failed batch.\n\nSpread the five cards across these axes, so no two cards share more than two of them: sub-genre (use the guide's genre and era palettes), tempo (spread at least 25 bpm across the batch, unless the brief fixes a tempo), key and mode (five different tonics; mix major, minor and modal), progression family, sound palette (lead, bass, pads, drums: name the engine's presets, recipes, kits, Ironfish programs or General MIDI instruments from the guide), form, rhythm feel (straight, swung, half-time, four-on-the-floor, broken), mood, lead voice, and title (five unrelated titles, none reused from recent batches). Honour the brief, the style markers and the guide's Learned-from-listening rules (they are the person's taste); avoid the keys, progressions, titles and palettes of the recent batches unless the person liked them.\n\nReply with exactly five cards, each starting with a line `=== CONCEPT k` (k = 1..5), then one line per field, in this order: TITLE, SUBGENRE, TEMPO (a bpm), KEY (tonic and mode), PROGRESSION (chord symbols), PALETTE, FORM (sections with bar counts that fit the length), FEEL, MOOD, LEAD, HOOK (the motif's idea in a few words), DIFFERS (one line: how this card differs from the other four). Nothing else.";

pub fn director_system() -> String {
    format!("{DIRECTOR_SYSTEM}\n\n# The production guide\n\n{}", crate::context::read())
}

/// The director's message: the request and what recent batches did.
pub fn director_prompt(req: &Request, n: usize, history: &str) -> String {
    let mut out = format!("Plan a batch of {n} songs.\n\n{}", req.text());
    out.push_str(&format!("Every song is about {} seconds: plan forms that fit (at B bpm, about {}*B/240 bars of 4/4).\n", req.seconds, req.seconds));
    if history.trim().is_empty() {
        out.push_str("\nNo recent batches.\n");
    } else {
        out.push_str(&format!("\nRecent batches (title, key, tempo, palette, the person's ratings): do not repeat their titles, keys and progressions, and move away from what was disliked:\n{history}\n"));
    }
    out
}

/// The cards of a director's reply.
pub fn parse_concepts(reply: &str) -> Vec<Concept> {
    reply
        .split("=== CONCEPT")
        .skip(1)
        .filter_map(|part| {
            let body: Vec<&str> = part.lines().skip(1).map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("```")).collect();
            let title = body.iter().find_map(|l| l.strip_prefix("TITLE:")).map(|t| t.trim().to_string())?;
            Some(Concept { title, card: body.join("\n") })
        })
        .collect()
}

/// Ask the director for `n` cards (one more turn if it gave too few or
/// repeated a title).
/// What the director is told about the song library's tools, when it has
/// them.
const DIRECTOR_TOOLS: &str = "\n\n# Real songs to ground the cards in\n\nYou have two tools on a library of real songs as MIDI: `midi_lookup` finds songs by decade, genre, tag, artist or title (\"80s synthpop\", \"italo disco\", \"artist:jan hammer\"), and `midi_transcribe` reads one as compact text: its tempo, key, form, parts and instruments, and per bar its chords and each part on a sixteenth grid (detail `chords`, `drums`, `melody` or `bass`, and `bars` such as 1-16, keep it short). When the brief names an era, a style or an artist, look up two or three fitting songs, read their form, groove and harmony (chords and drums, 8-16 bars), and ground the cards in what you read: tempos, forms, drum patterns, progressions, palettes. Never copy a melody or a whole progression; a card says what it takes from a reference in its PALETTE or FEEL line. Skip the tools when the brief needs no reference.";

pub fn direct(writer: &dyn Writer, req: &Request, n: usize, history: &str, cancel: &AtomicBool) -> Result<Vec<Concept>, String> {
    let tools = crate::context::midi_tools();
    let system = if tools.is_some() { format!("{}{DIRECTOR_TOOLS}", director_system()) } else { director_system() };
    let mut turns = vec![director_prompt(req, n, history)];
    for _ in 0..2 {
        let reply = writer.chat_with(&system, &turns, cancel, tools.as_ref())?;
        let cards = parse_concepts(&reply);
        let mut titles: Vec<String> = cards.iter().map(|c| c.title.to_lowercase()).collect();
        titles.sort();
        titles.dedup();
        if cards.len() >= n && titles.len() >= n {
            return Ok(cards.into_iter().take(n).collect());
        }
        turns.push(reply);
        turns.push(format!("That gave {} cards with {} distinct titles; send exactly {n} cards, all different, in the format asked.", cards.len(), titles.len()));
    }
    Err("the director did not plan the batch".into())
}

/// The first message for song `k` of `n` of a batch.
pub fn song_prompt(req: &Request, k: usize, n: usize, recent: &str, concept: Option<&Concept>) -> String {
    let bars = |bpm: f64| (req.seconds as f64 * bpm / 240.0).round();
    let mut out = format!("Write song {} of {n} of this batch.\n\n{}\n", k + 1, req.text());
    if let Some(c) = concept {
        out.push_str(&format!("Your concept card from the director. Follow it: its title, sub-genre, tempo, key, progression, palette, form, feel, mood, lead and hook (adapt a detail only when the engine cannot do it). The other four songs follow other cards, so do not drift toward a generic version of the brief.\n{}\n\n", c.card));
    }
    out.push_str(&format!(
        "Fit the length: at a tempo of B bpm in 4/4, {} seconds is about {}·B/240 bars (e.g. {} bars at 120 bpm, {} at 128).\n",
        req.seconds, req.seconds, bars(120.0), bars(128.0)
    ));
    if n > 1 && concept.is_none() {
        out.push_str("The batch's songs are heard side by side to learn what the person likes, so make this one clearly its own: within the brief and style, choose its tempo, sound palette, groove, form and hook independently of the obvious first idea");
        out.push_str(&format!(" (this is variation {} of {n}).\n", k + 1));
    }
    // One reference excerpt when the style matches one (the request, the
    // markers and the card's sub-genre and palette).
    let style = format!("{} {} {}", req.prompt, req.markers.join(" "), concept.map_or("", |c| c.card.as_str()));
    if let Some(reference) = crate::context::reference_for(&style) {
        out.push('\n');
        out.push_str(&reference);
    }
    if !recent.trim().is_empty() {
        out.push_str(&format!("\nThe person's last verdicts (what to avoid repeating and what to keep):\n{recent}\n"));
    }
    out
}

/// TITLE, IDEA and the score of a reply.
pub fn parse_song(reply: &str) -> Option<(String, String, String)> {
    let line = |key: &str| reply.lines().find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().to_string())).unwrap_or_default();
    let mut source = None;
    let mut rest = reply;
    while let Some(at) = rest.find("```") {
        let after = &rest[at + 3..];
        let body_start = after.find('\n').map_or(after.len(), |n| n + 1);
        let Some(end) = after[body_start..].find("```") else { break };
        let body = &after[body_start..body_start + end];
        if body.contains("Score{") {
            source = Some(body.trim().to_string() + "\n");
        }
        rest = &after[body_start + end + 3..];
    }
    let source = source.or_else(|| reply.find("use mod.score").map(|at| reply[at..].trim().to_string() + "\n"))?;
    Some((line("TITLE:"), line("IDEA:"), source))
}

/// A song as written and rendered.
pub struct Written {
    pub title: String,
    pub idea: String,
    pub source: String,
    pub song: Song,
    /// Model turns it took (1 = first try).
    pub turns: usize,
    /// The conversation so far (a revision continues it).
    pub history: Vec<String>,
}

/// How a song's writing goes, for the list.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    Writing { turn: usize },
    /// A producer's revision of a song that played.
    Revising,
    Rendering,
}

/// Write song `k` of `n`, render it (mix, stems, dry lead) and repair what
/// does not load or whose loops fail, up to `max_turns` model turns.
pub fn write_song(writer: &dyn Writer, system: &str, req: &Request, k: usize, n: usize, recent: &str, concept: Option<&Concept>, cancel: &AtomicBool, progress: &dyn Fn(Progress)) -> Result<Written, String> {
    write_from(writer, system, req, vec![song_prompt(req, k, n, recent, concept)], cancel, progress)
}

/// Continue a conversation whose last message asks for a whole song: up to
/// three turns until one loads and plays.
pub fn write_from(writer: &dyn Writer, system: &str, req: &Request, mut turns: Vec<String>, cancel: &AtomicBool, progress: &dyn Fn(Progress)) -> Result<Written, String> {
    const MAX_TURNS: usize = 3;
    let mut last_error = String::new();
    for turn in 1..=MAX_TURNS {
        progress(Progress::Writing { turn });
        let reply = writer.chat(system, &turns, cancel)?;
        turns.push(reply.clone());
        let problem = match parse_song(&reply) {
            None => "Your reply had no ```splash block with a whole score (`use mod.score.*` … `Score{…}`). Send the whole song again in the format asked.".to_string(),
            Some((title, idea, source)) => {
                progress(Progress::Rendering);
                let cap = req.seconds as f64 * 1.6 + 12.0;
                let length = makepad_score_live::model::load(&source).ok().map(|l| makepad_score_live::render::natural_length(&l.doc));
                let opts = RenderOpts { seconds: length.map(|s| s.min(cap)), ..RenderOpts::default() };
                match render_song(&source, &opts, true) {
                    Ok(song) if song.mix.errors.is_empty() && song.mix.left.iter().any(|v| v.abs() > 1e-3) => {
                        return Ok(Written { title, idea, source, song, turns: turn, history: turns });
                    }
                    Ok(song) if !song.mix.errors.is_empty() => format!(
                        "The score loads but some loops fail while playing (they render silence):\n{}\nFix them and send the whole song again in the same format.",
                        song.mix.errors.iter().take(12).map(|d| format!("{d:?}")).collect::<Vec<_>>().join("\n")
                    ),
                    Ok(_) => "The score renders silence. Fix it and send the whole song again in the same format.".to_string(),
                    Err(e) => format!("The score does not load:\n{e}\nFix it and send the whole song again in the same format."),
                }
            }
        };
        last_error = problem.clone();
        turns.push(problem);
    }
    Err(format!("no playable song after {MAX_TURNS} turns: {}", last_error.lines().next().unwrap_or("")))
}

/// The producer's notes on a rendered song: the scorecard's weak lines and
/// what the arrangement lacks, as a request for a revised whole song; None
/// when there is nothing worth a revision.
pub fn critique(song: &Song, card: &crate::card::Card) -> Option<String> {
    let mut notes: Vec<String> = card.weakest(4).iter().filter(|m| m.score < 75.0).map(|m| format!("- {} ({:.0}/100): {}. Fix: {}", m.key, m.score, m.value, m.fix)).collect();
    let bpb = song.mix.beats_per_bar.max(1.0);
    let sections = &song.mix.sections;
    let short: Vec<String> = sections.windows(2).filter(|w| (w[1].0 - w[0].0) / bpb < 4.0).map(|w| w[0].1.clone()).collect();
    if !short.is_empty() {
        notes.push(format!("- sections shorter than 4 bars ({}): build sections of 4 or 8 bars; a 1-bar break or fill is the only exception", short.join(", ")));
    }
    if sections.len() < 3 {
        notes.push("- fewer than three sections: give it an intro, a development and an ending".into());
    }
    (!notes.is_empty()).then(|| format!(
        "A producer listened to the render. Revise the song to fix these points, keeping its concept, title, key, tempo and hook:\n{}\nSend the whole revised song in the same format.",
        notes.join("\n")
    ))
}

/// A rated song as the learner reads it.
pub struct Rated<'a> {
    pub label: String,
    pub title: &'a str,
    pub idea: &'a str,
    pub source: &'a str,
    /// (aspect, liked) for the rated aspects only.
    pub ratings: Vec<(&'static str, bool)>,
    pub feedback: &'a str,
    pub card: Option<String>,
    /// The director's concept card it was written from.
    pub concept: &'a str,
}

const LEARN_SYSTEM: &str = "You maintain the \"Learned from listening\" section of MUSIC-AGENTS.md, the production guide a music AI reads before it writes a song as a Score (a Splash document). The person listened to songs that AI wrote and rated aspects of each (up = liked, down = disliked; an aspect without a rating says nothing and must be ignored) and sometimes wrote a comment. You get each rated song's concept card (the director's plan: sub-genre, tempo, key, progression, palette, form, feel, mood), its Score source, its ratings, the comment and an objective scorecard.\n\nWork out WHY: compare the songs whose aspect was liked with those whose same aspect was disliked, and find what the Score sources concretely did differently (tempo, instruments and presets, drum patterns and velocities, bass rhythm and register, chord voicings and progressions, hook shape and repetition, section lengths and layering, FX and levels). Learn at two levels: the CONCEPT (which sub-genres, tempos, moods, palettes, forms the person likes or rejects, e.g. `do: favour darkwave and minimal wave for 80s requests; their cold drum machines were liked twice`) and the CRAFT inside a song. Then rewrite the section's rule list:\n- one line per rule: `YYYY-MM-DD do: <rule, concrete, with numbers or Score terms where possible>; <short why>` or `YYYY-MM-DD don't: ...` (use the date given for new rules, keep old rules' dates);\n- never paste the person's raw words; write the cause behind the reaction as a rule that applies to the next song;\n- merge a new rule with an old one on the same point; delete old rules the new verdicts contradict; keep the rest;\n- each rule at most about 35 words: a general rule for the next song, not a description of one song's exact notes or bar plan (one example value is fine);
- a rule only from evidence: an aspect rated on at least one song; prefer patterns seen on several songs;\n- at most 30 lines.\n\nReply with the whole new rule list in one ```md code block (only the `- ` lines), then a line `CHANGES:` and one short paragraph on what you added, merged or removed and why.";

/// The learner's message.
pub fn learn_prompt(rated: &[Rated], rules: &[String], today: &str) -> String {
    let mut out = format!("Today is {today}.\n\nThe current rules:\n");
    if rules.is_empty() {
        out.push_str("(none yet)\n");
    }
    for r in rules {
        out.push_str(&format!("- {r}\n"));
    }
    for s in rated {
        out.push_str(&format!("\n## {} \"{}\"\nIdea: {}\n", s.label, s.title, s.idea));
        if !s.concept.trim().is_empty() {
            out.push_str(&format!("Concept card:\n{}\n", s.concept.trim()));
        }
        let ups: Vec<&str> = s.ratings.iter().filter(|(_, up)| *up).map(|(a, _)| *a).collect();
        let downs: Vec<&str> = s.ratings.iter().filter(|(_, up)| !*up).map(|(a, _)| *a).collect();
        out.push_str(&format!("Liked: {}\nDisliked: {}\n", if ups.is_empty() { "-".into() } else { ups.join(", ") }, if downs.is_empty() { "-".into() } else { downs.join(", ") }));
        if !s.feedback.trim().is_empty() {
            out.push_str(&format!("Comment: {}\n", s.feedback.trim()));
        }
        if let Some(card) = &s.card {
            out.push_str(card);
        }
        out.push_str(&format!("```splash\n{}\n```\n", s.source.trim()));
    }
    out
}

/// The rules and the CHANGES paragraph of a learner's reply.
pub fn parse_rules(reply: &str) -> Option<(Vec<String>, String)> {
    let start = reply.find("```")?;
    let after = &reply[start + 3..];
    let body_start = after.find('\n')? + 1;
    let end = after[body_start..].find("```")?;
    let rules: Vec<String> = after[body_start..body_start + end].lines().filter_map(|l| l.trim().strip_prefix("- ")).map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && l != "(none yet)").collect();
    let changes = reply.find("CHANGES:").map(|at| reply[at + 8..].trim().to_string()).unwrap_or_default();
    Some((rules, changes))
}

/// Ask the learner for the new rules.
pub fn learn(writer: &dyn Writer, rated: &[Rated], rules: &[String], today: &str, cancel: &AtomicBool) -> Result<(Vec<String>, String), String> {
    let reply = writer.chat(LEARN_SYSTEM, &[learn_prompt(rated, rules, today)], cancel)?;
    let (mut new, changes) = parse_rules(&reply).ok_or_else(|| "the learner's reply had no rule list".to_string())?;
    new.truncate(30);
    Ok((new, changes))
}

/// A writer for tests and offline runs: replies from a closure.
pub struct Scripted<F: Fn(&str, &[String]) -> Result<String, String> + Send + Sync>(pub F);

impl<F: Fn(&str, &[String]) -> Result<String, String> + Send + Sync> Writer for Scripted<F> {
    fn chat(&self, system: &str, turns: &[String], _cancel: &AtomicBool) -> Result<String, String> {
        (self.0)(system, turns)
    }

    fn label(&self) -> String {
        "scripted".into()
    }
}

pub type SharedWriter = Arc<dyn Writer>;

#[cfg(test)]
mod monitor_tests {
    use super::*;

    fn backdate(activity: &TurnActivity, by: Duration) {
        *activity.last_progress.lock().unwrap() = Instant::now().checked_sub(by).unwrap();
    }

    #[test]
    fn a_fresh_turn_is_responding() {
        let monitor = TurnMonitor::default();
        let _turn = monitor.begin();
        assert!(!monitor.not_responding());
    }

    #[test]
    fn a_silent_turn_is_not_responding_until_progress_arrives() {
        let monitor = TurnMonitor::default();
        let turn = monitor.begin();
        backdate(&turn, Duration::from_secs(61));
        assert!(monitor.not_responding());
        turn.progress();
        assert!(!monitor.not_responding());
    }

    #[test]
    fn a_finished_turn_no_longer_counts() {
        let monitor = TurnMonitor::default();
        let turn = monitor.begin();
        backdate(&turn, Duration::from_secs(61));
        drop(turn);
        assert!(!monitor.not_responding());
    }

    #[test]
    fn a_cancelled_turn_never_starts() {
        let writer = ClaudeCode::default();
        let cancel = AtomicBool::new(true);
        assert_eq!(writer.chat_with("s", &["hi".into()], &cancel, None), Err("cancelled".to_string()));
        assert!(!writer.monitor.not_responding());
    }
}
