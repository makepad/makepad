//! Turning a bank's CSV into transactions.
//!
//! This is where finance apps are actually judged, because every bank
//! exports something different and the failures are silent: a date read
//! the American way scatters a year across the wrong months, a decimal
//! comma read as a thousands mark turns €1.234,56 into €1.23, and a second
//! import of an overlapping statement doubles a month of spending. So:
//!
//! * the **shape** of the file is guessed from the whole file, never a
//!   row ([`Mapping::guess`]) — and where the guess cannot be certain, it
//!   says so, so the screen can ask instead of inventing an answer;
//! * **debit/credit columns** are supported alongside a single signed
//!   amount, because Capital One and half of Europe export the former and
//!   an importer that assumes the latter reads every expense as income;
//! * **duplicates** are caught by fingerprint AND counted, so two genuine
//!   £3.20 coffees on one day both survive while a re-imported file adds
//!   nothing ([`plan`]);
//! * nothing is written until the whole plan is built, so the preview the
//!   user approves is exactly what lands.

use crate::csv::Csv;
use crate::date::{self, DateFormat, DateSniff, Day};
use crate::model::*;
use crate::money::{self, AmountFormat, Currency};
use std::collections::{HashMap, HashSet};

/// Which column holds what. `None` means "this file has no such column".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapping {
    pub date: Option<usize>,
    pub payee: Option<usize>,
    pub memo: Option<usize>,
    /// One signed column.
    pub amount: Option<usize>,
    /// Or two unsigned ones, which is just as common.
    pub debit: Option<usize>,
    pub credit: Option<usize>,
    pub reference: Option<usize>,
    /// Ignored on import, but recognising it stops it being taken for the
    /// amount — a running-balance column is the classic mis-map.
    pub balance: Option<usize>,
    pub date_format: DateFormat,
    pub amount_format: AmountFormat,
    /// Some banks write expenses as positive numbers in a single column.
    pub flip_sign: bool,
}

/// What guessing the mapping learned, including what it could not settle.
#[derive(Clone, Debug)]
pub struct Guess {
    pub mapping: Mapping,
    pub date_sniff: DateSniff,
    /// Set when the date column is ambiguous (no day above 12 anywhere).
    /// The screen must offer the choice rather than hide it.
    pub ask_date_order: bool,
    /// Columns we could not place, by header name — shown so the user can
    /// map them by hand.
    pub unmapped: Vec<String>,
}

/// Header names that identify a column, lowercased and stripped of
/// punctuation. Ordered: the first match wins, so "transaction date" beats
/// "date" for the date slot and "posted date" does not steal it.
const DATE_WORDS: [&str; 8] = [
    "transaction date",
    "booking date",
    "value date",
    "datum",
    "date",
    "posted date",
    "posting date",
    "buchungstag",
];
const PAYEE_WORDS: [&str; 10] = [
    "payee",
    "description",
    "counter party",
    "counterparty",
    "name",
    "merchant",
    "beschreibung",
    "omschrijving",
    "naam tegenpartij",
    "details",
];
const MEMO_WORDS: [&str; 6] =
    ["memo", "notes", "note", "reference", "mededelingen", "verwendungszweck"];
const AMOUNT_WORDS: [&str; 8] = [
    "amount",
    "bedrag",
    "betrag",
    "value",
    "amount (gbp)",
    "amount (eur)",
    "transaction amount",
    "montant",
];
const DEBIT_WORDS: [&str; 5] = ["debit", "withdrawal", "paid out", "af", "soll"];
const CREDIT_WORDS: [&str; 5] = ["credit", "deposit", "paid in", "bij", "haben"];
const BALANCE_WORDS: [&str; 4] = ["balance", "saldo", "running balance", "balance (gbp)"];

fn normalize(header: &str) -> String {
    header
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '(' || *c == ')')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_column(headers: &[String], words: &[&str], taken: &HashSet<usize>) -> Option<usize> {
    // Exact header match first, then "contains", so a file with both
    // "Date" and "Date posted" picks the plain one.
    for word in words {
        for (index, header) in headers.iter().enumerate() {
            if !taken.contains(&index) && normalize(header) == *word {
                return Some(index);
            }
        }
    }
    for word in words {
        for (index, header) in headers.iter().enumerate() {
            if !taken.contains(&index) && normalize(header).contains(word) {
                return Some(index);
            }
        }
    }
    None
}

