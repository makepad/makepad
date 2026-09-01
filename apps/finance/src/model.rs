//! What a personal ledger is made of.
//!
//! The shape here is the one every consumer finance product converged on,
//! and it is worth saying why it is not double-entry. GnuCash models a
//! transaction as a bundle of splits that must sum to zero across accounts,
//! which is correct and is also why its users talk about "learning
//! accounting". Consumer apps — Quicken, YNAB, Monarch — instead give a
//! transaction ONE account and a signed amount, and represent a movement
//! between two accounts as a linked PAIR of such rows. The ledger is then
//! trivially "the rows of this account", which is the query the app runs a
//! thousand times more often than any other.
//!
//! We take the consumer model, with two rules that keep it honest:
//!
//! * a transfer is a pair joined by [`Transaction::transfer_group`], and
//!   the pair's amounts must be equal and opposite (see [`Ledger::transfer_is_balanced`]);
//! * a split transaction's parts must sum to its amount, always — enforced
//!   by [`Transaction::split_imbalance`] rather than hoped for.
//!
//! Money is `i64` minor units throughout ([`crate::money`]) and dates are
//! day numbers ([`crate::date`]). No floats, no timestamps.

use crate::date::Day;
use crate::money::Currency;

pub type Id = i64;

/// Ids are assigned by the database; this is what an unsaved row carries.
pub const NO_ID: Id = 0;

// ---------------------------------------------------------------- accounts

/// What kind of thing an account is. This drives the sign convention the
/// UI shows, whether a balance counts as an asset or a debt in net worth,
/// and which screens the account appears on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccountKind {
    Checking,
    Savings,
    Cash,
    /// A card. Balances are normally negative (you owe); the UI offers to
    /// show them flipped, the way a statement does.
    CreditCard,
    /// Mortgage, student loan, car loan. Negative, and paid down.
    Loan,
    /// Brokerage or retirement. Holds securities as well as cash.
    Investment,
    /// A house, a car — something worth money that has no transactions
    /// except revaluations.
    Asset,
    /// Money you are owed or owe outside a bank (a friend, an employer).
    Liability,
}

impl AccountKind {
    pub const ALL: [AccountKind; 8] = [
        AccountKind::Checking,
        AccountKind::Savings,
        AccountKind::Cash,
        AccountKind::CreditCard,
        AccountKind::Loan,
        AccountKind::Investment,
        AccountKind::Asset,
        AccountKind::Liability,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AccountKind::Checking => "Checking",
            AccountKind::Savings => "Savings",
            AccountKind::Cash => "Cash",
            AccountKind::CreditCard => "Credit card",
            AccountKind::Loan => "Loan",
            AccountKind::Investment => "Investment",
            AccountKind::Asset => "Asset",
            AccountKind::Liability => "Liability",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AccountKind::Checking => "checking",
            AccountKind::Savings => "savings",
            AccountKind::Cash => "cash",
            AccountKind::CreditCard => "credit_card",
            AccountKind::Loan => "loan",
            AccountKind::Investment => "investment",
            AccountKind::Asset => "asset",
            AccountKind::Liability => "liability",
        }
    }

    pub fn from_str(text: &str) -> AccountKind {
        AccountKind::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == text)
            .unwrap_or(AccountKind::Checking)
    }

    /// True for accounts whose balance is money you owe. Net worth adds
    /// every balance as it stands (debts are already negative); this is for
    /// grouping and for the "show positive" display option.
    pub fn is_debt(self) -> bool {
        matches!(self, AccountKind::CreditCard | AccountKind::Loan | AccountKind::Liability)
    }

    /// Accounts that hold securities, and so get a holdings view.
    pub fn holds_securities(self) -> bool {
        matches!(self, AccountKind::Investment)
    }

    /// Accounts whose balance moves by revaluation, not by spending — they
    /// are excluded from cash-flow and budget screens.
    pub fn is_valuation_only(self) -> bool {
        matches!(self, AccountKind::Asset)
    }
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: Id,
    pub name: String,
    pub kind: AccountKind,
    pub currency: Currency,
    pub institution: String,
    /// The balance before the first transaction we hold — what makes an
    /// imported partial history add up to the real balance.
    pub opening_balance: i64,
    pub opening_date: Day,
    /// Closed accounts stay for history but leave the sidebar by default.
    pub closed: bool,
    /// Kept out of net worth (a business account in a personal file).
    pub off_budget: bool,
    pub sort_order: i32,
    /// Free text: last four digits, IBAN tail, whatever identifies it.
    pub note: String,
}

