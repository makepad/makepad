//! The SQLite file, and the load that turns it into a [`Ledger`].
//!
//! The database is the file format — one `.finance` file you can copy,
//! back up, and open with any SQLite tool, which is the whole reason for
//! choosing it over a private binary. But no screen queries it. Everything
//! is read once into memory ([`Ledger`]) and written through on change,
//! because the queries a finance app makes — "every transaction of this
//! account with its running balance", "spend per category per month for
//! two years" — are passes over a few hundred thousand small rows, and
//! that is microseconds in RAM against milliseconds per round trip in SQL.
//! It is also why this app stays fast where the commercial ones famously
//! do not: the reports never touch the disk.
//!
//! Schema changes go in [`MIGRATIONS`], never by editing [`SCHEMA`]: an
//! existing file must survive an upgrade. `user_version` records how far a
//! file has come.

use crate::date::Day;
use crate::model::*;
use crate::money::{currency_by_code, Currency, USD};
use makepad_sqlite::{Connection, Value};
use std::path::Path;
use std::time::Duration;

/// Bumped whenever [`MIGRATIONS`] grows.
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    currency TEXT NOT NULL DEFAULT 'USD',
    institution TEXT NOT NULL DEFAULT '',
    opening_balance INTEGER NOT NULL DEFAULT 0,
    opening_date INTEGER NOT NULL DEFAULT 0,
    closed INTEGER NOT NULL DEFAULT 0,
    off_budget INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0,
    note TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS categories(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent INTEGER,
    kind TEXT NOT NULL DEFAULT 'expense',
    budgeted INTEGER NOT NULL DEFAULT 1,
    rollover INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0,
    color INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS transactions(
    id INTEGER PRIMARY KEY,
    account INTEGER NOT NULL,
    date INTEGER NOT NULL,
    payee TEXT NOT NULL DEFAULT '',
    memo TEXT NOT NULL DEFAULT '',
    amount INTEGER NOT NULL,
    category INTEGER,
    transfer_group INTEGER,
    cleared TEXT NOT NULL DEFAULT 'uncleared',
    statement INTEGER,
    import_hash INTEGER,
    reference TEXT NOT NULL DEFAULT '',
    flagged INTEGER NOT NULL DEFAULT 0,
    notes TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS transactions_account_date ON transactions(account, date);
CREATE INDEX IF NOT EXISTS transactions_date ON transactions(date);
CREATE INDEX IF NOT EXISTS transactions_import_hash ON transactions(import_hash);
CREATE TABLE IF NOT EXISTS splits(
    id INTEGER PRIMARY KEY,
    txn INTEGER NOT NULL,
    category INTEGER,
    amount INTEGER NOT NULL,
    memo TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS splits_txn ON splits(txn);
CREATE TABLE IF NOT EXISTS payees(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    default_category INTEGER
);
CREATE TABLE IF NOT EXISTS rules(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    match_on TEXT NOT NULL DEFAULT 'raw',
    how TEXT NOT NULL DEFAULT 'contains',
    pattern TEXT NOT NULL DEFAULT '',
    amount_min INTEGER NOT NULL DEFAULT 0,
    amount_max INTEGER NOT NULL DEFAULT 0,
    set_category INTEGER,
    rename_payee TEXT,
    set_memo TEXT,
    flag INTEGER NOT NULL DEFAULT 0,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    hits INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS budgets(
    category INTEGER NOT NULL,
    month INTEGER NOT NULL,
    assigned INTEGER NOT NULL DEFAULT 0,
    rollover INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(category, month)
);
CREATE TABLE IF NOT EXISTS scheduled(
    id INTEGER PRIMARY KEY,
    account INTEGER NOT NULL,
    payee TEXT NOT NULL DEFAULT '',
    amount INTEGER NOT NULL DEFAULT 0,
    category INTEGER,
    recurrence TEXT NOT NULL DEFAULT 'monthly',
    next_due INTEGER NOT NULL DEFAULT 0,
    last_posted INTEGER,
    auto_post INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    detected INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS import_profiles(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    account INTEGER,
    mapping TEXT NOT NULL DEFAULT '',
    date_order TEXT NOT NULL DEFAULT 'ymd',
    decimal_comma INTEGER NOT NULL DEFAULT 0,
    delimiter TEXT NOT NULL DEFAULT ',',
    used INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS settings(
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Each entry runs once, in order, on a file whose `user_version` is below
/// its index + 1. Append only — never edit one that has shipped.
const MIGRATIONS: [&str; 0] = [];

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (creating if absent) and bring the schema up to date.
    pub fn open(path: &Path) -> Result<Db, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        let mut conn = Connection::open(path, Duration::from_secs(5))
            .map_err(|e| format!("open {}: {e:?}", path.display()))?;
        // A ledger is one table scan wide, not a hundred: the default row
        // budget would refuse a decade of transactions in one query.
        conn.limits_mut().max_rows = 5_000_000;
        conn.limits_mut().max_steps = 500_000_000;
        conn.execute_batch(SCHEMA).map_err(|e| format!("schema: {e:?}"))?;
        let mut db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<(), String> {
        let from = self.conn.user_version().max(0) as usize;
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(from) {
            self.conn
                .execute_batch(sql)
                .map_err(|e| format!("migration {}: {e:?}", index + 1))?;
        }
        if from < MIGRATIONS.len() {
            self.conn
                .execute(&format!("PRAGMA user_version = {}", MIGRATIONS.len()), &[])
                .map_err(|e| format!("set user_version: {e:?}"))?;
        }
        Ok(())
    }

    /// True for a file with no accounts — a first run, which is what the
    /// demo data offers to fill.
    pub fn is_empty(&mut self) -> Result<bool, String> {
        let result = self
            .conn
            .query("SELECT COUNT(*) FROM accounts", &[])
            .map_err(|e| format!("count accounts: {e:?}"))?;
        Ok(result.scalar().and_then(|v| v.as_integer()).unwrap_or(0) == 0)
    }

    /// Read the whole file into memory. Every screen reads the result;
    /// nothing else queries.
    pub fn load(&mut self) -> Result<Ledger, String> {
        let mut ledger = Ledger { base_currency: self.base_currency(), ..Ledger::default() };

        let rows = self
            .conn
            .query(
                "SELECT id, name, kind, currency, institution, opening_balance, opening_date, \
                 closed, off_budget, sort_order, note FROM accounts ORDER BY sort_order, id",
                &[],
            )
            .map_err(|e| format!("load accounts: {e:?}"))?;
        for row in &rows.rows {
            ledger.accounts.push(Account {
                id: int(&row[0]),
                name: text(&row[1]),
                kind: AccountKind::from_str(&text(&row[2])),
                currency: currency_by_code(&text(&row[3])).unwrap_or(USD),
                institution: text(&row[4]),
                opening_balance: int(&row[5]),
                opening_date: int(&row[6]) as Day,
                closed: int(&row[7]) != 0,
                off_budget: int(&row[8]) != 0,
                sort_order: int(&row[9]) as i32,
                note: text(&row[10]),
            });
        }

        let rows = self
            .conn
            .query(
                "SELECT id, name, parent, kind, budgeted, rollover, sort_order, color, hidden \
                 FROM categories ORDER BY sort_order, id",
                &[],
            )
            .map_err(|e| format!("load categories: {e:?}"))?;
        for row in &rows.rows {
            ledger.categories.categories.push(Category {
                id: int(&row[0]),
                name: text(&row[1]),
                parent: opt_int(&row[2]),
                kind: CategoryKind::from_str(&text(&row[3])),
                budgeted: int(&row[4]) != 0,
                rollover: int(&row[5]) != 0,
                sort_order: int(&row[6]) as i32,
                color: int(&row[7]) as u32,
                hidden: int(&row[8]) != 0,
            });
        }

        let rows = self
            .conn
            .query(
                "SELECT id, account, date, payee, memo, amount, category, transfer_group, \
                 cleared, statement, import_hash, reference, flagged, notes \
                 FROM transactions ORDER BY date, id",
                &[],
            )
            .map_err(|e| format!("load transactions: {e:?}"))?;
        ledger.transactions.reserve(rows.rows.len());
        for row in &rows.rows {
            ledger.transactions.push(Transaction {
                id: int(&row[0]),
                account: int(&row[1]),
                date: int(&row[2]) as Day,
                payee: text(&row[3]),
                memo: text(&row[4]),
                amount: int(&row[5]),
                category: opt_int(&row[6]),
                splits: Vec::new(),
                transfer_group: opt_int(&row[7]),
                cleared: Cleared::from_str(&text(&row[8])),
                statement: opt_int(&row[9]),
                import_hash: opt_int(&row[10]),
                reference: text(&row[11]),
                flagged: int(&row[12]) != 0,
                notes: text(&row[13]),
            });
        }

        // Splits come back in one query and are distributed by id, rather
        // than a query per transaction.
        let rows = self
            .conn
            .query("SELECT id, txn, category, amount, memo FROM splits ORDER BY txn, id", &[])
            .map_err(|e| format!("load splits: {e:?}"))?;
        if !rows.rows.is_empty() {
            let mut index: std::collections::HashMap<Id, usize> =
                std::collections::HashMap::with_capacity(ledger.transactions.len());
            for (position, txn) in ledger.transactions.iter().enumerate() {
                index.insert(txn.id, position);
            }
            for row in &rows.rows {
                let Some(position) = index.get(&int(&row[1])) else { continue };
                ledger.transactions[*position].splits.push(Split {
                    id: int(&row[0]),
                    category: opt_int(&row[2]),
                    amount: int(&row[3]),
                    memo: text(&row[4]),
                });
            }
        }

        let rows = self
            .conn
            .query("SELECT category, month, assigned, rollover FROM budgets", &[])
            .map_err(|e| format!("load budgets: {e:?}"))?;
        for row in &rows.rows {
            ledger.budgets.push(BudgetEntry {
                category: int(&row[0]),
                month: int(&row[1]) as i32,
                assigned: int(&row[2]),
                rollover: int(&row[3]) != 0,
            });
        }

        let rows = self
            .conn
            .query(
                "SELECT id, name, match_on, how, pattern, amount_min, amount_max, set_category, \
                 rename_payee, set_memo, flag, priority, enabled, hits FROM rules \
                 ORDER BY priority, id",
                &[],
            )
            .map_err(|e| format!("load rules: {e:?}"))?;
        for row in &rows.rows {
            ledger.rules.push(Rule {
                id: int(&row[0]),
                name: text(&row[1]),
                match_on: match_on_from_str(&text(&row[2])),
                how: match_how_from_str(&text(&row[3])),
                pattern: text(&row[4]),
                amount_min: int(&row[5]),
                amount_max: int(&row[6]),
                set_category: opt_int(&row[7]),
                rename_payee: opt_text(&row[8]),
                set_memo: opt_text(&row[9]),
                flag: int(&row[10]) != 0,
                priority: int(&row[11]) as i32,
                enabled: int(&row[12]) != 0,
                hits: int(&row[13]),
            });
        }

        let rows = self
            .conn
            .query(
                "SELECT id, account, payee, amount, category, recurrence, next_due, last_posted, \
                 auto_post, enabled, detected FROM scheduled ORDER BY next_due, id",
                &[],
            )
            .map_err(|e| format!("load scheduled: {e:?}"))?;
        for row in &rows.rows {
            ledger.scheduled.push(Scheduled {
                id: int(&row[0]),
                account: int(&row[1]),
                payee: text(&row[2]),
                amount: int(&row[3]),
                category: opt_int(&row[4]),
                recurrence: recurrence_from_str(&text(&row[5])),
                next_due: int(&row[6]) as Day,
                last_posted: opt_int(&row[7]).map(|v| v as Day),
                auto_post: int(&row[8]) != 0,
                enabled: int(&row[9]) != 0,
                detected: int(&row[10]) != 0,
            });
        }

        let rows = self
            .conn
            .query("SELECT id, name, default_category FROM payees ORDER BY name", &[])
            .map_err(|e| format!("load payees: {e:?}"))?;
        for row in &rows.rows {
            ledger.payees.push(Payee {
                id: int(&row[0]),
                name: text(&row[1]),
                default_category: opt_int(&row[2]),
                transactions: 0,
            });
        }

        Ok(ledger)
    }

    fn base_currency(&mut self) -> Currency {
        self.conn
            .query("SELECT value FROM settings WHERE key = 'base_currency'", &[])
            .ok()
            .and_then(|r| r.scalar().and_then(|v| v.as_text().map(str::to_string)))
            .and_then(|code| currency_by_code(&code))
            .unwrap_or(USD)
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO settings(key, value) VALUES(?, ?) \
                 ON CONFLICT(key) DO UPDATE SET value = ?",
                &[Value::text(key), Value::text(value), Value::text(value)],
            )
            .map(|_| ())
            .map_err(|e| format!("set {key}: {e:?}"))
    }

    /// Run `body` inside one transaction, rolling back if it fails. Every
    /// multi-row write goes through this: a half-written import is worse
    /// than a refused one.
    pub fn transact<T>(
        &mut self,
        body: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        self.conn.execute("BEGIN", &[]).map_err(|e| format!("begin: {e:?}"))?;
        match body(&mut self.conn) {
            Ok(value) => {
                self.conn.execute("COMMIT", &[]).map_err(|e| format!("commit: {e:?}"))?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", &[]);
                Err(error)
            }
        }
    }

    /// Insert one account, returning the id the database assigned.
    pub fn insert_account(&mut self, account: &Account) -> Result<Id, String> {
        self.conn
            .execute(
                "INSERT INTO accounts(name, kind, currency, institution, opening_balance, \
                 opening_date, closed, off_budget, sort_order, note) \
                 VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &[
                    Value::text(account.name.as_str()),
                    Value::text(account.kind.as_str()),
                    Value::text(account.currency.code),
                    Value::text(account.institution.as_str()),
                    Value::Integer(account.opening_balance),
                    Value::Integer(account.opening_date as i64),
                    Value::Integer(account.closed as i64),
                    Value::Integer(account.off_budget as i64),
                    Value::Integer(account.sort_order as i64),
                    Value::text(account.note.as_str()),
                ],
            )
            .map_err(|e| format!("insert account: {e:?}"))?;
        self.last_id("accounts")
    }

    pub fn insert_category(&mut self, category: &Category) -> Result<Id, String> {
        self.conn
            .execute(
                "INSERT INTO categories(name, parent, kind, budgeted, rollover, sort_order, \
                 color, hidden) VALUES(?, ?, ?, ?, ?, ?, ?, ?)",
                &[
                    Value::text(category.name.as_str()),
                    category.parent.map(Value::Integer).unwrap_or(Value::Null),
                    Value::text(category.kind.as_str()),
                    Value::Integer(category.budgeted as i64),
                    Value::Integer(category.rollover as i64),
                    Value::Integer(category.sort_order as i64),
                    Value::Integer(category.color as i64),
                    Value::Integer(category.hidden as i64),
                ],
            )
            .map_err(|e| format!("insert category: {e:?}"))?;
        self.last_id("categories")
    }

    pub fn insert_transaction(&mut self, txn: &Transaction) -> Result<Id, String> {
        insert_transaction_on(&mut self.conn, txn)?;
        let id = self.last_id("transactions")?;
        for split in &txn.splits {
            insert_split_on(&mut self.conn, id, split)?;
        }
        Ok(id)
    }

    pub fn insert_budget(&mut self, entry: &BudgetEntry) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO budgets(category, month, assigned, rollover) VALUES(?, ?, ?, ?) \
                 ON CONFLICT(category, month) DO UPDATE SET assigned = ?, rollover = ?",
                &[
                    Value::Integer(entry.category),
                    Value::Integer(entry.month as i64),
                    Value::Integer(entry.assigned),
                    Value::Integer(entry.rollover as i64),
                    Value::Integer(entry.assigned),
                    Value::Integer(entry.rollover as i64),
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("insert budget: {e:?}"))
    }

    pub fn insert_scheduled(&mut self, item: &Scheduled) -> Result<Id, String> {
        self.conn
            .execute(
                "INSERT INTO scheduled(account, payee, amount, category, recurrence, next_due, \
                 last_posted, auto_post, enabled, detected) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &[
                    Value::Integer(item.account),
                    Value::text(item.payee.as_str()),
                    Value::Integer(item.amount),
                    item.category.map(Value::Integer).unwrap_or(Value::Null),
                    Value::text(recurrence_to_str(item.recurrence)),
                    Value::Integer(item.next_due as i64),
                    item.last_posted.map(|d| Value::Integer(d as i64)).unwrap_or(Value::Null),
                    Value::Integer(item.auto_post as i64),
                    Value::Integer(item.enabled as i64),
                    Value::Integer(item.detected as i64),
                ],
            )
            .map_err(|e| format!("insert scheduled: {e:?}"))?;
        self.last_id("scheduled")
    }

    pub fn insert_rule(&mut self, rule: &Rule) -> Result<Id, String> {
        self.conn
            .execute(
                "INSERT INTO rules(name, match_on, how, pattern, amount_min, amount_max, \
                 set_category, rename_payee, set_memo, flag, priority, enabled, hits) \
                 VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &[
                    Value::text(rule.name.as_str()),
                    Value::text(match_on_to_str(rule.match_on)),
                    Value::text(match_how_to_str(rule.how)),
                    Value::text(rule.pattern.as_str()),
                    Value::Integer(rule.amount_min),
                    Value::Integer(rule.amount_max),
                    rule.set_category.map(Value::Integer).unwrap_or(Value::Null),
                    rule.rename_payee
                        .as_deref()
                        .map(Value::text)
                        .unwrap_or(Value::Null),
                    rule.set_memo.as_deref().map(Value::text).unwrap_or(Value::Null),
                    Value::Integer(rule.flag as i64),
                    Value::Integer(rule.priority as i64),
                    Value::Integer(rule.enabled as i64),
                    Value::Integer(rule.hits),
                ],
            )
            .map_err(|e| format!("insert rule: {e:?}"))?;
        self.last_id("rules")
    }

    /// Update the fields the ledger screen can edit. Splits are replaced
    /// wholesale — there are never more than a handful.
    pub fn update_transaction(&mut self, txn: &Transaction) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE transactions SET account = ?, date = ?, payee = ?, memo = ?, amount = ?, \
                 category = ?, transfer_group = ?, cleared = ?, reference = ?, flagged = ?, \
                 notes = ? WHERE id = ?",
                &[
                    Value::Integer(txn.account),
                    Value::Integer(txn.date as i64),
                    Value::text(txn.payee.as_str()),
                    Value::text(txn.memo.as_str()),
                    Value::Integer(txn.amount),
                    txn.category.map(Value::Integer).unwrap_or(Value::Null),
                    txn.transfer_group.map(Value::Integer).unwrap_or(Value::Null),
                    Value::text(txn.cleared.as_str()),
                    Value::text(txn.reference.as_str()),
                    Value::Integer(txn.flagged as i64),
                    Value::text(txn.notes.as_str()),
                    Value::Integer(txn.id),
                ],
            )
            .map_err(|e| format!("update transaction: {e:?}"))?;
        self.conn
            .execute("DELETE FROM splits WHERE txn = ?", &[Value::Integer(txn.id)])
            .map_err(|e| format!("clear splits: {e:?}"))?;
        for split in &txn.splits {
            insert_split_on(&mut self.conn, txn.id, split)?;
        }
        Ok(())
    }

    pub fn delete_transaction(&mut self, id: Id) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM splits WHERE txn = ?", &[Value::Integer(id)])
            .map_err(|e| format!("delete splits: {e:?}"))?;
        self.conn
            .execute("DELETE FROM transactions WHERE id = ?", &[Value::Integer(id)])
            .map(|_| ())
            .map_err(|e| format!("delete transaction: {e:?}"))
    }

    /// Import fingerprints already in the file, so an import can tell what
    /// it has seen before without re-reading every transaction.
    pub fn known_fingerprints(&mut self) -> Result<std::collections::HashSet<i64>, String> {
        let rows = self
            .conn
            .query(
                "SELECT import_hash FROM transactions WHERE import_hash IS NOT NULL",
                &[],
            )
            .map_err(|e| format!("load fingerprints: {e:?}"))?;
        Ok(rows.rows.iter().filter_map(|r| r[0].as_integer()).collect())
    }

    fn last_id(&mut self, table: &str) -> Result<Id, String> {
        let rows = self
            .conn
            .query(&format!("SELECT MAX(id) FROM {table}"), &[])
            .map_err(|e| format!("last id {table}: {e:?}"))?;
        Ok(rows.scalar().and_then(|v| v.as_integer()).unwrap_or(0))
    }
}