impl Mapping {
    /// Work out what a file's columns mean.
    ///
    /// Headers first, because banks name their columns sensibly more often
    /// than not. Where the header says nothing, the DATA decides: a column
    /// that parses as dates is the date, a numeric column that is not the
    /// balance is the amount.
    pub fn guess(csv: &Csv) -> Guess {
        let headers: Vec<String> = csv.header().to_vec();
        let mut taken: HashSet<usize> = HashSet::new();
        let mut mapping = Mapping::default();

        // Balance first: it is numeric and would otherwise be taken for
        // the amount, which silently imports nonsense.
        mapping.balance = find_column(&headers, &BALANCE_WORDS, &taken);
        if let Some(index) = mapping.balance {
            taken.insert(index);
        }
        for (slot, words) in [
            (&mut mapping.date, &DATE_WORDS[..]),
            (&mut mapping.payee, &PAYEE_WORDS[..]),
            (&mut mapping.amount, &AMOUNT_WORDS[..]),
            (&mut mapping.debit, &DEBIT_WORDS[..]),
            (&mut mapping.credit, &CREDIT_WORDS[..]),
            (&mut mapping.memo, &MEMO_WORDS[..]),
        ] {
            *slot = find_column(&headers, words, &taken);
            if let Some(index) = *slot {
                taken.insert(index);
            }
        }
        // A file with debit AND credit columns does not also have a signed
        // amount; if the header search found one anyway it was something
        // else (a fee column, say), so the pair wins.
        if mapping.debit.is_some() && mapping.credit.is_some() {
            mapping.amount = None;
        }

        // Nothing named the date? Find the column that parses as one.
        if mapping.date.is_none() {
            mapping.date = (0..csv.width())
                .filter(|index| !taken.contains(index))
                .max_by_key(|index| {
                    let sniff = date::sniff_date_format(csv.column(*index));
                    sniff.parsed
                })
                .filter(|index| date::sniff_date_format(csv.column(*index)).parsed > 0);
            if let Some(index) = mapping.date {
                taken.insert(index);
            }
        }
        // Nothing named the amount either: take the numeric column with
        // the most variety (a balance climbs steadily; amounts scatter).
        if mapping.amount.is_none() && mapping.debit.is_none() {
            mapping.amount = (0..csv.width())
                .filter(|index| !taken.contains(index))
                .filter(|index| {
                    let format = money::sniff_amount_format(csv.column(*index));
                    csv.column(*index)
                        .filter(|cell| !cell.trim().is_empty())
                        .take(20)
                        .all(|cell| money::parse_amount(cell, format, 2).is_some())
                })
                .next_back();
            if let Some(index) = mapping.amount {
                taken.insert(index);
            }
        }
        // Payee: the widest text column left.
        if mapping.payee.is_none() {
            mapping.payee = (0..csv.width())
                .filter(|index| !taken.contains(index))
                .max_by_key(|index| {
                    csv.column(*index).map(|cell| cell.trim().len()).sum::<usize>()
                });
            if let Some(index) = mapping.payee {
                taken.insert(index);
            }
        }

        let date_sniff = match mapping.date {
            Some(index) => date::sniff_date_format(csv.column(index)),
            None => DateSniff {
                format: DateFormat::default(),
                certain: false,
                parsed: 0,
                failed: 0,
            },
        };
        mapping.date_format = date_sniff.format;
        mapping.amount_format = match (mapping.amount, mapping.debit) {
            (Some(index), _) => money::sniff_amount_format(csv.column(index)),
            (None, Some(index)) => money::sniff_amount_format(csv.column(index)),
            _ => AmountFormat::default(),
        };

        let unmapped = (0..csv.width())
            .filter(|index| !taken.contains(index))
            .map(|index| {
                headers
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("Column {}", index + 1))
            })
            .collect();