impl Account {
    pub fn new(name: &str, kind: AccountKind, currency: Currency) -> Account {
        Account {
            id: NO_ID,
            name: name.to_string(),
            kind,
            currency,
            institution: String::new(),
            opening_balance: 0,
            opening_date: 0,
            closed: false,
            off_budget: false,
            sort_order: 0,
            note: String::new(),
        }
    }
}

// -------------------------------------------------------------- categories

/// Categories are a two-level tree — group ("Food") and child ("Groceries")
/// — because that is what every product settled on and what budgets are
/// laid out as. Deeper nesting buys nothing and makes every report a
/// recursion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CategoryKind {
    Income,
    Expense,
    /// Neither: the two halves of a transfer, and the opening balance.
    /// Excluded from spending reports and from budgets.
    Transfer,
}

impl CategoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CategoryKind::Income => "income",
            CategoryKind::Expense => "expense",
            CategoryKind::Transfer => "transfer",
        }
    }

    pub fn from_str(text: &str) -> CategoryKind {
        match text {
            "income" => CategoryKind::Income,
            "transfer" => CategoryKind::Transfer,
            _ => CategoryKind::Expense,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Category {
    pub id: Id,
    pub name: String,
    /// `None` for a group; `Some(group_id)` for a child.
    pub parent: Option<Id>,
    pub kind: CategoryKind,
    /// Budgeted categories appear on the budget screen with a monthly
    /// target. Groups and transfers are not budgeted directly.
    pub budgeted: bool,
    /// Where unspent money goes at month end (envelope budgeting).
    pub rollover: bool,
    pub sort_order: i32,
    /// A hue for charts, so a category keeps its colour everywhere.
    pub color: u32,
    pub hidden: bool,
}

impl Category {
    pub fn group(name: &str, kind: CategoryKind) -> Category {
        Category {
            id: NO_ID,
            name: name.to_string(),
            parent: None,
            kind,
            budgeted: false,
            rollover: false,
            sort_order: 0,
            color: 0,
            hidden: false,
        }
    }

    pub fn child(name: &str, parent: Id, kind: CategoryKind) -> Category {
        Category { parent: Some(parent), ..Category::group(name, kind) }
    }

    pub fn is_group(&self) -> bool {
        self.parent.is_none()
    }
}

// ------------------------------------------------------------ transactions

/// How far a transaction has got towards being real money.
///
/// The three states are the ones a statement forces on you: the bank has
/// not shown it yet, the bank has shown it, and you have agreed with the
/// bank that it happened (reconciled). Reconciled rows are protected from
/// casual editing, because changing one silently breaks a balance you
/// already agreed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cleared {
    Uncleared,
    Cleared,
    Reconciled,
}

impl Cleared {
    pub fn as_str(self) -> &'static str {
        match self {
            Cleared::Uncleared => "uncleared",
            Cleared::Cleared => "cleared",
            Cleared::Reconciled => "reconciled",
        }
    }

    pub fn from_str(text: &str) -> Cleared {
        match text {
            "cleared" => Cleared::Cleared,
            "reconciled" => Cleared::Reconciled,
            _ => Cleared::Uncleared,
        }
    }

    /// The one-character mark the ledger column shows.
    pub fn mark(self) -> &'static str {
        match self {
            Cleared::Uncleared => "",
            Cleared::Cleared => "c",
            Cleared::Reconciled => "R",
        }
    }
}

/// One part of a split transaction: an amount against a category.
#[derive(Clone, Debug)]
pub struct Split {
    pub id: Id,
    pub category: Option<Id>,
    pub amount: i64,
    pub memo: String,
}

