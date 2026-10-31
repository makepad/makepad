//! Settings that survive a run: the theme, how many days back "new" reaches,
//! the last story, the two switches and the folders the person closed.
//!
//! One tab-separated `key<TAB>value` line per setting in a file under the
//! user's config directory, or wherever `MAKEPAD_STORYBOOK_SETTINGS` points
//! (a run that must leave the person's own file alone points it at a
//! scratch one). Missing or unreadable means defaults; every write rewrites
//! the whole file. The tests never touch the store: they check the file's
//! text and the key map through the helpers the store calls.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

static STORE: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

pub const THEME: &str = "theme";
/// How many days back the NEW marker reaches, a whole number of at least
/// one. Once the person has set it, the baseline date is today less that
/// number and moves with the calendar. Until then the reach is the days
/// since `registry::DEFAULT_BASELINE`, so the default marker stays on that
/// date however long the catalogue is left alone.
pub const NEW_DAYS: &str = "new_days";
pub const LAST_STORY: &str = "last_story";
pub const NEW_ONLY: &str = "new_only";
pub const SEARCH_FILTER: &str = "search_filter";
/// The navigator folders the person closed, as [`format_folded`] writes them.
pub const FOLDED: &str = "folded";
/// The stories the person starred (keys, as [`format_folded`] writes them),
/// and whether the navigator lists only those.
pub const STARRED: &str = "starred";
pub const STARRED_ONLY: &str = "starred_only";

pub fn path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MAKEPAD_STORYBOOK_SETTINGS") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(home).join(".config").join("makepad-storybook").join("settings.txt"))
}

/// The key map in a settings file's text: one `key<TAB>value` per line.
/// A line without a tab is skipped rather than guessed at.
fn parse_settings(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('\t') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

/// The whole file's text for a key map, as [`parse_settings`] reads it.
fn format_settings(map: &BTreeMap<String, String>) -> String {
    map.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect()
}

fn with<R>(f: impl FnOnce(&mut BTreeMap<String, String>) -> R) -> R {
    let mut guard = STORE.lock().unwrap();
    if guard.is_none() {
        let text = path().and_then(|p| std::fs::read_to_string(p).ok());
        *guard = Some(text.as_deref().map(parse_settings).unwrap_or_default());
    }
    f(guard.as_mut().unwrap())
}

pub fn get(key: &str) -> Option<String> {
    with(|map| map.get(key).cloned())
}

pub fn set(key: &str, value: &str) {
    with(|map| {
        map.insert(key.to_string(), value.to_string());
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&p, format_settings(map));
        }
    })
}

/// Today's date in ISO form, from the system clock. Civil time rather
/// than local: the clock hands over seconds since the epoch and nothing
/// else without a platform library, and a marker that reaches back whole
/// days is not sharpened by knowing the time zone.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso_from_days(secs.div_euclid(86_400))
}

/// Days since 1970-01-01 of an ISO date, or None when it is not one.
/// Howard Hinnant's `days_from_civil`: the year is taken to start in
/// March, so the leap day is the last day of the year and every month's
/// length is a fixed arithmetic of its number.
pub fn days_from_iso(date: &str) -> Option<i64> {
    if !crate::registry::is_iso_date(date) {
        return None;
    }
    let y: i64 = date[0..4].parse().ok()?;
    let m: i64 = date[5..7].parse().ok()?;
    let d: i64 = date[8..10].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The ISO date `days` days after 1970-01-01; Hinnant's `civil_from_days`,
/// the inverse of [`days_from_iso`].
pub fn iso_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// The date `days` days before `date`. A date that will not parse is
/// handed back as it came: there is no better answer to give.
pub fn days_before(date: &str, days: u32) -> String {
    match days_from_iso(date) {
        Some(z) => iso_from_days(z - days as i64),
        None => date.to_string(),
    }
}

/// How many days `to` lies after `from`; negative when it lies before.
pub fn days_between(from: &str, to: &str) -> Option<i64> {
    Some(days_from_iso(to)? - days_from_iso(from)?)
}

/// A stored reach: a whole number of days, at least one. Anything else
/// is no reach at all rather than a reach of nothing.
pub fn parse_new_days(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|n| *n >= 1)
}

/// The reach that lands on the day the catalogue was started, seen from
/// `today`: what "new within N days" means before the person sets N.
/// Never less than a day, whatever the clock says.
pub fn default_new_days_from(today: &str) -> u32 {
    days_between(crate::registry::DEFAULT_BASELINE, today)
        .unwrap_or(1)
        .clamp(1, u32::MAX as i64) as u32
}

pub fn default_new_days() -> u32 {
    default_new_days_from(&today())
}

/// The key an earlier build kept the marker's fixed date under, before the
/// reach replaced it. Read once to carry that date over; never written.
const OLD_BASELINE: &str = "baseline";

/// The reach that lands on a fixed baseline date from an earlier build,
/// seen from `today`: the days between them, at least one. None when
/// either will not parse, so the default applies instead.
pub fn new_days_from_old_baseline(baseline: &str, today: &str) -> Option<u32> {
    let days = days_between(baseline, today)?;
    Some(days.clamp(1, u32::MAX as i64) as u32)
}

