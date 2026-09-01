//! Every number the screens show, computed from the ledger in memory.
//!
//! These are the reports the commercial products converged on — spending
//! by category, income against expense, net worth over time, category
//! drilldown, merchant ranking, budget available — and they are all one
//! pass over a `Vec<Transaction>`. That is the point: a decade of history
//! is a few hundred thousand structs, so a report is a millisecond and can
//! be recomputed on every keystroke of a filter instead of being cached,
//! invalidated, and got wrong.
//!
//! Two rules run through all of it:
//!
//! * **Transfers are not spending.** Moving money to savings is not an
//!   expense, and paying a credit card is not spending twice. Anything
//!   with a [`Transaction::transfer_group`], or in a
//!   [`CategoryKind::Transfer`] category, is excluded from every
//!   income/expense figure — this is the single most common way a naive
//!   finance report lies.
//! * **Splits are counted per part.** A supermarket trip split between
//!   food and household appears in both categories, for its own share.

use crate::date::{self, Day, DateRange, MonthKey};
use crate::model::*;

/// Positive amounts are money in, negative money out — the ledger's own
/// convention, kept all the way to the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flow {
    pub income: i64,
    /// Positive: what left. (Reports read better when spending counts up.)
    pub expense: i64,
}

impl Flow {
    pub fn net(&self) -> i64 {
        self.income - self.expense
    }
}

/// True when a transaction is real spending or real income, rather than
/// money moving between the user's own pockets.
pub fn is_spending(txn: &Transaction, categories: &CategoryTree) -> bool {
    if txn.transfer_group.is_some() {
        return false;
    }
    match txn.category {
        Some(id) => categories.kind_of(id) != CategoryKind::Transfer,
        // An uncategorized row is still real money; it shows up as
        // "Uncategorized", which is what makes people categorize it.
        None => !txn.splits.is_empty() || true,
    }
}

/// Income and expense for a date range.
pub fn flow(ledger: &Ledger, range: DateRange) -> Flow {
    let mut flow = Flow::default();
    for txn in &ledger.transactions {
        if !range.contains(txn.date) || !is_spending(txn, &ledger.categories) {
            continue;
        }
        if ledger.account(txn.account).is_some_and(|a| a.off_budget) {
            continue;
        }
        if txn.amount >= 0 {
            flow.income += txn.amount;
        } else {
            flow.expense += -txn.amount;
        }
    }
    flow
}

/// Income and expense per month, oldest first — the bars on the reports
/// screen and the shape of the cash-flow chart.
pub fn monthly_flow(ledger: &Ledger, months: i32, today: Day) -> Vec<(MonthKey, Flow)> {
    let first = date::month_key(date::add_months(today, -(months - 1)));
    let mut out: Vec<(MonthKey, Flow)> =
        (0..months).map(|i| (first + i, Flow::default())).collect();
    for txn in &ledger.transactions {
        if !is_spending(txn, &ledger.categories) {
            continue;
        }
        if ledger.account(txn.account).is_some_and(|a| a.off_budget) {
            continue;
        }
        let key = date::month_key(txn.date);
        let Some(index) = key.checked_sub(first).filter(|i| *i >= 0 && *i < months) else {
            continue;
        };
        let slot = &mut out[index as usize].1;
        if txn.amount >= 0 {
            slot.income += txn.amount;
        } else {
            slot.expense += -txn.amount;
        }
    }
    out
}

/// What was spent per category in a range, biggest first. Groups are
/// rolled up from their children, so "Food" totals its own leaves.
pub fn spending_by_category(ledger: &Ledger, range: DateRange) -> Vec<(Option<Id>, i64)> {
    let mut totals: std::collections::HashMap<Option<Id>, i64> = std::collections::HashMap::new();
    for txn in &ledger.transactions {
        if !range.contains(txn.date) || !is_spending(txn, &ledger.categories) {
            continue;
        }
        if ledger.account(txn.account).is_some_and(|a| a.off_budget) {
            continue;
        }
        for (category, amount) in txn.category_amounts() {
            if amount >= 0 {
                continue; // income is not spending
            }
            if category.is_some_and(|id| {
                ledger.categories.kind_of(id) != CategoryKind::Expense
            }) {
                continue;
            }
            *totals.entry(category).or_default() += -amount;
        }
    }
    let mut out: Vec<(Option<Id>, i64)> = totals.into_iter().collect();
    out.sort_by_key(|(id, total)| (std::cmp::Reverse(*total), id.unwrap_or(0)));
    out
}