/// A row in an account's register.
///
/// Sign convention, once, for the whole app: **money leaving the account is
/// negative**. A card purchase is negative, a refund positive, a salary
/// positive, a payment from checking to the card is negative on checking
/// and positive on the card. There is no per-account inversion anywhere in
/// the data — only in how a credit-card balance may be DISPLAYED.
#[derive(Clone, Debug)]
pub struct Transaction {
    pub id: Id,
    pub account: Id,
    pub date: Day,
    /// Who it was with. Free text, normalized against the payee table on
    /// import so "AMZN Mktp US*2K4L" and "Amazon" become one payee.
    pub payee: String,
    pub memo: String,
    /// Signed minor units, in the ACCOUNT's currency.
    pub amount: i64,
    /// `None` = uncategorized, which the UI nags about. Ignored when the
    /// transaction has splits — the splits carry the categories then.
    pub category: Option<Id>,
    pub splits: Vec<Split>,
    /// Both rows of a transfer carry the same group id.
    pub transfer_group: Option<Id>,
    pub cleared: Cleared,
    /// The statement this row was reconciled on, if any.
    pub statement: Option<Id>,
    /// Fingerprint of the imported line, so re-importing the same file does
    /// not double every row. `None` for hand-entered rows.
    pub import_hash: Option<i64>,
    /// A cheque number or the bank's own reference.
    pub reference: String,
    pub flagged: bool,
    pub notes: String,
}

impl Transaction {
    pub fn new(account: Id, date: Day, payee: &str, amount: i64) -> Transaction {
        Transaction {
            id: NO_ID,
            account,
            date,
            payee: payee.to_string(),
            memo: String::new(),
            amount,
            category: None,
            splits: Vec::new(),
            transfer_group: None,
            cleared: Cleared::Uncleared,
            statement: None,
            import_hash: None,
            reference: String::new(),
            flagged: false,
            notes: String::new(),
        }
    }

    pub fn is_split(&self) -> bool {
        !self.splits.is_empty()
    }

    pub fn is_transfer(&self) -> bool {
        self.transfer_group.is_some()
    }

    /// How far the splits are from the transaction's amount. Zero is the
    /// only valid state to save; the editor shows the remainder while you
    /// type, the way every product does.
    pub fn split_imbalance(&self) -> i64 {
        if self.splits.is_empty() {
            return 0;
        }
        self.amount - self.splits.iter().map(|s| s.amount).sum::<i64>()
    }

    /// The categories this transaction touches — one, or all of the split
    /// parts'. Reports iterate this rather than special-casing splits.
    pub fn category_amounts(&self) -> Vec<(Option<Id>, i64)> {
        if self.splits.is_empty() {
            vec![(self.category, self.amount)]
        } else {
            self.splits.iter().map(|s| (s.category, s.amount)).collect()
        }
    }

    /// What the ledger shows in the category column.
    pub fn category_label(&self, categories: &CategoryTree) -> String {
        if self.splits.len() > 1 {
            return format!("Split ({})", self.splits.len());
        }
        let id = if self.splits.len() == 1 { self.splits[0].category } else { self.category };
        match id {
            Some(id) => categories.path(id),
            None if self.is_transfer() => "Transfer".to_string(),
            None => String::new(),
        }
    }

    /// Reconciled rows resist editing: changing one invalidates a balance
    /// the user already agreed with the bank.
    pub fn is_locked(&self) -> bool {
        self.cleared == Cleared::Reconciled
    }
}

/// The fingerprint that stops a re-imported file from doubling the ledger.
///
/// Built from the fields a bank cannot change between two exports of the
/// same transaction: account, date, amount, and a squashed form of the
/// description. NOT the running balance (it shifts as later rows arrive)
/// and not the row number (it moves). Two genuinely identical transactions
/// on one day — two £3.20 coffees — collide by design; the importer
/// resolves that by counting occurrences, not by dropping them.
pub fn import_fingerprint(account: Id, date: Day, amount: i64, description: &str) -> i64 {
    // FNV-1a over the normalized parts.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&account.to_le_bytes());
    eat(&date.to_le_bytes());
    eat(&amount.to_le_bytes());
    // Case, punctuation and runs of spaces differ between two exports of
    // the same row often enough to matter.
    let mut last_space = false;
    for ch in description.chars() {
        if ch.is_alphanumeric() {
            last_space = false;
            let lower = ch.to_ascii_lowercase();
            let mut buf = [0u8; 4];
            eat(lower.encode_utf8(&mut buf).as_bytes());
        } else if !last_space {
            last_space = true;
            eat(b" ");
        }
    }
    hash as i64
}

