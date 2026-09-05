//! Native SQLite persistence and statement import.

use crate::date;
use crate::db::Db;
use crate::model::*;
use crate::runtime::{ImportState, Runtime, Start};
use makepad_widgets::makepad_platform::file_dialogs::{FileDialog, FileDialogAction};
use makepad_widgets::*;
use std::collections::{BTreeSet, HashMap};

const PICK_STATEMENT: LiveId = live_id!(finance_pick_statement);

#[derive(Default)]
pub(crate) struct Backend {
    db: Option<Db>,
}

impl Runtime for Backend {
    fn start(&mut self) -> Start {
        let today = date::today();
        let path = std::path::PathBuf::from("local/finance/finance.db");
        let mut db = match Db::open(&path) {
            Ok(db) => db,
            Err(error) => {
                return Start {
                    today,
                    ledger: Ledger::default(),
                    status: format!("cannot open {}: {error}", path.display()),
                };
            }
        };

        let mut status = String::new();
        match db.is_empty() {
            Ok(true) => {
                let ledger = crate::seed::generate(crate::seed::DEFAULT_YEARS, today);
                match persist(&mut db, &ledger) {
                    Ok(_) => {
                        let start = date::month_start(date::add_months(
                            today,
                            -(crate::seed::DEFAULT_YEARS * 12 - 1),
                        ));
                        status = format!(
                            "Demo file created: {} transactions across {} accounts, {} to {}",
                            ledger.transactions.len(),
                            ledger.accounts.len(),
                            date::format_short(start),
                            date::format_short(today)
                        );
                    }
                    Err(error) => status = format!("demo data failed: {error}"),
                }
            }
            Ok(false) => {}
            Err(error) => status = format!("cannot read {}: {error}", path.display()),
        }
        let ledger = match db.load() {
            Ok(ledger) => ledger,
            Err(error) => {
                status = format!("load failed: {error}");
                Ledger::default()
            }
        };
        self.db = Some(db);
        Start { today, ledger, status }
    }

    fn has_import(&self) -> bool {
        true
    }

    fn pick_statement(&mut self, cx: &mut Cx) {
        let dialog = FileDialog::new()
            .set_id(PICK_STATEMENT)
            .set_title("Choose a statement".to_string())
            .add_filter("Comma-separated values".to_string(), vec!["csv".to_string()])
            .add_filter("Text".to_string(), vec!["txt".to_string()])
            .add_filter("All Files".to_string(), vec!["*".to_string()]);
        cx.open_select_file_dialog(dialog);
    }

    fn prepare_from_actions(
        &mut self,
        actions: &Actions,
        ledger: &Ledger,
        account_filter: Option<Id>,
    ) -> Option<Result<ImportState, String>> {
        for action in actions {
            let Some(picked) = action.downcast_ref::<FileDialogAction>() else { continue };
            if picked.id() == PICK_STATEMENT {
                if let Some(path) = picked.path() {
                    return Some(self.prepare_import(path, ledger, account_filter));
                }
            }
        }
        None
    }

    fn commit_import(&mut self, state: ImportState) -> Result<(Ledger, String), String> {
        let db = self.db.as_mut().ok_or_else(|| "database is not open".to_string())?;
        let rows: Vec<Transaction> = state.plan.to_import().cloned().collect();
        let count = rows.len();
        db.transact(|conn| {
            for txn in &rows {
                crate::db::insert_transaction_on(conn, txn)?;
            }
            Ok(())
        })?;
        let ledger = db.load()?;
        let status = format!("Imported {count} transactions from {}", state.path);
        Ok((ledger, status))
    }
}