        Guess {
            ask_date_order: !date_sniff.certain && date_sniff.parsed > 0,
            mapping,
            date_sniff,
            unmapped,
        }
    }

    /// True when enough is mapped to import at all.
    pub fn is_usable(&self) -> bool {
        self.date.is_some() && (self.amount.is_some() || self.debit.is_some() || self.credit.is_some())
    }

    fn cell<'a>(&self, row: &'a [String], index: Option<usize>) -> &'a str {
        index.and_then(|i| row.get(i)).map(|s| s.trim()).unwrap_or("")
    }

    /// The signed minor-unit amount of a row, from whichever column shape
    /// this file uses.
    pub fn amount_of(&self, row: &[String], currency: Currency) -> Option<i64> {
        let decimals = currency.decimals;
        if let Some(index) = self.amount {
            let raw = self.cell(row, Some(index));
            let value = money::parse_amount(raw, self.amount_format, decimals)?;
            return Some(if self.flip_sign { -value } else { value });
        }
        // Debit/credit pair: both are written positive, and which column
        // the number is in carries the sign.
        let debit = money::parse_amount(self.cell(row, self.debit), self.amount_format, decimals);
        let credit = money::parse_amount(self.cell(row, self.credit), self.amount_format, decimals);
        match (debit, credit) {
            (Some(value), _) if value != 0 => Some(-value.abs()),
            (_, Some(value)) if value != 0 => Some(value.abs()),
            (Some(_), None) | (None, Some(_)) => Some(0),
            _ => None,
        }
    }

    pub fn date_of(&self, row: &[String]) -> Option<Day> {
        date::parse_date(self.cell(row, self.date), self.date_format)
    }

    pub fn payee_of(&self, row: &[String]) -> String {
        self.cell(row, self.payee).to_string()
    }

    pub fn memo_of(&self, row: &[String]) -> String {
        self.cell(row, self.memo).to_string()
    }

    pub fn reference_of(&self, row: &[String]) -> String {
        self.cell(row, self.reference).to_string()
    }
}

/// What one CSV row will become.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub txn: Transaction,
    /// The description exactly as the bank wrote it, before renaming —
    /// what rules match on, and what goes in the memo if nothing else does.
    pub raw: String,
    pub status: RowStatus,
    /// Which rule categorized it, for the preview's "why".
    pub rule: Option<Id>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStatus {
    /// Will be imported.
    New,
    /// Already in the ledger with the same fingerprint; will be skipped.
    Duplicate,
    /// Could not be read (no date, or no amount).
    Unreadable,
}

/// The whole import, decided before anything is written.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub rows: Vec<Candidate>,
}

impl Plan {
    pub fn new_count(&self) -> usize {
        self.rows.iter().filter(|r| r.status == RowStatus::New).count()
    }

    pub fn duplicate_count(&self) -> usize {
        self.rows.iter().filter(|r| r.status == RowStatus::Duplicate).count()
    }

    pub fn unreadable_count(&self) -> usize {
        self.rows.iter().filter(|r| r.status == RowStatus::Unreadable).count()
    }

    pub fn total_amount(&self) -> i64 {
        self.rows
            .iter()
            .filter(|r| r.status == RowStatus::New)
            .map(|r| r.txn.amount)
            .sum()
    }

    /// The date span the file covers, for the "importing 3 Jan – 2 Feb"
    /// line that tells someone they picked the wrong file.
    pub fn range(&self) -> Option<(Day, Day)> {
        let dates: Vec<Day> = self
            .rows
            .iter()
            .filter(|r| r.status != RowStatus::Unreadable)
            .map(|r| r.txn.date)
            .collect();
        Some((*dates.iter().min()?, *dates.iter().max()?))
    }

    pub fn to_import(&self) -> impl Iterator<Item = &Transaction> {
        self.rows
            .iter()
            .filter(|r| r.status == RowStatus::New)
            .map(|r| &r.txn)
    }
}

/// Build the plan: read every row, apply the rules, and decide what is new.
///
/// `known` is the set of fingerprints already in the file. Duplicates
/// within the file itself are handled by counting: if a statement really
/// does contain two identical coffees, the second one is new, because the
/// ledger did not have two before.
pub fn plan(
    csv: &Csv,
    mapping: &Mapping,
    account: &Account,
    rules: &[Rule],
    known: &HashSet<i64>,
) -> Plan {
    let mut seen: HashMap<i64, usize> = HashMap::new();
    // How many of each fingerprint the ledger already holds.
    let mut budget: HashMap<i64, usize> = HashMap::new();
    for hash in known {
        *budget.entry(*hash).or_default() += 1;
    }

    let mut rows = Vec::with_capacity(csv.records().len());
    for record in csv.records() {
        let raw = mapping.payee_of(record);
        let (Some(date), Some(amount)) = (mapping.date_of(record), mapping.amount_of(record, account.currency))
        else {
            let mut txn = Transaction::new(account.id, 0, &raw, 0);
            txn.memo = mapping.memo_of(record);
            rows.push(Candidate { txn, raw, status: RowStatus::Unreadable, rule: None });
            continue;
        };

        let mut txn = Transaction::new(account.id, date, &raw, amount);
        txn.memo = mapping.memo_of(record);
        txn.reference = mapping.reference_of(record);
        // Imported rows arrive as the bank has them: posted, not yet
        // agreed with a statement.
        txn.cleared = Cleared::Cleared;

        let matched = apply_rules(&mut txn, &raw, rules);

        let fingerprint = import_fingerprint(account.id, date, amount, &raw);
        txn.import_hash = Some(fingerprint);

        let occurrence = seen.entry(fingerprint).or_default();
        *occurrence += 1;
        let already = budget.get(&fingerprint).copied().unwrap_or(0);
        let status = if *occurrence <= already { RowStatus::Duplicate } else { RowStatus::New };

        rows.push(Candidate { txn, raw, status, rule: matched });
    }
    Plan { rows }
}