// ------------------------------------------------------------- category tree

/// Categories with their parent/child structure resolved, which is what
/// every screen wants — the flat table is only how they are stored.
#[derive(Clone, Debug, Default)]
pub struct CategoryTree {
    pub categories: Vec<Category>,
}

impl CategoryTree {
    pub fn get(&self, id: Id) -> Option<&Category> {
        self.categories.iter().find(|c| c.id == id)
    }

    pub fn name(&self, id: Id) -> &str {
        self.get(id).map(|c| c.name.as_str()).unwrap_or("")
    }

    /// `Food: Groceries` — what the ledger's category column shows.
    pub fn path(&self, id: Id) -> String {
        match self.get(id) {
            Some(category) => match category.parent.and_then(|p| self.get(p)) {
                Some(parent) => format!("{}: {}", parent.name, category.name),
                None => category.name.clone(),
            },
            None => String::new(),
        }
    }

    pub fn groups(&self) -> impl Iterator<Item = &Category> {
        self.categories.iter().filter(|c| c.is_group())
    }

    pub fn children_of(&self, parent: Id) -> impl Iterator<Item = &Category> {
        self.categories.iter().filter(move |c| c.parent == Some(parent))
    }

    /// Budgetable leaves in display order: each group followed by its
    /// children — the row order of the budget screen.
    pub fn budget_order(&self) -> Vec<&Category> {
        let mut out = Vec::new();
        let mut groups: Vec<&Category> = self
            .groups()
            .filter(|g| g.kind != CategoryKind::Transfer && !g.hidden)
            .collect();
        groups.sort_by_key(|g| (g.kind == CategoryKind::Expense, g.sort_order, g.name.clone()));
        for group in groups {
            out.push(group);
            let mut children: Vec<&Category> =
                self.children_of(group.id).filter(|c| !c.hidden).collect();
            children.sort_by_key(|c| (c.sort_order, c.name.clone()));
            out.extend(children);
        }
        out
    }

    /// The group a category belongs to (itself, if it is a group).
    pub fn group_of(&self, id: Id) -> Option<Id> {
        let category = self.get(id)?;
        Some(category.parent.unwrap_or(category.id))
    }

    pub fn kind_of(&self, id: Id) -> CategoryKind {
        self.get(id).map(|c| c.kind).unwrap_or(CategoryKind::Expense)
    }
}

// -------------------------------------------------------------- budgeting

/// One category's budget for one month.
///
/// Envelope budgeting in the YNAB sense: you assign an amount to a category
/// for a month, spend against it, and what is left either rolls into next
/// month or does not. `assigned` is the decision; everything else is
/// computed from the ledger, never stored, so it cannot go stale.
#[derive(Clone, Copy, Debug)]
pub struct BudgetEntry {
    pub category: Id,
    pub month: crate::date::MonthKey,
    pub assigned: i64,
    pub rollover: bool,
}

/// What the budget screen shows for one category in one month.
#[derive(Clone, Copy, Debug, Default)]
pub struct BudgetLine {
    pub assigned: i64,
    /// Positive number: what left the account for this category.
    pub spent: i64,
    /// Carried in from previous months (rollover categories only).
    pub carried: i64,
    pub available: i64,
}