/// Insert on a borrowed connection, so a batch can run inside one
/// [`Db::transact`] without re-borrowing `Db`.
pub fn insert_transaction_on(conn: &mut Connection, txn: &Transaction) -> Result<(), String> {
    conn.execute(
        "INSERT INTO transactions(account, date, payee, memo, amount, category, transfer_group, \
         cleared, statement, import_hash, reference, flagged, notes) \
         VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            Value::Integer(txn.account),
            Value::Integer(txn.date as i64),
            Value::text(txn.payee.as_str()),
            Value::text(txn.memo.as_str()),
            Value::Integer(txn.amount),
            txn.category.map(Value::Integer).unwrap_or(Value::Null),
            txn.transfer_group.map(Value::Integer).unwrap_or(Value::Null),
            Value::text(txn.cleared.as_str()),
            txn.statement.map(Value::Integer).unwrap_or(Value::Null),
            txn.import_hash.map(Value::Integer).unwrap_or(Value::Null),
            Value::text(txn.reference.as_str()),
            Value::Integer(txn.flagged as i64),
            Value::text(txn.notes.as_str()),
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("insert transaction: {e:?}"))
}

fn insert_split_on(conn: &mut Connection, txn: Id, split: &Split) -> Result<(), String> {
    conn.execute(
        "INSERT INTO splits(txn, category, amount, memo) VALUES(?, ?, ?, ?)",
        &[
            Value::Integer(txn),
            split.category.map(Value::Integer).unwrap_or(Value::Null),
            Value::Integer(split.amount),
            Value::text(split.memo.as_str()),
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("insert split: {e:?}"))
}

// ------------------------------------------------------------ value readers

fn int(value: &Value) -> i64 {
    value.as_integer().unwrap_or(0)
}

fn opt_int(value: &Value) -> Option<i64> {
    value.as_integer()
}

fn text(value: &Value) -> String {
    value.as_text().unwrap_or("").to_string()
}

fn opt_text(value: &Value) -> Option<String> {
    value.as_text().map(str::to_string)
}

fn match_on_to_str(value: MatchOn) -> &'static str {
    match value {
        MatchOn::Payee => "payee",
        MatchOn::Memo => "memo",
        MatchOn::Raw => "raw",
        MatchOn::Amount => "amount",
    }
}