/// The same, rolled up to top-level groups — the pie every product opens
/// with.
pub fn spending_by_group(ledger: &Ledger, range: DateRange) -> Vec<(Option<Id>, i64)> {
    let mut totals: std::collections::HashMap<Option<Id>, i64> = std::collections::HashMap::new();
    for (category, amount) in spending_by_category(ledger, range) {
        let group = category.and_then(|id| ledger.categories.group_of(id));
        *totals.entry(group).or_default() += amount;
    }
    let mut out: Vec<(Option<Id>, i64)> = totals.into_iter().collect();
    out.sort_by_key(|(id, total)| (std::cmp::Reverse(*total), id.unwrap_or(0)));
    out
}

/// Who took the most money, biggest first — the merchant analysis every
/// product has and everyone actually reads.
pub fn top_payees(ledger: &Ledger, range: DateRange, limit: usize) -> Vec<(String, i64, usize)> {
    let mut totals: std::collections::HashMap<&str, (i64, usize)> =
        std::collections::HashMap::new();
    for txn in &ledger.transactions {
        if !range.contains(txn.date) || !is_spending(txn, &ledger.categories) || txn.amount >= 0 {
            continue;
        }
        let entry = totals.entry(txn.payee.as_str()).or_default();
        entry.0 += -txn.amount;
        entry.1 += 1;
    }
    let mut out: Vec<(String, i64, usize)> = totals
        .into_iter()
        .map(|(payee, (total, count))| (payee.to_string(), total, count))
        .collect();
    out.sort_by_key(|(payee, total, _)| (std::cmp::Reverse(*total), payee.clone()));
    out.truncate(limit);
    out
}

/// Net worth at the end of each of the last `months` months.
///
/// Computed by walking the transactions once in date order and carrying a
/// running total, rather than by asking for a balance per month — the
/// naive version is O(months × transactions) and is why some products take
/// a second to draw this.
pub fn net_worth_series(ledger: &Ledger, months: i32, today: Day) -> Vec<(MonthKey, i64)> {
    let first = date::month_key(date::add_months(today, -(months - 1)));
    let on_budget: std::collections::HashSet<Id> = ledger
        .accounts
        .iter()
        .filter(|a| !a.off_budget)
        .map(|a| a.id)
        .collect();

    // Everything before the window is the opening position.
    let window_start = date::month_key_start(first);
    let mut running: i64 = ledger
        .accounts
        .iter()
        .filter(|a| !a.off_budget)
        .map(|a| a.opening_balance)
        .sum();
    let mut sorted: Vec<&Transaction> = ledger
        .transactions
        .iter()
        .filter(|t| on_budget.contains(&t.account))
        .collect();
    sorted.sort_by_key(|t| t.date);

    let mut out = Vec::with_capacity(months as usize);
    let mut index = 0usize;
    for txn in sorted.iter() {
        if txn.date >= window_start {
            break;
        }
        running += txn.amount;
        index += 1;
    }
    for offset in 0..months {
        let month_end = date::month_end(date::month_key_start(first + offset));
        while index < sorted.len() && sorted[index].date <= month_end {
            running += sorted[index].amount;
            index += 1;
        }
        out.push((first + offset, running));
    }
    out
}

/// A daily balance series for one account, for the sparkline in its row.
pub fn balance_series(ledger: &Ledger, account: Id, days: i32, today: Day) -> Vec<f64> {
    let start = today - days + 1;
    let opening = ledger.account(account).map(|a| a.opening_balance).unwrap_or(0);
    let mut rows: Vec<&Transaction> =
        ledger.transactions.iter().filter(|t| t.account == account).collect();
    rows.sort_by_key(|t| t.date);
    let mut running = opening;
    let mut index = 0usize;
    while index < rows.len() && rows[index].date < start {
        running += rows[index].amount;
        index += 1;
    }
    let mut out = Vec::with_capacity(days as usize);
    for day in start..=today {
        while index < rows.len() && rows[index].date <= day {
            running += rows[index].amount;
            index += 1;
        }
        out.push(running as f64);
    }
    out
}