impl Backend {
    fn prepare_import(
        &mut self,
        path: &std::path::Path,
        ledger: &Ledger,
        account_filter: Option<Id>,
    ) -> Result<ImportState, String> {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let csv = crate::csv::parse(&String::from_utf8_lossy(&bytes));
        let guess = crate::import::Mapping::guess(&csv);
        let account = account_filter
            .or_else(|| ledger.accounts.first().map(|account| account.id))
            .ok_or_else(|| "no account to import into".to_string())?;
        let account = ledger
            .account(account)
            .ok_or_else(|| "no account to import into".to_string())?;
        let known = self
            .db
            .as_mut()
            .ok_or_else(|| "database is not open".to_string())?
            .known_fingerprints()?;
        let plan = crate::import::plan(&csv, &guess.mapping, account, &ledger.rules, &known);
        Ok(ImportState {
            path: path.display().to_string(),
            plan,
            ask_date_order: guess.ask_date_order,
        })
    }
}

#[derive(Default)]
struct PersistedIds {
    accounts: HashMap<Id, Id>,
    categories: HashMap<Id, Id>,
    payees: HashMap<Id, Id>,
    transactions: HashMap<Id, Id>,
    splits: HashMap<Id, Id>,
    transfer_groups: HashMap<Id, Id>,
    rules: HashMap<Id, Id>,
    scheduled: HashMap<Id, Id>,
}

fn mapped(ids: &HashMap<Id, Id>, old: Id, kind: &str) -> Result<Id, String> {
    ids.get(&old)
        .copied()
        .ok_or_else(|| format!("missing {kind} id {old} while persisting generated ledger"))
}

fn mapped_opt(
    ids: &HashMap<Id, Id>,
    old: Option<Id>,
    kind: &str,
) -> Result<Option<Id>, String> {
    old.map(|id| mapped(ids, id, kind)).transpose()
}