/// How many days back "new" reaches in a settings key map, seen from
/// `today`, and whether that number came from an old `baseline` line and
/// so has to be stored. The store asks here, and so do the tests.
///
/// A stored number wins. A map from before the reach replaced the fixed
/// date has only that date: the reach that lands on it today is the
/// answer, so the marker goes on meaning what it meant. With neither, the
/// default applies and nothing needs storing.
fn resolve_new_days(map: &BTreeMap<String, String>, today: &str) -> (u32, bool) {
    if let Some(n) = map.get(NEW_DAYS).and_then(|s| parse_new_days(s)) {
        return (n, false);
    }
    match map.get(OLD_BASELINE).and_then(|b| new_days_from_old_baseline(b, today)) {
        Some(n) => (n, true),
        None => (default_new_days_from(today), false),
    }
}

/// How many days back "new" reaches: the stored number, or the default.
///
/// A settings file from before the reach replaced the fixed `baseline`
/// date is migrated the first time it is read: the number
/// [`resolve_new_days`] carries over is stored, and from then on the
/// number is what counts. The date line is left where it is; nothing
/// reads it again.
pub fn new_days() -> u32 {
    let today = today();
    let (n, migrated) = with(|map| resolve_new_days(map, &today));
    if migrated {
        set(NEW_DAYS, &n.to_string());
    }
    n
}

/// The date a widget has to have arrived on or after to count as new:
/// today, less the reach. `registry::is_new` compares against it.
pub fn baseline() -> String {
    days_before(&today(), new_days())
}

/// The closed folders, read back from one line. A folder is named by its
/// slug path (`buttons`, or `data-display/datagrid` for a component), so
/// a comma can join them: a slug has no comma, and the store's own line
/// has no tab or newline to trip over.
pub fn parse_folded(text: &str) -> BTreeSet<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The closed folders as the one line [`parse_folded`] reads.
pub fn format_folded(folded: &BTreeSet<String>) -> String {
    folded.iter().map(String::as_str).collect::<Vec<_>>().join(",")
}