impl BudgetLine {
    pub fn state(&self) -> BudgetState {
        if self.available < 0 {
            BudgetState::Overspent
        } else if self.assigned == 0 && self.spent == 0 {
            BudgetState::Untouched
        } else if self.available == 0 {
            BudgetState::Exact
        } else if self.spent > 0 && self.available > 0 {
            BudgetState::OnTrack
        } else {
            BudgetState::Funded
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetState {
    Untouched,
    Funded,
    OnTrack,
    Exact,
    Overspent,
}

// ----------------------------------------------------------------- payees

/// A payee, learned from imports. The rename is what makes a ledger
/// readable: banks write `SQ *BLUE BOTTLE 0123`, a person reads
/// `Blue Bottle Coffee`.
#[derive(Clone, Debug)]
pub struct Payee {
    pub id: Id,
    pub name: String,
    /// The category to apply when this payee shows up with no other rule.
    pub default_category: Option<Id>,
    pub transactions: i64,
}

// ------------------------------------------------------------------ rules

/// How a rule decides it applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchOn {
    Payee,
    Memo,
    /// Description as imported, before any renaming.
    Raw,
    Amount,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchHow {
    Contains,
    StartsWith,
    Equals,
    /// For amounts: exactly this value (in minor units).
    AmountEquals,
    AmountBetween,
}

/// An auto-categorization rule. Deliberately not regex: the people who
/// need rules most are the ones who will not write `^SQ \*(.+?) \d+$`, and
/// "contains" covers the overwhelming majority of real cases.
#[derive(Clone, Debug)]
pub struct Rule {
    pub id: Id,
    pub name: String,
    pub match_on: MatchOn,
    pub how: MatchHow,
    pub pattern: String,
    pub amount_min: i64,
    pub amount_max: i64,
    /// What to do when it matches.
    pub set_category: Option<Id>,
    pub rename_payee: Option<String>,
    pub set_memo: Option<String>,
    pub flag: bool,
    /// Lower runs first; the first rule that sets a field wins it.
    pub priority: i32,
    pub enabled: bool,
    pub hits: i64,
}

impl Rule {
    pub fn matches(&self, payee: &str, memo: &str, raw: &str, amount: i64) -> bool {
        if !self.enabled {
            return false;
        }
        let haystack = match self.match_on {
            MatchOn::Payee => payee,
            MatchOn::Memo => memo,
            MatchOn::Raw => raw,
            MatchOn::Amount => "",
        };
        match self.how {
            MatchHow::Contains => {
                !self.pattern.is_empty()
                    && haystack.to_lowercase().contains(&self.pattern.to_lowercase())
            }
            MatchHow::StartsWith => {
                !self.pattern.is_empty()
                    && haystack.to_lowercase().starts_with(&self.pattern.to_lowercase())
            }
            MatchHow::Equals => haystack.eq_ignore_ascii_case(&self.pattern),
            MatchHow::AmountEquals => amount == self.amount_min,
            MatchHow::AmountBetween => amount >= self.amount_min && amount <= self.amount_max,
        }
    }
}

// ------------------------------------------------------------- scheduling

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recurrence {
    Weekly,
    Fortnightly,
    Monthly,
    Quarterly,
    Yearly,
}

impl Recurrence {
    pub fn label(self) -> &'static str {
        match self {
            Recurrence::Weekly => "Weekly",
            Recurrence::Fortnightly => "Every 2 weeks",
            Recurrence::Monthly => "Monthly",
            Recurrence::Quarterly => "Quarterly",
            Recurrence::Yearly => "Yearly",
        }
    }

    /// The next occurrence after `from`. Monthly and longer clamp the day
    /// of month, so a bill on the 31st lands on the 28th in February
    /// instead of skipping the month.
    pub fn next(self, from: Day) -> Day {
        match self {
            Recurrence::Weekly => from + 7,
            Recurrence::Fortnightly => from + 14,
            Recurrence::Monthly => crate::date::add_months(from, 1),
            Recurrence::Quarterly => crate::date::add_months(from, 3),
            Recurrence::Yearly => crate::date::add_months(from, 12),
        }
    }

    pub fn approx_days(self) -> i32 {
        match self {
            Recurrence::Weekly => 7,
            Recurrence::Fortnightly => 14,
            Recurrence::Monthly => 30,
            Recurrence::Quarterly => 91,
            Recurrence::Yearly => 365,
        }
    }
}

/// A bill or paycheque that repeats. Drives the upcoming list, the
/// cash-flow forecast, and the subscription screen.
#[derive(Clone, Debug)]
pub struct Scheduled {
    pub id: Id,
    pub account: Id,
    pub payee: String,
    pub amount: i64,
    pub category: Option<Id>,
    pub recurrence: Recurrence,
    pub next_due: Day,
    pub last_posted: Option<Day>,
    /// Post automatically on the due date, or just remind.
    pub auto_post: bool,
    pub enabled: bool,
    /// True when this was detected from history rather than entered.
    pub detected: bool,
}

// ---------------------------------------------------------------- ledger

/// The whole file, in memory.
///
/// Everything is loaded: a hundred thousand transactions is about 20 MB of
/// `Transaction`, and holding them means every filter, sort and report is a
/// pass over a `Vec` at memory speed rather than a round trip through SQL.
/// SQLite remains the file format and the durable store — this is a cache
/// that is rebuilt on load and kept in step on every write.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    pub accounts: Vec<Account>,
    pub categories: CategoryTree,
    pub transactions: Vec<Transaction>,
    pub payees: Vec<Payee>,
    pub rules: Vec<Rule>,
    pub budgets: Vec<BudgetEntry>,
    pub scheduled: Vec<Scheduled>,
    pub base_currency: Currency,
}

impl Ledger {
    pub fn account(&self, id: Id) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn account_name(&self, id: Id) -> &str {
        self.account(id).map(|a| a.name.as_str()).unwrap_or("")
    }

