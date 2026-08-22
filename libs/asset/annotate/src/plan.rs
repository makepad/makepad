//! Fold a parsed [`Record`] into the annotation record the store will store.
//!
//! The store's annotation route is a whole-record PUT, so every run rewrites
//! the complete record. This module decides what changes and what is carried
//! through, and it is where the replace semantics live: the fields the pass
//! owns are recomputed from scratch, never merged with what was there before.

use crate::parse::Record;

/// Every tag the pass owns starts with this. The prefix IS the ownership
/// boundary: a re-run drops all of them and writes the new set, so no facet
/// from a previous model or prompt can survive.
pub const VLM_PREFIX: &str = "vlm-";

/// Store limits (mirrored from the server's search module) the plan respects
/// so a publish is never refused for shape.
const MAX_LABELS: usize = 24;
const MAX_DESCRIPTION_BYTES: usize = 4096;

/// Who produced an annotation, recorded as tags so 3.5-origin and 3.8-origin
/// rows are exactly identifiable in SQL.
#[derive(Clone, Debug)]
pub struct Annotator {
    pub version: u32,
    /// Short model slug, label-safe, e.g. `qwen35-9b`.
    pub model: String,
}

impl Annotator {
    pub fn version_tag(&self) -> String {
        format!("{VLM_PREFIX}v{}", self.version)
    }
    pub fn model_tag(&self) -> String {
        format!("{VLM_PREFIX}m-{}", self.model)
    }
}

/// The annotation record as it stands in the store today.
#[derive(Clone, Debug, Default)]
pub struct BaseAnnotation {
    pub title: String,
    pub description: String,
    pub kind: Option<String>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    pub prompt: String,
    pub provenance: String,
    pub private: bool,
}

/// The record to PUT back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub title: String,
    pub description: String,
    pub kind: Option<String>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    pub prompt: String,
    pub provenance: String,
    pub private: bool,
}

/// True when this asset still needs annotating at `annotator`'s version.
/// Idempotency is one tag lookup: bumping the version invalidates every asset
/// at once, which is exactly how a prompt revision or a model swap re-runs.
pub fn needs_annotation(tags: &[String], annotator: &Annotator) -> bool {
    let want = annotator.version_tag();
    !tags.iter().any(|t| *t == want)
}

/// Tags carried through from the previous record: everything the pass does
/// not own.
fn carried_tags(tags: &[String]) -> Vec<String> {
    tags.iter().filter(|t| !t.starts_with(VLM_PREFIX)).cloned().collect()
}

/// The facets this run publishes, in priority order — if the label budget is
/// tight the tail is what gets dropped, so identity and category come first.
fn facet_tags(rec: &Record, annotator: &Annotator) -> Vec<String> {
    let mut out = vec![annotator.version_tag(), annotator.model_tag()];
    if let Some(c) = &rec.cat {
        out.push(format!("{VLM_PREFIX}cat-{c}"));
    }
    if let Some(r) = &rec.role {
        out.push(format!("{VLM_PREFIX}role-{r}"));
    }
    // "none" is the absence of connectivity, not a facet worth a row: nothing
    // ever queries for it and it would sit on most of the catalog.
    if let Some(c) = rec.conn.as_deref().filter(|c| *c != "none") {
        out.push(format!("{VLM_PREFIX}conn-{c}"));
    }
    if let Some(s) = &rec.size {
        out.push(format!("{VLM_PREFIX}size-{s}"));
    }
    // Person facets (v5): the retrieval keys of "the old guy", "the cop",
    // "the girl with the ponytail" — identity words first, then features,
    // then clothing colours. "clean" and "average" are absences like conn
    // "none": nothing queries them and they would sit on most characters.
    if let Some(a) = &rec.age {
        out.push(format!("{VLM_PREFIX}age-{a}"));
    }
    if let Some(j) = &rec.job {
        out.push(format!("{VLM_PREFIX}job-{j}"));
    }
    if let Some(b) = rec.build.as_deref().filter(|b| *b != "average") {
        out.push(format!("{VLM_PREFIX}build-{b}"));
    }
    for f in rec.face.iter().filter(|f| f.as_str() != "clean") {
        out.push(format!("{VLM_PREFIX}face-{f}"));
    }
    for h in &rec.hair {
        out.push(format!("{VLM_PREFIX}hair-{h}"));
    }
    for c in &rec.colors {
        out.push(format!("{VLM_PREFIX}col-{c}"));
    }
    for s in &rec.style {
        out.push(format!("{VLM_PREFIX}sty-{s}"));
    }
    out
}