/// Persist a generated ledger while translating every local id to the id
/// SQLite assigned. Keeping the maps explicit prevents insertion order from
/// leaking into parent links or any downstream reference.
fn persist(db: &mut Db, ledger: &Ledger) -> Result<PersistedIds, String> {
    let mut ids = PersistedIds::default();

    for account in &ledger.accounts {
        ids.accounts.insert(account.id, db.insert_account(account)?);
    }

    // Parents must be assigned before their children, regardless of the
    // display order in the generated vector.
    let mut categories: Vec<&Category> = ledger.categories.categories.iter().collect();
    while !categories.is_empty() {
        let Some(index) = categories.iter().position(|category| {
            category.parent.is_none_or(|parent| ids.categories.contains_key(&parent))
        }) else {
            return Err("category tree contains a missing or cyclic parent".to_string());
        };
        let category = categories.remove(index);
        let mut stored = category.clone();
        stored.parent = mapped_opt(&ids.categories, category.parent, "category parent")?;
        ids.categories.insert(category.id, db.insert_category(&stored)?);
    }

    for payee in &ledger.payees {
        let mut stored = payee.clone();
        stored.default_category =
            mapped_opt(&ids.categories, payee.default_category, "payee category")?;
        ids.payees.insert(payee.id, db.insert_payee(&stored)?);
    }

    let groups: BTreeSet<Id> =
        ledger.transactions.iter().filter_map(|txn| txn.transfer_group).collect();
    for (index, group) in groups.into_iter().enumerate() {
        ids.transfer_groups.insert(group, index as Id + 1);
    }

    for txn in &ledger.transactions {
        let mut stored = txn.clone();
        stored.account = mapped(&ids.accounts, txn.account, "transaction account")?;
        stored.category = mapped_opt(&ids.categories, txn.category, "transaction category")?;
        stored.transfer_group =
            mapped_opt(&ids.transfer_groups, txn.transfer_group, "transfer group")?;
        for split in &mut stored.splits {
            split.category = mapped_opt(&ids.categories, split.category, "split category")?;
        }
        let (transaction_id, split_ids) = db.insert_transaction_with_ids(&stored)?;
        ids.transactions.insert(txn.id, transaction_id);
        for (split, stored_id) in txn.splits.iter().zip(split_ids) {
            ids.splits.insert(split.id, stored_id);
        }
    }

    for budget in &ledger.budgets {
        let mut stored = *budget;
        stored.category = mapped(&ids.categories, budget.category, "budget category")?;
        db.insert_budget(&stored)?;
    }
    for rule in &ledger.rules {
        let mut stored = rule.clone();
        stored.set_category = mapped_opt(&ids.categories, rule.set_category, "rule category")?;
        ids.rules.insert(rule.id, db.insert_rule(&stored)?);
    }
    for scheduled in &ledger.scheduled {
        let mut stored = scheduled.clone();
        stored.account = mapped(&ids.accounts, scheduled.account, "scheduled account")?;
        stored.category =
            mapped_opt(&ids.categories, scheduled.category, "scheduled category")?;
        ids.scheduled.insert(scheduled.id, db.insert_scheduled(&stored)?);
    }
    db.set_setting("base_currency", ledger.base_currency.code)?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("finance-native-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn assert_references_resolve(ledger: &Ledger) {
        for category in &ledger.categories.categories {
            if let Some(parent) = category.parent {
                assert!(ledger.categories.get(parent).is_some(), "missing parent {parent}");
            }
        }
        for txn in &ledger.transactions {
            assert!(ledger.account(txn.account).is_some(), "missing account {}", txn.account);
            if let Some(category) = txn.category {
                assert!(ledger.categories.get(category).is_some(), "missing category {category}");
            }
            for split in &txn.splits {
                if let Some(category) = split.category {
                    assert!(ledger.categories.get(category).is_some(), "missing split category");
                }
            }
        }
        for budget in &ledger.budgets {
            assert!(ledger.categories.get(budget.category).is_some(), "missing budget category");
        }
        for payee in &ledger.payees {
            if let Some(category) = payee.default_category {
                assert!(ledger.categories.get(category).is_some(), "missing payee category");
            }
        }
        for rule in &ledger.rules {
            if let Some(category) = rule.set_category {
                assert!(ledger.categories.get(category).is_some(), "missing rule category");
            }
        }
        for item in &ledger.scheduled {
            assert!(ledger.account(item.account).is_some(), "missing scheduled account");
            if let Some(category) = item.category {
                assert!(ledger.categories.get(category).is_some(), "missing scheduled category");
            }
        }
    }

    #[test]
    fn generated_ledger_round_trips_through_sqlite_with_all_links_remapped() {
        let path = temp_path("generated-roundtrip");
        let generated = crate::seed::generate(2, date::from_ymd(2026, 8, 28));
        let mut db = Db::open(&path).expect("open temp database");
        let ids = persist(&mut db, &generated).expect("persist generated ledger");
        let loaded = db.load().expect("load generated ledger");

        assert_eq!(loaded.accounts.len(), generated.accounts.len());
        assert_eq!(loaded.categories.categories.len(), generated.categories.categories.len());
        assert_eq!(loaded.transactions.len(), generated.transactions.len());
        assert_eq!(
            loaded.transactions.iter().map(|txn| txn.splits.len()).sum::<usize>(),
            generated.transactions.iter().map(|txn| txn.splits.len()).sum::<usize>()
        );
        assert_eq!(loaded.payees.len(), generated.payees.len());
        assert_eq!(loaded.budgets.len(), generated.budgets.len());
        assert_eq!(loaded.rules.len(), generated.rules.len());
        assert_eq!(loaded.scheduled.len(), generated.scheduled.len());
        assert_references_resolve(&loaded);

        for account in &generated.accounts {
            let loaded_id = ids.accounts[&account.id];
            let mut expected = account.clone();
            expected.id = loaded_id;
            let actual = loaded.account(loaded_id).expect("mapped account");
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
            assert_eq!(loaded.balance(loaded_id), generated.balance(account.id), "{}", account.name);
        }
        for category in &generated.categories.categories {
            let mut expected = category.clone();
            expected.id = ids.categories[&category.id];
            expected.parent = category.parent.map(|parent| ids.categories[&parent]);
            let actual = loaded.categories.get(expected.id).expect("mapped category");
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        }
        for txn in &generated.transactions {
            let mut expected = txn.clone();
            expected.id = ids.transactions[&txn.id];
            expected.account = ids.accounts[&txn.account];
            expected.category = txn.category.map(|category| ids.categories[&category]);
            expected.transfer_group =
                txn.transfer_group.map(|group| ids.transfer_groups[&group]);
            for split in &mut expected.splits {
                split.id = ids.splits[&split.id];
                split.category = split.category.map(|category| ids.categories[&category]);
            }
            let actual = loaded.transaction(expected.id).expect("mapped transaction");
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        }
        for budget in &generated.budgets {
            let mut expected = *budget;
            expected.category = ids.categories[&budget.category];
            let actual = loaded
                .budgets
                .iter()
                .find(|item| item.category == expected.category && item.month == expected.month)
                .expect("mapped budget");
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        }
        for rule in &generated.rules {
            let mut expected = rule.clone();
            expected.id = ids.rules[&rule.id];
            expected.set_category = rule.set_category.map(|category| ids.categories[&category]);
            let actual = loaded.rules.iter().find(|item| item.id == expected.id).expect("mapped rule");
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        }
        for item in &generated.scheduled {
            let mut expected = item.clone();
            expected.id = ids.scheduled[&item.id];
            expected.account = ids.accounts[&item.account];
            expected.category = item.category.map(|category| ids.categories[&category]);
            let actual = loaded
                .scheduled
                .iter()
                .find(|candidate| candidate.id == expected.id)
                .expect("mapped scheduled entry");
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        }

        let mut generated_tree: Vec<_> = generated
            .categories
            .categories
            .iter()
            .map(|category| {
                (
                    category.name.clone(),
                    category.parent.map(|parent| generated.categories.name(parent).to_string()),
                )
            })
            .collect();
        let mut loaded_tree: Vec<_> = loaded
            .categories
            .categories
            .iter()
            .map(|category| {
                (
                    category.name.clone(),
                    category.parent.map(|parent| loaded.categories.name(parent).to_string()),
                )
            })
            .collect();
        generated_tree.sort();
        loaded_tree.sort();
        assert_eq!(loaded_tree, generated_tree);

        assert_eq!(ids.accounts.len(), generated.accounts.len());
        assert_eq!(ids.categories.len(), generated.categories.categories.len());
        assert_eq!(ids.payees.len(), generated.payees.len());
        assert_eq!(ids.transactions.len(), generated.transactions.len());
        assert_eq!(
            ids.splits.len(),
            generated.transactions.iter().map(|txn| txn.splits.len()).sum::<usize>()
        );
        assert_eq!(ids.rules.len(), generated.rules.len());
        assert_eq!(ids.scheduled.len(), generated.scheduled.len());
        assert_eq!(
            ids.transfer_groups.len(),
            generated
                .transactions
                .iter()
                .filter_map(|txn| txn.transfer_group)
                .collect::<BTreeSet<_>>()
                .len()
        );

        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn native_backend_advertises_import() {
        assert!(Backend::default().has_import());
    }

    #[test]
    fn payee_category_reference_is_remapped() {
        let path = temp_path("payee-remap");
        let mut ledger = Ledger::default();
        let mut category = Category::group("Food", CategoryKind::Expense);
        category.id = 40;
        ledger.categories.categories.push(category);
        ledger.payees.push(Payee {
            id: 90,
            name: "Market".to_string(),
            default_category: Some(40),
            transactions: 0,
        });

        let mut db = Db::open(&path).expect("open temp database");
        let ids = persist(&mut db, &ledger).expect("persist payee");
        let loaded = db.load().expect("load payee");
        assert_eq!(loaded.payees.len(), 1);
        assert_eq!(loaded.payees[0].id, ids.payees[&90]);
        assert_eq!(loaded.payees[0].default_category, Some(ids.categories[&40]));

        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