    pub fn transaction(&self, id: Id) -> Option<&Transaction> {
        self.transactions.iter().find(|t| t.id == id)
    }

    /// Balance of an account as of a day (inclusive), opening balance
    /// included. This is the number in the sidebar.
    pub fn balance_on(&self, account: Id, day: Day) -> i64 {
        let opening = self.account(account).map(|a| a.opening_balance).unwrap_or(0);
        opening
            + self
                .transactions
                .iter()
                .filter(|t| t.account == account && t.date <= day)
                .map(|t| t.amount)
                .sum::<i64>()
    }

    pub fn balance(&self, account: Id) -> i64 {
        self.balance_on(account, Day::MAX)
    }

    /// What the bank thinks you have: cleared and reconciled rows only.
    /// The gap between this and [`Ledger::balance`] is money in flight.
    pub fn cleared_balance(&self, account: Id) -> i64 {
        let opening = self.account(account).map(|a| a.opening_balance).unwrap_or(0);
        opening
            + self
                .transactions
                .iter()
                .filter(|t| t.account == account && t.cleared != Cleared::Uncleared)
                .map(|t| t.amount)
                .sum::<i64>()
    }

    /// Assets minus debts across every on-budget account, as of a day.
    pub fn net_worth_on(&self, day: Day) -> i64 {
        self.accounts
            .iter()
            .filter(|a| !a.off_budget)
            .map(|a| self.balance_on(a.id, day))
            .sum()
    }

    /// A transfer pair is balanced when its two rows cancel out. An
    /// unbalanced pair means an edit went wrong, and the UI says so rather
    /// than quietly showing a net-worth number that is wrong.
    pub fn transfer_is_balanced(&self, group: Id) -> bool {
        let sum: i64 = self
            .transactions
            .iter()
            .filter(|t| t.transfer_group == Some(group))
            .map(|t| t.amount)
            .sum();
        sum == 0
    }

    /// Every transaction of an account, oldest first, with the running
    /// balance after it — the ledger's most-used view.
    pub fn register(&self, account: Id) -> Vec<(&Transaction, i64)> {
        let mut rows: Vec<&Transaction> =
            self.transactions.iter().filter(|t| t.account == account).collect();
        // Same-day rows need a stable tiebreak or the running balance
        // jitters between loads; the id is insertion order, which is the
        // order they were entered or imported in.
        rows.sort_by_key(|t| (t.date, t.id));
        let mut balance = self.account(account).map(|a| a.opening_balance).unwrap_or(0);
        rows.into_iter()
            .map(|t| {
                balance += t.amount;
                (t, balance)
            })
            .collect()
    }