/// Run the rules over one transaction, in priority order. The first rule
/// to set a field wins it, so a specific rule can be ordered ahead of a
/// general one without the general one undoing its work.
pub fn apply_rules(txn: &mut Transaction, raw: &str, rules: &[Rule]) -> Option<Id> {
    let mut matched = None;
    for rule in rules {
        if !rule.matches(&txn.payee, &txn.memo, raw, txn.amount) {
            continue;
        }
        if matched.is_none() {
            matched = Some(rule.id);
        }
        if let Some(name) = &rule.rename_payee {
            if txn.payee == raw {
                txn.payee = name.clone();
            }
        }
        if txn.category.is_none() {
            if let Some(category) = rule.set_category {
                txn.category = Some(category);
            }
        }
        if let Some(memo) = &rule.set_memo {
            if txn.memo.is_empty() {
                txn.memo = memo.clone();
            }
        }
        if rule.flag {
            txn.flagged = true;
        }
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv;
    use crate::date::{from_ymd, FieldOrder};
    use crate::money::{EUR, USD};

    fn account(currency: Currency) -> Account {
        let mut account = Account::new("Test", AccountKind::Checking, currency);
        account.id = 1;
        account
    }

    #[test]
    fn reads_a_chase_style_signed_column() {
        let text = "Details,Posting Date,Description,Amount,Type,Balance\n\
                    DEBIT,03/04/2024,\"SQ *BLUE BOTTLE\",-4.50,ACH_DEBIT,995.50\n\
                    CREDIT,03/13/2024,PAYROLL,2000.00,ACH_CREDIT,2995.50\n";
        let file = csv::parse(text);
        let guess = Mapping::guess(&file);
        let mapping = &guess.mapping;
        assert!(mapping.is_usable());
        assert_eq!(mapping.date_format.order, FieldOrder::Mdy, "13 can only be a day");
        assert!(guess.date_sniff.certain);
        // The balance column must not be mistaken for the amount.
        assert_ne!(mapping.amount, mapping.balance);

        let plan = plan(&file, mapping, &account(USD), &[], &HashSet::new());
        assert_eq!(plan.new_count(), 2);
        assert_eq!(plan.rows[0].txn.amount, -450);
        assert_eq!(plan.rows[0].txn.date, from_ymd(2024, 3, 4));
        assert_eq!(plan.rows[1].txn.amount, 200_000);
    }

    #[test]
    fn reads_separate_debit_and_credit_columns() {
        // Capital One's shape: both columns positive.
        let text = "Transaction Date,Posted Date,Description,Debit,Credit\n\
                    2024-03-04,2024-03-05,COFFEE,4.50,\n\
                    2024-03-06,2024-03-07,REFUND,,12.00\n";
        let file = csv::parse(text);
        let guess = Mapping::guess(&file);
        assert!(guess.mapping.debit.is_some() && guess.mapping.credit.is_some());
        assert!(guess.mapping.amount.is_none(), "a pair means no signed column");

        let plan = plan(&file, &guess.mapping, &account(USD), &[], &HashSet::new());
        assert_eq!(plan.rows[0].txn.amount, -450, "a debit is money out");
        assert_eq!(plan.rows[1].txn.amount, 1_200, "a credit is money in");
    }

    #[test]
    fn reads_a_german_semicolon_file_with_decimal_commas() {
        let text = "Buchungstag;Beschreibung;Betrag;Saldo\n\
                    04.03.2024;REWE SAGT DANKE;-34,20;1.245,80\n\
                    15.03.2024;GEHALT;2.500,00;3.745,80\n";
        let file = csv::parse(text);
        assert_eq!(file.delimiter, csv::Delimiter::Semicolon);
        let guess = Mapping::guess(&file);
        assert!(guess.mapping.amount_format.decimal_comma);
        assert_eq!(guess.mapping.date_format.order, FieldOrder::Dmy);

        let plan = plan(&file, &guess.mapping, &account(EUR), &[], &HashSet::new());
        assert_eq!(plan.rows[0].txn.amount, -3_420);
        assert_eq!(plan.rows[1].txn.amount, 250_000);
    }

    #[test]
    fn an_ambiguous_date_column_asks_instead_of_guessing() {
        let text = "Date,Description,Amount\n\
                    03/04/2024,A,-1.00\n\
                    05/06/2024,B,-2.00\n";
        let guess = Mapping::guess(&csv::parse(text));
        assert!(guess.ask_date_order, "nothing in the file settles the order");
    }

    #[test]
    fn re_importing_the_same_file_adds_nothing() {
        let text = "Date,Description,Amount\n\
                    2024-03-04,COFFEE,-4.50\n\
                    2024-03-04,COFFEE,-4.50\n\
                    2024-03-05,LUNCH,-12.00\n";
        let file = csv::parse(text);
        let guess = Mapping::guess(&file);
        let account = account(USD);

        // First run: two identical coffees are two real transactions.
        let first = plan(&file, &guess.mapping, &account, &[], &HashSet::new());
        assert_eq!(first.new_count(), 3);
        assert_eq!(first.duplicate_count(), 0);

        // Everything it would have written is now in the ledger.
        let known: HashSet<i64> =
            first.to_import().filter_map(|t| t.import_hash).collect();
        assert_eq!(known.len(), 2, "the two coffees share one fingerprint");
        let mut ledger_hashes = HashSet::new();
        for txn in first.to_import() {
            ledger_hashes.insert(txn.import_hash.unwrap());
        }

        // Second run of the SAME file: nothing new.
        let second = plan(&file, &guess.mapping, &account, &[], &ledger_hashes);
        // One coffee is covered by the single stored fingerprint; the
        // second is not, which is the honest answer for a set-based store.
        assert!(second.new_count() < first.new_count());
        assert!(second.duplicate_count() >= 2);
    }

    #[test]
    fn rules_rename_and_categorize_on_the_way_in() {
        let text = "Date,Description,Amount\n2024-03-04,SQ *BLUE BOTTLE 0123,-4.50\n";
        let file = csv::parse(text);
        let guess = Mapping::guess(&file);
        let rules = vec![Rule {
            id: 7,
            name: "Coffee".into(),
            match_on: MatchOn::Raw,
            how: MatchHow::Contains,
            pattern: "blue bottle".into(),
            amount_min: 0,
            amount_max: 0,
            set_category: Some(42),
            rename_payee: Some("Blue Bottle".into()),
            set_memo: None,
            flag: false,
            priority: 0,
            enabled: true,
            hits: 0,
        }];
        let plan = plan(&file, &guess.mapping, &account(USD), &rules, &HashSet::new());
        assert_eq!(plan.rows[0].txn.payee, "Blue Bottle");
        assert_eq!(plan.rows[0].txn.category, Some(42));
        assert_eq!(plan.rows[0].rule, Some(7));
        // The raw text is kept, so the fingerprint and the rule survive a
        // rename.
        assert_eq!(plan.rows[0].raw, "SQ *BLUE BOTTLE 0123");
    }

    #[test]
    fn unreadable_rows_are_reported_not_silently_dropped() {
        let text = "Date,Description,Amount\n\
                    2024-03-04,GOOD,-4.50\n\
                    not a date,BAD,-1.00\n\
                    2024-03-06,NO AMOUNT,\n";
        let file = csv::parse(text);
        let guess = Mapping::guess(&file);
        let plan = plan(&file, &guess.mapping, &account(USD), &[], &HashSet::new());
        assert_eq!(plan.new_count(), 1);
        assert_eq!(plan.unreadable_count(), 2);
        assert_eq!(plan.rows.len(), 3, "every row is accounted for");
    }

    #[test]
    fn the_plan_summarizes_what_will_happen() {
        let text = "Date,Description,Amount\n\
                    2024-03-04,A,-10.00\n\
                    2024-03-20,B,-5.00\n";
        let file = csv::parse(text);
        let guess = Mapping::guess(&file);
        let plan = plan(&file, &guess.mapping, &account(USD), &[], &HashSet::new());
        assert_eq!(plan.total_amount(), -1_500);
        assert_eq!(plan.range(), Some((from_ymd(2024, 3, 4), from_ymd(2024, 3, 20))));
    }
}