/// The budget screen's rows for one month: what was assigned, what was
/// spent, and what is left — including what rolled in from before.
///
/// Rollover is computed from the start of the file rather than stored,
/// because a stored carry goes stale the moment an old transaction is
/// edited, and editing old transactions is exactly what people do.
pub fn budget_lines(
    ledger: &Ledger,
    month: MonthKey,
) -> Vec<(Id, BudgetLine)> {
    let mut out = Vec::new();
    for category in ledger.categories.budget_order() {
        if category.is_group() || category.kind != CategoryKind::Expense {
            continue;
        }
        let mut line = BudgetLine::default();
        line.assigned = assigned_for(ledger, category.id, month);
        line.spent = spent_in(ledger, category.id, month);
        if category.rollover {
            // Walk from the first month that has any activity.
            let mut carry = 0i64;
            if let Some(first) = first_month(ledger) {
                let mut cursor = first;
                while cursor < month {
                    carry += assigned_for(ledger, category.id, cursor)
                        - spent_in(ledger, category.id, cursor);
                    // A rollover category cannot carry a negative balance
                    // forward: overspending is settled in the month it
                    // happened, which is what YNAB does and what keeps the
                    // number understandable.
                    carry = carry.max(0);
                    cursor += 1;
                }
            }
            line.carried = carry;
        }
        line.available = line.carried + line.assigned - line.spent;
        out.push((category.id, line));
    }
    out
}

fn assigned_for(ledger: &Ledger, category: Id, month: MonthKey) -> i64 {
    ledger
        .budgets
        .iter()
        .find(|b| b.category == category && b.month == month)
        .map(|b| b.assigned)
        .unwrap_or(0)
}

fn spent_in(ledger: &Ledger, category: Id, month: MonthKey) -> i64 {
    let mut total = 0i64;
    for txn in &ledger.transactions {
        if date::month_key(txn.date) != month || !is_spending(txn, &ledger.categories) {
            continue;
        }
        for (id, amount) in txn.category_amounts() {
            if id == Some(category) && amount < 0 {
                total += -amount;
            }
        }
    }
    total
}

fn first_month(ledger: &Ledger) -> Option<MonthKey> {
    ledger.transactions.iter().map(|t| date::month_key(t.date)).min()
}

/// What is due in the next `days`, soonest first — the "upcoming" list.
pub fn upcoming(ledger: &Ledger, days: i32, today: Day) -> Vec<&Scheduled> {
    let horizon = today + days;
    let mut out: Vec<&Scheduled> = ledger
        .scheduled
        .iter()
        .filter(|s| s.enabled && s.next_due <= horizon)
        .collect();
    out.sort_by_key(|s| s.next_due);
    out
}

/// Where the balance is heading: today's balance, then each scheduled item
/// applied on its due date. The forecast every product added late and
/// everyone asks for.
pub fn cash_forecast(ledger: &Ledger, account: Id, days: i32, today: Day) -> Vec<f64> {
    let mut balance = ledger.balance_on(account, today);
    let mut out = Vec::with_capacity(days as usize);
    for offset in 0..days {
        let day = today + offset;
        for item in &ledger.scheduled {
            if !item.enabled || item.account != account {
                continue;
            }
            // Walk this schedule's occurrences into the window.
            let mut due = item.next_due;
            while due < day {
                due = item.recurrence.next(due);
            }
            if due == day {
                balance += item.amount;
            }
        }
        out.push(balance as f64);
    }
    out
}