    pub fn uncategorized_count(&self) -> usize {
        self.transactions
            .iter()
            .filter(|t| t.category.is_none() && t.splits.is_empty() && !t.is_transfer())
            .count()
    }

    /// The next id to hand out for a table, for in-memory work before a
    /// write reaches the database.
    pub fn next_transaction_id(&self) -> Id {
        self.transactions.iter().map(|t| t.id).max().unwrap_or(0) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::from_ymd;
    use crate::money::USD;

    fn ledger() -> Ledger {
        let mut ledger = Ledger { base_currency: USD, ..Ledger::default() };
        let mut checking = Account::new("Checking", AccountKind::Checking, USD);
        checking.id = 1;
        checking.opening_balance = 100_000; // $1,000
        let mut card = Account::new("Card", AccountKind::CreditCard, USD);
        card.id = 2;
        ledger.accounts.push(checking);
        ledger.accounts.push(card);
        ledger
    }

    #[test]
    fn balances_include_the_opening_balance_and_respect_dates() {
        let mut l = ledger();
        let mut t = Transaction::new(1, from_ymd(2024, 3, 4), "Rent", -150_000);
        t.id = 1;
        l.transactions.push(t);
        assert_eq!(l.balance(1), -50_000);
        assert_eq!(l.balance_on(1, from_ymd(2024, 3, 3)), 100_000);
        assert_eq!(l.balance_on(1, from_ymd(2024, 3, 4)), -50_000);
    }

    #[test]
    fn cleared_balance_is_what_the_bank_shows() {
        let mut l = ledger();
        let mut posted = Transaction::new(1, from_ymd(2024, 3, 1), "Salary", 300_000);
        posted.id = 1;
        posted.cleared = Cleared::Cleared;
        let mut pending = Transaction::new(1, from_ymd(2024, 3, 2), "Coffee", -450);
        pending.id = 2;
        l.transactions.push(posted);
        l.transactions.push(pending);
        assert_eq!(l.balance(1), 399_550);
        assert_eq!(l.cleared_balance(1), 400_000);
    }

    #[test]
    fn a_transfer_pair_cancels_out_and_leaves_net_worth_alone() {
        let mut l = ledger();
        let mut out = Transaction::new(1, from_ymd(2024, 3, 4), "Card payment", -50_000);
        out.id = 1;
        out.transfer_group = Some(7);
        let mut into = Transaction::new(2, from_ymd(2024, 3, 4), "Payment received", 50_000);
        into.id = 2;
        into.transfer_group = Some(7);
        let before = l.net_worth_on(Day::MAX);
        l.transactions.push(out);
        l.transactions.push(into);
        assert!(l.transfer_is_balanced(7));
        assert_eq!(l.net_worth_on(Day::MAX), before);

        // Break one side: the ledger must be able to say so.
        l.transactions[1].amount = 40_000;
        assert!(!l.transfer_is_balanced(7));
    }

    #[test]
    fn splits_must_sum_to_the_transaction() {
        let mut t = Transaction::new(1, from_ymd(2024, 3, 4), "Supermarket", -10_000);
        assert_eq!(t.split_imbalance(), 0); // no splits: nothing to balance
        t.splits = vec![
            Split { id: 1, category: Some(1), amount: -7_000, memo: String::new() },
            Split { id: 2, category: Some(2), amount: -2_000, memo: String::new() },
        ];
        assert_eq!(t.split_imbalance(), -1_000);
        t.splits.push(Split { id: 3, category: Some(3), amount: -1_000, memo: String::new() });
        assert_eq!(t.split_imbalance(), 0);
        assert_eq!(t.category_amounts().len(), 3);
    }

    #[test]
    fn the_running_balance_is_stable_for_same_day_rows() {
        let mut l = ledger();
        for (id, amount) in [(1, -1_000), (2, -2_000), (3, 5_000)] {
            let mut t = Transaction::new(1, from_ymd(2024, 3, 4), "x", amount);
            t.id = id;
            l.transactions.push(t);
        }
        let first: Vec<i64> = l.register(1).iter().map(|(_, b)| *b).collect();
        // Same input, reversed insertion: the register must not change.
        l.transactions.reverse();
        let second: Vec<i64> = l.register(1).iter().map(|(_, b)| *b).collect();
        assert_eq!(first, second);
        assert_eq!(*first.last().unwrap(), l.balance(1));
    }

    #[test]
    fn import_fingerprints_survive_cosmetic_differences_only() {
        let a = import_fingerprint(1, 100, -450, "SQ *BLUE BOTTLE  0123");
        let b = import_fingerprint(1, 100, -450, "sq *blue bottle 0123");
        assert_eq!(a, b, "case and spacing must not change the fingerprint");

        let different_amount = import_fingerprint(1, 100, -451, "SQ *BLUE BOTTLE 0123");
        let different_day = import_fingerprint(1, 101, -450, "SQ *BLUE BOTTLE 0123");
        let different_account = import_fingerprint(2, 100, -450, "SQ *BLUE BOTTLE 0123");
        assert_ne!(a, different_amount);
        assert_ne!(a, different_day);
        assert_ne!(a, different_account);
    }

    #[test]
    fn rules_match_the_way_a_person_expects() {
        let rule = Rule {
            id: 1,
            name: "Coffee".into(),
            match_on: MatchOn::Raw,
            how: MatchHow::Contains,
            pattern: "blue bottle".into(),
            amount_min: 0,
            amount_max: 0,
            set_category: Some(9),
            rename_payee: Some("Blue Bottle".into()),
            set_memo: None,
            flag: false,
            priority: 0,
            enabled: true,
            hits: 0,
        };
        assert!(rule.matches("", "", "SQ *BLUE BOTTLE 0123", -450));
        assert!(!rule.matches("", "", "STARBUCKS", -450));

        let disabled = Rule { enabled: false, ..rule.clone() };
        assert!(!disabled.matches("", "", "SQ *BLUE BOTTLE 0123", -450));

        let big = Rule {
            match_on: MatchOn::Amount,
            how: MatchHow::AmountBetween,
            amount_min: -100_000,
            amount_max: -50_000,
            ..rule
        };
        assert!(big.matches("", "", "", -75_000));
        assert!(!big.matches("", "", "", -10_000));
    }

    #[test]
    fn category_paths_and_budget_order_read_like_the_screen() {
        let mut tree = CategoryTree::default();
        let mut food = Category::group("Food", CategoryKind::Expense);
        food.id = 1;
        let mut groceries = Category::child("Groceries", 1, CategoryKind::Expense);
        groceries.id = 2;
        groceries.budgeted = true;
        let mut income = Category::group("Income", CategoryKind::Income);
        income.id = 3;
        tree.categories = vec![food, groceries, income];
        assert_eq!(tree.path(2), "Food: Groceries");
        assert_eq!(tree.path(1), "Food");
        assert_eq!(tree.group_of(2), Some(1));
        // Income groups sort above expense groups.
        let order: Vec<&str> =
            tree.budget_order().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(order, ["Income", "Food", "Groceries"]);
    }

    #[test]
    fn recurrence_clamps_month_ends_instead_of_skipping() {
        let jan31 = from_ymd(2024, 1, 31);
        assert_eq!(Recurrence::Monthly.next(jan31), from_ymd(2024, 2, 29));
        assert_eq!(Recurrence::Weekly.next(jan31), from_ymd(2024, 2, 7));
        assert_eq!(Recurrence::Yearly.next(jan31), from_ymd(2025, 1, 31));
    }

    #[test]
    fn budget_lines_classify_themselves() {
        let over = BudgetLine { assigned: 10_000, spent: 12_000, carried: 0, available: -2_000 };
        assert_eq!(over.state(), BudgetState::Overspent);
        let untouched = BudgetLine::default();
        assert_eq!(untouched.state(), BudgetState::Untouched);
        let on_track = BudgetLine { assigned: 10_000, spent: 4_000, carried: 0, available: 6_000 };
        assert_eq!(on_track.state(), BudgetState::OnTrack);
    }
}