/// The one dense line that lands in `description`.
///
/// Every segment is optional and empty ones are dropped, so a thin record
/// yields a short line rather than a line full of placeholders. The target is
/// roughly 20 tokens: this string is what a 30-row SQL result multiplies by
/// 30 inside an 8k context.
pub fn construction_line(rec: &Record) -> String {
    let mut segs: Vec<String> = Vec::new();
    if !rec.what.is_empty() {
        segs.push(rec.what.clone());
    }
    // role and connectivity read as one phrase: "tile, corner" / "standalone"
    let mut shape = Vec::new();
    if let Some(r) = &rec.role {
        shape.push(r.clone());
    }
    if let Some(c) = &rec.conn {
        // role "corner" + conn "corner" is one fact, not two words
        if c != "none" && Some(c) != rec.role.as_ref() {
            shape.push(c.clone());
        }
    }
    if !shape.is_empty() {
        segs.push(shape.join(" "));
    }
    if let Some(s) = &rec.size {
        segs.push(s.clone());
    }
    if !rec.colors.is_empty() {
        segs.push(rec.colors.join("/"));
    }
    if !rec.desc.is_empty() {
        segs.push(rec.desc.clone());
    }
    // Last, and marked, because it answers a different question from
    // everything before it: not "what is this" but "what will the player
    // see". A character is chosen from the front and then looked at from
    // behind for the rest of the session.
    if !rec.back.is_empty() {
        segs.push(format!("from behind: {}", rec.back));
    }
    let mut line = segs.join("; ");
    if line.len() > MAX_DESCRIPTION_BYTES {
        line.truncate(MAX_DESCRIPTION_BYTES);
        while !line.is_char_boundary(line.len()) {
            line.pop();
        }
    }
    line
}