/// Recurring charges the ledger can see for itself — the subscription
/// screen, without anyone having to declare anything.
///
/// A payee qualifies when it has charged a similar amount at a regular
/// interval at least three times. Three is the smallest number that can
/// tell a rhythm from a coincidence.
pub fn detected_subscriptions(ledger: &Ledger, today: Day) -> Vec<(String, i64, Recurrence, Day)> {
    let mut by_payee: std::collections::HashMap<&str, Vec<&Transaction>> =
        std::collections::HashMap::new();
    let year_ago = today - 400;
    for txn in &ledger.transactions {
        if txn.amount >= 0 || txn.date < year_ago || txn.transfer_group.is_some() {
            continue;
        }
        by_payee.entry(txn.payee.as_str()).or_default().push(txn);
    }
    let mut out = Vec::new();
    for (payee, mut rows) in by_payee {
        if rows.len() < 3 {
            continue;
        }
        rows.sort_by_key(|t| t.date);
        let gaps: Vec<i32> = rows.windows(2).map(|w| w[1].date - w[0].date).collect();
        let average = gaps.iter().sum::<i32>() / gaps.len() as i32;
        let recurrence = match average {
            5..=9 => Recurrence::Weekly,
            12..=16 => Recurrence::Fortnightly,
            26..=35 => Recurrence::Monthly,
            85..=100 => Recurrence::Quarterly,
            350..=380 => Recurrence::Yearly,
            _ => continue,
        };
        // The amounts have to be alike: a supermarket visited weekly is
        // not a subscription, a gym charged the same every month is.
        let amounts: Vec<i64> = rows.iter().map(|t| t.amount).collect();
        let typical = amounts[amounts.len() / 2];
        let steady = amounts
            .iter()
            .all(|a| (a - typical).abs() <= (typical.abs() / 10).max(100));
        if !steady {
            continue;
        }
        let last = rows.last().unwrap().date;
        out.push((payee.to_string(), typical, recurrence, recurrence.next(last)));
    }
    out.sort_by_key(|(payee, amount, _, _)| (*amount, payee.clone()));
    out
}