/// The closed folders a settings file holds, as folders of the tree drawn
/// now. `moved` is the registry's table of old keys and the keys they went
/// to, and `live` every folder the tree draws.
///
/// A folder the tree still draws stays closed. A component folder whose
/// pages moved is closed where they went: its old path is the first two
/// segments of the old keys under it, and the new one the first two
/// segments of the keys they lead to. It is forgotten when those pages
/// went to more than one folder, or to a component with one page, which
/// has no folder to close; so is any other path the tree does not draw.
pub fn carry_folded(
    folded: &BTreeSet<String>,
    moved: &[(&str, &str)],
    live: &BTreeSet<String>,
) -> BTreeSet<String> {
    fn component_path(key: &str) -> &str {
        key.rsplit_once('/').map_or(key, |(path, _)| path)
    }
    folded
        .iter()
        .filter_map(|path| {
            if live.contains(path) {
                return Some(path.clone());
            }
            let mut went = moved
                .iter()
                .filter(|(old, _)| component_path(old) == path)
                .map(|(_, new)| component_path(new));
            let first = went.next()?;
            (went.all(|other| other == first) && live.contains(first)).then(|| first.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_dates_round_trip() {
        for (date, days) in [
            ("1970-01-01", 0),
            ("1970-01-02", 1),
            ("1969-12-31", -1),
            ("2000-03-01", 11_017),
            ("2024-02-29", 19_782),
            ("2026-09-05", 20_701),
            ("2026-09-12", 20_708),
        ] {
            assert_eq!(days_from_iso(date), Some(days), "{date}");
            assert_eq!(iso_from_days(days), date, "{days}");
        }
        assert_eq!(days_from_iso("2026-9-12"), None);
        assert_eq!(days_from_iso("2026-13-01"), None);
        assert_eq!(days_from_iso("not a date"), None);
    }

    #[test]
    fn a_reach_counts_back_across_month_and_leap_day() {
        assert_eq!(days_before("2026-09-12", 7), "2026-09-05");
        assert_eq!(days_before("2026-09-12", 30), "2026-08-13");
        assert_eq!(days_before("2026-03-01", 1), "2026-02-28");
        assert_eq!(days_before("2024-03-01", 1), "2024-02-29");
        assert_eq!(days_before("2026-01-01", 1), "2025-12-31");
        assert_eq!(days_before("2026-09-12", 0), "2026-09-12");
        assert_eq!(days_before("junk", 3), "junk");
        assert_eq!(days_between("2026-09-05", "2026-09-12"), Some(7));
        assert_eq!(days_between("2026-09-12", "2026-09-05"), Some(-7));
        assert_eq!(days_between("junk", "2026-09-05"), None);
    }

    #[test]
    fn the_default_reach_lands_on_the_first_day_and_never_under_one() {
        assert_eq!(default_new_days_from("2026-09-12"), 7);
        assert_eq!(default_new_days_from("2026-10-05"), 30);
        assert_eq!(default_new_days_from("2026-09-05"), 1);
        assert_eq!(default_new_days_from("2020-01-01"), 1);
        assert_eq!(default_new_days_from("junk"), 1);
        assert_eq!(
            days_before("2026-09-12", default_new_days_from("2026-09-12")),
            crate::registry::DEFAULT_BASELINE
        );
    }

    #[test]
    fn a_stored_reach_is_a_whole_day_or_nothing() {
        assert_eq!(parse_new_days("30"), Some(30));
        assert_eq!(parse_new_days(" 7 "), Some(7));
        assert_eq!(parse_new_days("1"), Some(1));
        assert_eq!(parse_new_days("0"), None);
        assert_eq!(parse_new_days("-3"), None);
        assert_eq!(parse_new_days("7.5"), None);
        assert_eq!(parse_new_days(""), None);
    }

    #[test]
    fn an_old_baseline_line_migrates_to_the_reach_that_lands_on_it() {
        assert_eq!(new_days_from_old_baseline("2026-08-01", "2026-09-13"), Some(43));
        assert_eq!(days_before("2026-09-13", 43), "2026-08-01");
        assert_eq!(new_days_from_old_baseline("2026-09-13", "2026-09-13"), Some(1));
        assert_eq!(new_days_from_old_baseline("2026-12-01", "2026-09-13"), Some(1));
        assert_eq!(new_days_from_old_baseline("2024-02-29", "2026-09-13"), Some(927));
        assert_eq!(new_days_from_old_baseline("junk", "2026-09-13"), None);
        assert_eq!(new_days_from_old_baseline("2026-08-01", "junk"), None);
    }

    #[test]
    fn a_file_with_only_the_old_date_is_read_once_and_the_number_stored() {
        // Through the helpers the store calls, never the store itself: the
        // store is one per process, and a test that loaded it could read and
        // write the person's own settings file.
        let today = "2026-09-13";
        let mut map = parse_settings("baseline\t2026-08-01\ntheme\tdark\n");
        let expected = new_days_from_old_baseline("2026-08-01", today).unwrap();
        assert_eq!(expected, 43);
        assert_eq!(resolve_new_days(&map, today), (expected, true));
        map.insert(NEW_DAYS.to_string(), expected.to_string());
        let text = format_settings(&map);
        assert!(text.contains(&format!("new_days\t{expected}\n")), "{text}");
        assert!(text.contains("baseline\t2026-08-01\n"), "{text}");
        assert!(text.contains("theme\tdark\n"), "{text}");
        // Read back, the number counts and nothing is stored again, even on
        // a later day: the reach stays what it was rather than growing.
        let map = parse_settings(&text);
        assert_eq!(resolve_new_days(&map, today), (expected, false));
        assert_eq!(resolve_new_days(&map, "2026-10-01"), (expected, false));
    }

    #[test]
    fn with_no_reach_and_no_old_date_the_default_applies_and_nothing_is_stored() {
        let map = parse_settings("theme\tlight\nnew_days\t0\n");
        assert_eq!(resolve_new_days(&map, "2026-09-12"), (7, false));
        assert_eq!(resolve_new_days(&BTreeMap::new(), "2026-10-05"), (30, false));
        let map = parse_settings("new_days\t30\nbaseline\t2026-08-01\n");
        assert_eq!(resolve_new_days(&map, "2026-09-13"), (30, false));
        assert!(parse_settings("no tab here\n\n").is_empty());
    }

    #[test]
    fn closed_folders_round_trip_through_one_line() {
        let folded = parse_folded("data-display/datagrid,buttons, buttons ,,");
        assert_eq!(
            folded.iter().map(String::as_str).collect::<Vec<_>>(),
            ["buttons", "data-display/datagrid"]
        );
        let line = format_folded(&folded);
        assert_eq!(line, "buttons,data-display/datagrid");
        assert_eq!(parse_folded(&line), folded);
        assert!(parse_folded("").is_empty());
        assert_eq!(format_folded(&BTreeSet::new()), "");
        assert!(!line.contains('\t') && !line.contains('\n'));
    }

    #[test]
    fn a_closed_folder_follows_its_pages_and_a_folder_that_is_gone_is_forgotten() {
        let moved = [
            ("old/pair/overview", "new/pair/overview"),
            ("old/pair/more", "new/pair/more"),
            ("old/split/one", "new/pair/overview"),
            ("old/split/two", "new/other/overview"),
            ("old/folded/overview", "new/single/overview"),
        ];
        let live: BTreeSet<String> =
            ["old", "new", "new/pair", "new/other", "kept/folder"].iter().map(|s| s.to_string()).collect();
        let carry = |line: &str| format_folded(&carry_folded(&parse_folded(line), &moved, &live));
        assert_eq!(carry("old/pair"), "new/pair");
        assert_eq!(carry("kept/folder,old"), "kept/folder,old");
        // Its pages went two ways: there is no one folder to close.
        assert_eq!(carry("old/split"), "");
        // Its pages went to a component with one page, which has no folder.
        assert_eq!(carry("old/folded"), "");
        assert_eq!(carry("never/was,gone"), "");
        assert_eq!(carry("old/pair,new/pair"), "new/pair");
        assert_eq!(carry(""), "");
    }
}