/// Build the record to PUT.
///
/// Owned and recomputed: `description`, and the `vlm-` prefixed tags.
/// Carried through untouched: everything else, including the asset's own
/// generator/backend/model provenance, its categories, and its non-prefixed
/// tags.
pub fn plan_upload(base: &BaseAnnotation, rec: &Record, annotator: &Annotator) -> Upload {
    let mut tags = carried_tags(&base.tags);
    for t in facet_tags(rec, annotator) {
        if tags.len() >= MAX_LABELS {
            break;
        }
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    tags.sort();
    tags.dedup();
    Upload {
        title: base.title.clone(),
        description: construction_line(rec),
        kind: base.kind.clone(),
        categories: base.categories.clone(),
        tags,
        creator: base.creator.clone(),
        generator: base.generator.clone(),
        backend: base.backend.clone(),
        model: base.model.clone(),
        prompt: base.prompt.clone(),
        provenance: base.provenance.clone(),
        private: base.private,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_record;

    fn base() -> BaseAnnotation {
        BaseAnnotation {
            title: "cart".into(),
            description: "Kenney fantasy-town-kit · kenney/fantasy-town-kit/cart · CC-BY-4.0".into(),
            kind: Some("mesh".into()),
            categories: vec!["kenney".into(), "fantasy-town-kit".into()],
            tags: vec![
                "cc-by-4-0".into(),
                "fantasy-town-kit".into(),
                "kenney".into(),
                "mesh".into(),
            ],
            creator: "Kenney (kenney.nl)".into(),
            generator: "pack_import".into(),
            backend: "asset-ui".into(),
            model: "fantasy-town-kit".into(),
            prompt: String::new(),
            provenance: "Kenney (kenney.nl) · license CC-BY-4.0".into(),
            private: false,
        }
    }

    fn cart_record() -> Record {
        parse_record(
            "what: wooden cart\ncat: vehicle\nrole: standalone\nconn: none\n\
             size: 1x2\ncolors: brown, grey\nstyle: low-poly\n\
             desc: open cart with one large spoked wheel",
        )
    }

    fn v(n: u32, model: &str) -> Annotator {
        Annotator { version: n, model: model.into() }
    }

    #[test]
    fn carries_everything_it_does_not_own() {
        let b = base();
        let up = plan_upload(&b, &cart_record(), &v(1, "qwen35-9b"));
        assert_eq!(up.title, b.title);
        assert_eq!(up.categories, b.categories);
        assert_eq!(up.creator, b.creator);
        assert_eq!(up.generator, b.generator);
        assert_eq!(up.backend, b.backend);
        assert_eq!(up.model, b.model);
        assert_eq!(up.provenance, b.provenance);
        assert_eq!(up.prompt, b.prompt);
        assert_eq!(up.kind, b.kind);
        // the import's own tags survive
        for t in ["cc-by-4-0", "fantasy-town-kit", "kenney", "mesh"] {
            assert!(up.tags.iter().any(|x| x == t), "lost tag {t}: {:?}", up.tags);
        }
    }

    /// v5, the query-twice fix: "the old guy" hits `vlm-age-old` in
    /// search_labels, never a `LIKE '%old%'` substring.
    #[test]
    fn person_facets_publish_as_tags() {
        let rec = parse_record(
            "what: old bald man\ncat: character\nrole: standalone\nconn: none\n\
             size: tall\nage: old\nbuild: average\nhair: bald\n\
             face: beard, clean\njob: farmer\ncolors: orange, brown\n\
             style: low-poly\ndesc: old bald man with a full grey beard in an orange shirt",
        );
        assert_eq!(rec.age.as_deref(), Some("old"));
        assert_eq!(rec.hair, vec!["bald"]);
        assert_eq!(rec.face, vec!["beard", "clean"]);
        assert_eq!(rec.job.as_deref(), Some("farmer"));
        let up = plan_upload(&base(), &rec, &v(5, "qwen38-27b"));
        for t in ["vlm-age-old", "vlm-face-beard", "vlm-hair-bald", "vlm-job-farmer"] {
            assert!(up.tags.iter().any(|x| x == t), "missing {t}: {:?}", up.tags);
        }
        // Absences never become facets: "clean" faces and "average" builds
        // would sit on most of the cast (the conn-"none" rule).
        assert!(!up.tags.iter().any(|x| x == "vlm-face-clean"));
        assert!(!up.tags.iter().any(|x| x.starts_with("vlm-build-")));
        // A hair colour rides beside the shape word.
        let rec2 = parse_record("what: old woman\ncat: character\nhair: long grey\nage: old");
        assert_eq!(rec2.hair, vec!["long", "grey"]);
        // Job words stay contained: junk and empty words never coin a facet.
        let junk = parse_record("what: man\ncat: character\njob: person");
        assert_eq!(junk.job, None);
        let multi = parse_record("what: man\ncat: character\njob: Police Officer!");
        assert_eq!(multi.job.as_deref(), Some("police"));
    }

    #[test]
    fn publishes_the_facets() {
        let up = plan_upload(&base(), &cart_record(), &v(1, "qwen35-9b"));
        for t in [
            "vlm-v1",
            "vlm-m-qwen35-9b",
            "vlm-cat-vehicle",
            "vlm-role-standalone",
            "vlm-size-1x2",
            "vlm-col-brown",
            "vlm-col-grey",
            "vlm-sty-low-poly",
        ] {
            assert!(up.tags.iter().any(|x| x == t), "missing {t}: {:?}", up.tags);
        }
        // conn: none is not a facet worth a row
        assert!(!up.tags.iter().any(|t| t.starts_with("vlm-conn-")));
    }

    /// The load-bearing test: a second run at a new version with a different
    /// model must leave NO trace of the first, on any field the pass owns.
    #[test]
    fn rerun_hard_replaces_every_owned_field() {
        let first = plan_upload(&base(), &cart_record(), &v(1, "qwen35-9b"));

        // feed the first run's output back in as the current state
        let mut current = base();
        current.description = first.description.clone();
        current.tags = first.tags.clone();

        let second_rec = parse_record(
            "what: hay wagon\ncat: prop\nrole: prop\nconn: none\n\
             size: 2x2\ncolors: yellow\nstyle: rustic\ndesc: wagon loaded with hay bales",
        );
        let second = plan_upload(&current, &second_rec, &v(2, "qwen38-27b"));

        // description replaced outright
        assert_eq!(second.description, construction_line(&second_rec));
        assert!(!second.description.contains("wooden cart"));
        // not one v1 facet survives
        for stale in [
            "vlm-v1",
            "vlm-m-qwen35-9b",
            "vlm-cat-vehicle",
            "vlm-role-standalone",
            "vlm-size-1x2",
            "vlm-col-brown",
            "vlm-col-grey",
            "vlm-sty-low-poly",
        ] {
            assert!(!second.tags.iter().any(|t| t == stale), "stale {stale}: {:?}", second.tags);
        }
        assert!(second.tags.iter().any(|t| t == "vlm-v2"));
        assert!(second.tags.iter().any(|t| t == "vlm-m-qwen38-27b"));
        // and the import's tags are still there after two rounds
        assert!(second.tags.iter().any(|t| t == "fantasy-town-kit"));
        assert!(second.tags.iter().any(|t| t == "kenney"));
    }

    #[test]
    fn idempotent_skip_is_version_scoped() {
        let up = plan_upload(&base(), &cart_record(), &v(1, "qwen35-9b"));
        assert!(!needs_annotation(&up.tags, &v(1, "qwen35-9b")));
        // a bumped version re-annotates everything
        assert!(needs_annotation(&up.tags, &v(2, "qwen38-27b")));
        // never annotated at all
        assert!(needs_annotation(&base().tags, &v(1, "qwen35-9b")));
    }

    #[test]
    fn construction_line_stays_dense() {
        let line = construction_line(&cart_record());
        assert_eq!(
            line,
            "wooden cart; standalone; 1x2; brown/grey; \
             open cart with one large spoked wheel"
        );
        // a rough token proxy: this is what 30 rows multiply in an 8k context
        assert!(line.split_whitespace().count() <= 20, "{line}");
    }

    /// THE LINE THIS PASS EXISTS FOR.
    ///
    /// A character is CHOSEN from a front portrait and then looked at from
    /// behind, small and in motion, for the rest of the session. Every
    /// description before v6 carried only the portrait, which is how
    /// kenney/mini-dungeon/character-human could be honestly described as
    /// "old man … full brown beard, brown hat … holding a sword" while the
    /// player looking at its back called it a monkey. Both were true.
    #[test]
    fn a_character_line_says_how_it_reads_from_behind() {
        let rec = parse_record(
            "what: old bearded man\n\
             cat: character\n\
             role: standalone\n\
             size: tall\n\
             age: old\n\
             face: beard hat\n\
             colors: brown, grey\n\
             desc: Old man with brown beard, brown hat, brown tunic, holding a sword.\n\
             back: plain brown box head, no face, dark tunic, hat barely visible\n",
        );
        assert_eq!(
            rec.back,
            "plain brown box head, no face, dark tunic, hat barely visible"
        );
        let line = construction_line(&rec);
        assert!(
            line.contains("from behind: plain brown box head"),
            "the rear read must reach the description: {line}"
        );
        assert!(
            line.find("from behind:") > line.find("Old man"),
            "it comes LAST — it answers a different question from the portrait: {line}"
        );

        // Absent (a kit piece, or a model that could not tell) adds nothing
        // rather than a placeholder.
        let quiet = parse_record("what: rock\ncat: rock");
        assert!(!construction_line(&quiet).contains("from behind"));
    }

    #[test]
    fn thin_records_yield_short_lines_not_placeholders() {
        let line = construction_line(&parse_record("what: rock\ncat: rock"));
        assert_eq!(line, "rock");
        let road = parse_record("what: road tile\ncat: road\nrole: tile\nconn: corner\ncolors: grey");
        assert_eq!(construction_line(&road), "road tile; tile corner; grey");
    }

    #[test]
    fn label_budget_is_never_exceeded() {
        let mut b = base();
        // an asset that already carries a lot of import tags
        b.tags = (0..20).map(|i| format!("import-tag-{i}")).collect();
        let up = plan_upload(&b, &cart_record(), &v(1, "qwen35-9b"));
        assert!(up.tags.len() <= MAX_LABELS, "{}", up.tags.len());
        // identity still made the cut: it is first in priority order
        assert!(up.tags.iter().any(|t| t == "vlm-v1"));
    }
}