/// Rows the user should look at: uncategorized, unbalanced splits, and
/// transfers whose halves do not cancel.
pub fn needs_attention(ledger: &Ledger) -> Vec<(Id, &'static str)> {
    let mut out = Vec::new();
    for txn in &ledger.transactions {
        if txn.split_imbalance() != 0 {
            out.push((txn.id, "split does not add up"));
        } else if txn.category.is_none() && txn.splits.is_empty() && !txn.is_transfer() {
            out.push((txn.id, "no category"));
        }
    }
    for group in ledger
        .transactions
        .iter()
        .filter_map(|t| t.transfer_group)
        .collect::<std::collections::HashSet<_>>()
    {
        if !ledger.transfer_is_balanced(group) {
            if let Some(txn) = ledger.transactions.iter().find(|t| t.transfer_group == Some(group))
            {
                out.push((txn.id, "transfer does not cancel"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::from_ymd;
    use crate::money::EUR;

    /// A tiny ledger with one of everything the reports have to get right.
    fn ledger() -> Ledger {
        let mut ledger = Ledger { base_currency: EUR, ..Ledger::default() };
        let mut checking = Account::new("Checking", AccountKind::Checking, EUR);
        checking.id = 1;
        checking.opening_balance = 100_000;
        let mut savings = Account::new("Savings", AccountKind::Savings, EUR);
        savings.id = 2;
        ledger.accounts = vec![checking, savings];

        let mut food = Category::group("Food", CategoryKind::Expense);
        food.id = 10;
        let mut groceries = Category::child("Groceries", 10, CategoryKind::Expense);
        groceries.id = 11;
        let mut restaurants = Category::child("Restaurants", 10, CategoryKind::Expense);
        restaurants.id = 12;
        let mut income = Category::group("Income", CategoryKind::Income);
        income.id = 20;
        let mut salary = Category::child("Salary", 20, CategoryKind::Income);
        salary.id = 21;
        ledger.categories.categories = vec![food, groceries, restaurants, income, salary];

        let mut next_id = 100;
        let mut push = |ledger: &mut Ledger, account, date, payee: &str, amount, category| {
            let mut txn = Transaction::new(account, date, payee, amount);
            txn.id = next_id;
            next_id += 1;
            txn.category = category;
            ledger.transactions.push(txn);
        };
        push(&mut ledger, 1, from_ymd(2024, 3, 25), "Employer", 300_000, Some(21));
        push(&mut ledger, 1, from_ymd(2024, 3, 4), "Albert Heijn", -8_000, Some(11));
        push(&mut ledger, 1, from_ymd(2024, 3, 11), "Albert Heijn", -6_500, Some(11));
        push(&mut ledger, 1, from_ymd(2024, 3, 14), "Café", -3_200, Some(12));
        push(&mut ledger, 1, from_ymd(2024, 2, 4), "Albert Heijn", -7_000, Some(11));

        // A transfer to savings: not spending, not income.
        let mut out = Transaction::new(1, from_ymd(2024, 3, 26), "Savings", -50_000);
        out.id = 200;
        out.transfer_group = Some(1);
        let mut into = Transaction::new(2, from_ymd(2024, 3, 26), "From checking", 50_000);
        into.id = 201;
        into.transfer_group = Some(1);
        ledger.transactions.push(out);
        ledger.transactions.push(into);
        ledger
    }

    #[test]
    fn transfers_are_never_spending_or_income() {
        let ledger = ledger();
        let march = DateRange::month(date::month_key(from_ymd(2024, 3, 1)));
        let flow = flow(&ledger, march);
        assert_eq!(flow.income, 300_000, "the transfer must not count as income");
        assert_eq!(flow.expense, 8_000 + 6_500 + 3_200, "nor the other half as spending");
        assert_eq!(flow.net(), 300_000 - 17_700);

        // And the money did not vanish: net worth is unchanged by it.
        let before = ledger.net_worth_on(from_ymd(2024, 3, 25));
        let after = ledger.net_worth_on(from_ymd(2024, 3, 26));
        assert_eq!(before, after);
    }

    #[test]
    fn category_totals_roll_up_and_rank() {
        let ledger = ledger();
        let march = DateRange::month(date::month_key(from_ymd(2024, 3, 1)));
        let by_category = spending_by_category(&ledger, march);
        assert_eq!(by_category[0], (Some(11), 14_500));
        assert_eq!(by_category[1], (Some(12), 3_200));
        let by_group = spending_by_group(&ledger, march);
        assert_eq!(by_group[0], (Some(10), 17_700), "Food totals its children");
    }

    #[test]
    fn splits_are_counted_in_each_of_their_parts() {
        let mut ledger = ledger();
        let mut txn = Transaction::new(1, from_ymd(2024, 3, 20), "Supermarket", -10_000);
        txn.id = 300;
        txn.splits = vec![
            Split { id: 1, category: Some(11), amount: -6_000, memo: String::new() },
            Split { id: 2, category: Some(12), amount: -4_000, memo: String::new() },
        ];
        ledger.transactions.push(txn);
        let march = DateRange::month(date::month_key(from_ymd(2024, 3, 1)));
        let by_category = spending_by_category(&ledger, march);
        let groceries = by_category.iter().find(|(id, _)| *id == Some(11)).unwrap().1;
        let restaurants = by_category.iter().find(|(id, _)| *id == Some(12)).unwrap().1;
        assert_eq!(groceries, 14_500 + 6_000);
        assert_eq!(restaurants, 3_200 + 4_000);
    }

    #[test]
    fn monthly_flow_lines_up_with_the_months_asked_for() {
        let ledger = ledger();
        let series = monthly_flow(&ledger, 3, from_ymd(2024, 3, 31));
        assert_eq!(series.len(), 3);
        assert_eq!(series[2].0, date::month_key(from_ymd(2024, 3, 1)));
        assert_eq!(series[2].1.income, 300_000);
        assert_eq!(series[1].1.expense, 7_000, "February had one shop");
        assert_eq!(series[0].1, Flow::default(), "January is empty, not missing");
    }

    #[test]
    fn net_worth_walks_forward_once() {
        let ledger = ledger();
        let series = net_worth_series(&ledger, 3, from_ymd(2024, 3, 31));
        assert_eq!(series.len(), 3);
        // January: nothing had happened, so just the opening balance.
        assert_eq!(series[0].1, 100_000);
        // February: one shop.
        assert_eq!(series[1].1, 100_000 - 7_000);
        // March: everything, and the transfer cancels out.
        assert_eq!(series[2].1, ledger.net_worth_on(from_ymd(2024, 3, 31)));
    }

    #[test]
    fn top_payees_rank_by_money_not_by_count() {
        let ledger = ledger();
        let march = DateRange::month(date::month_key(from_ymd(2024, 3, 1)));
        let payees = top_payees(&ledger, march, 5);
        assert_eq!(payees[0].0, "Albert Heijn");
        assert_eq!(payees[0].1, 14_500);
        assert_eq!(payees[0].2, 2);
        assert!(payees.iter().all(|(name, _, _)| name != "Savings"));
    }

    #[test]
    fn budget_available_carries_only_where_asked() {
        let mut ledger = ledger();
        let feb = date::month_key(from_ymd(2024, 2, 1));
        let mar = date::month_key(from_ymd(2024, 3, 1));
        // Groceries: no rollover. Restaurants: rollover.
        ledger.categories.categories[1].rollover = false;
        ledger.categories.categories[2].rollover = true;
        for month in [feb, mar] {
            ledger.budgets.push(BudgetEntry { category: 11, month, assigned: 10_000, rollover: false });
            ledger.budgets.push(BudgetEntry { category: 12, month, assigned: 5_000, rollover: true });
        }
        let lines = budget_lines(&ledger, mar);
        let groceries = lines.iter().find(|(id, _)| *id == 11).unwrap().1;
        let restaurants = lines.iter().find(|(id, _)| *id == 12).unwrap().1;

        // Groceries spent 14,500 against 10,000 — overspent, nothing carried.
        assert_eq!(groceries.spent, 14_500);
        assert_eq!(groceries.carried, 0);
        assert_eq!(groceries.available, -4_500);
        assert_eq!(groceries.state(), BudgetState::Overspent);

        // Restaurants: February assigned 5,000 and spent nothing, so 5,000
        // carried into March, where 3,200 went.
        assert_eq!(restaurants.carried, 5_000);
        assert_eq!(restaurants.spent, 3_200);
        assert_eq!(restaurants.available, 5_000 + 5_000 - 3_200);
    }

    #[test]
    fn subscriptions_are_found_by_rhythm_not_by_name() {
        let mut ledger = Ledger { base_currency: EUR, ..Ledger::default() };
        let mut account = Account::new("Card", AccountKind::CreditCard, EUR);
        account.id = 1;
        ledger.accounts.push(account);
        let today = from_ymd(2024, 6, 1);
        // A monthly charge at the same price: a subscription.
        for month in 1..=5 {
            let mut txn = Transaction::new(1, from_ymd(2024, month, 7), "Netflix", -1_399);
            txn.id = 100 + month as i64;
            ledger.transactions.push(txn);
        }
        // A supermarket, visited often at wildly different amounts: not one.
        for (index, day) in [3, 9, 15, 21, 27].into_iter().enumerate() {
            let mut txn = Transaction::new(
                1,
                from_ymd(2024, 5, day),
                "Albert Heijn",
                -(2_000 + index as i64 * 3_000),
            );
            txn.id = 200 + index as i64;
            ledger.transactions.push(txn);
        }
        let found = detected_subscriptions(&ledger, today);
        assert!(found.iter().any(|(payee, amount, recurrence, _)| {
            payee == "Netflix" && *amount == -1_399 && *recurrence == Recurrence::Monthly
        }));
        assert!(
            !found.iter().any(|(payee, _, _, _)| payee == "Albert Heijn"),
            "varying amounts are not a subscription"
        );
    }

    #[test]
    fn attention_finds_what_a_person_would_want_told() {
        let mut ledger = ledger();
        let mut loose = Transaction::new(1, from_ymd(2024, 3, 28), "Mystery", -1_000);
        loose.id = 400;
        ledger.transactions.push(loose);
        let mut broken = Transaction::new(1, from_ymd(2024, 3, 29), "Shop", -5_000);
        broken.id = 401;
        broken.splits =
            vec![Split { id: 1, category: Some(11), amount: -4_000, memo: String::new() }];
        ledger.transactions.push(broken);

        let attention = needs_attention(&ledger);
        assert!(attention.iter().any(|(id, why)| *id == 400 && *why == "no category"));
        assert!(attention.iter().any(|(id, why)| *id == 401 && *why == "split does not add up"));
        // The balanced transfer is not a problem.
        assert!(!attention.iter().any(|(id, _)| *id == 200));
    }
}