fn match_on_from_str(value: &str) -> MatchOn {
    match value {
        "payee" => MatchOn::Payee,
        "memo" => MatchOn::Memo,
        "amount" => MatchOn::Amount,
        _ => MatchOn::Raw,
    }
}

fn match_how_to_str(value: MatchHow) -> &'static str {
    match value {
        MatchHow::Contains => "contains",
        MatchHow::StartsWith => "starts_with",
        MatchHow::Equals => "equals",
        MatchHow::AmountEquals => "amount_equals",
        MatchHow::AmountBetween => "amount_between",
    }
}

fn match_how_from_str(value: &str) -> MatchHow {
    match value {
        "starts_with" => MatchHow::StartsWith,
        "equals" => MatchHow::Equals,
        "amount_equals" => MatchHow::AmountEquals,
        "amount_between" => MatchHow::AmountBetween,
        _ => MatchHow::Contains,
    }
}

fn recurrence_to_str(value: Recurrence) -> &'static str {
    match value {
        Recurrence::Weekly => "weekly",
        Recurrence::Fortnightly => "fortnightly",
        Recurrence::Monthly => "monthly",
        Recurrence::Quarterly => "quarterly",
        Recurrence::Yearly => "yearly",
    }
}

fn recurrence_from_str(value: &str) -> Recurrence {
    match value {
        "weekly" => Recurrence::Weekly,
        "fortnightly" => Recurrence::Fortnightly,
        "quarterly" => Recurrence::Quarterly,
        "yearly" => Recurrence::Yearly,
        _ => Recurrence::Monthly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::from_ymd;
    use crate::money::USD;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("finance-test-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_file_round_trips_the_whole_ledger() {
        let path = temp_path("roundtrip");
        let mut db = Db::open(&path).expect("open");
        assert!(db.is_empty().expect("is_empty"));

        let mut checking = Account::new("Checking", AccountKind::Checking, USD);
        checking.opening_balance = 100_000;
        let account_id = db.insert_account(&checking).expect("account");

        let group = db
            .insert_category(&Category::group("Food", CategoryKind::Expense))
            .expect("group");
        let groceries = db
            .insert_category(&Category::child("Groceries", group, CategoryKind::Expense))
            .expect("child");

        let mut txn = Transaction::new(account_id, from_ymd(2024, 3, 4), "Supermarket", -10_000);
        txn.category = Some(groceries);
        txn.cleared = Cleared::Cleared;
        txn.import_hash = Some(4242);
        txn.splits = vec![
            Split { id: 0, category: Some(groceries), amount: -7_000, memo: "food".into() },
            Split { id: 0, category: Some(group), amount: -3_000, memo: "wine".into() },
        ];
        db.insert_transaction(&txn).expect("txn");

        let ledger = db.load().expect("load");
        assert_eq!(ledger.accounts.len(), 1);
        assert_eq!(ledger.categories.categories.len(), 2);
        assert_eq!(ledger.categories.path(groceries), "Food: Groceries");
        assert_eq!(ledger.transactions.len(), 1);
        let loaded = &ledger.transactions[0];
        assert_eq!(loaded.amount, -10_000);
        assert_eq!(loaded.cleared, Cleared::Cleared);
        assert_eq!(loaded.splits.len(), 2);
        assert_eq!(loaded.split_imbalance(), 0);
        assert_eq!(ledger.balance(account_id), 90_000);
        assert!(!db.is_empty().expect("is_empty"));
        assert!(db.known_fingerprints().expect("hashes").contains(&4242));

        // Reopening reads the same file back.
        drop(db);
        let mut again = Db::open(&path).expect("reopen");
        assert_eq!(again.load().expect("load").transactions.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn edits_and_deletes_reach_the_file() {
        let path = temp_path("edit");
        let mut db = Db::open(&path).expect("open");
        let account = db
            .insert_account(&Account::new("Cash", AccountKind::Cash, USD))
            .expect("account");
        let mut txn = Transaction::new(account, from_ymd(2024, 1, 1), "Kiosk", -500);
        let id = db.insert_transaction(&txn).expect("insert");
        txn.id = id;
        txn.payee = "Newsagent".into();
        txn.amount = -650;
        txn.splits = vec![Split { id: 0, category: None, amount: -650, memo: String::new() }];
        db.update_transaction(&txn).expect("update");

        let ledger = db.load().expect("load");
        assert_eq!(ledger.transactions[0].payee, "Newsagent");
        assert_eq!(ledger.transactions[0].amount, -650);
        assert_eq!(ledger.transactions[0].splits.len(), 1);

        db.delete_transaction(id).expect("delete");
        let ledger = db.load().expect("load");
        assert!(ledger.transactions.is_empty());
        // The split went with it rather than being orphaned.
        let orphans = db
            .conn
            .query("SELECT COUNT(*) FROM splits", &[])
            .expect("count")
            .scalar()
            .and_then(|v| v.as_integer())
            .unwrap_or(-1);
        assert_eq!(orphans, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_failed_batch_rolls_back_whole() {
        let path = temp_path("rollback");
        let mut db = Db::open(&path).expect("open");
        let account = db
            .insert_account(&Account::new("Checking", AccountKind::Checking, USD))
            .expect("account");
        let result: Result<(), String> = db.transact(|conn| {
            let txn = Transaction::new(account, from_ymd(2024, 1, 1), "One", -100);
            insert_transaction_on(conn, &txn)?;
            Err("something went wrong halfway".to_string())
        });
        assert!(result.is_err());
        assert!(db.load().expect("load").transactions.is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
