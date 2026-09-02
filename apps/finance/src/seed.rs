//! A believable financial life, generated, so the app is never empty.
//!
//! An empty finance app is unusable as a demo and hard to develop against:
//! no balances, no charts, nothing to click. So a file with no accounts
//! gets filled with two years of one household's money — salary on the
//! 25th, rent on the 1st, groceries twice a week, a card that gets paid
//! off monthly, a mortgage that amortises, subscriptions that renew, and
//! the occasional holiday.
//!
//! It is generated rather than canned because a fixed CSV goes stale: the
//! demo has to end *today* whenever today is, or every screen opens on an
//! empty current month. The generator is seeded and deterministic, so the
//! same day always produces the same file and a screenshot is reproducible.
//!
//! Generation is pure. Native first-run persistence writes the resulting
//! ledger through the ordinary database paths, while demo builds can use it
//! directly without a filesystem.

use crate::date::{self, Day};
use crate::model::*;
use crate::money::{Currency, EUR};

/// How much history to generate. Two years covers every screen: a full
/// year-over-year comparison, twelve months of budgets, and enough of a
/// net-worth curve to have a shape.
pub const DEFAULT_YEARS: i32 = 2;

/// xorshift64*, so the demo is identical on every machine and every run.
/// `rand` is not a dependency of this tree and a ledger does not need
/// cryptographic randomness — it needs the same numbers twice.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Inclusive range.
    fn between(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        low + (self.next() % (high - low + 1) as u64) as i64
    }

    /// True with probability `percent`.
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() % items.len() as u64) as usize]
    }
}

/// The ids the generator needs to refer back to while it works.
struct Cats {
    salary: Id,
    interest: Id,
    housing: Id,
    utilities: Id,
    internet: Id,
    phone: Id,
    groceries: Id,
    restaurants: Id,
    coffee: Id,
    household: Id,
    fuel: Id,
    transit: Id,
    car: Id,
    clothing: Id,
    electronics: Id,
    pharmacy: Id,
    gym: Id,
    streaming: Id,
    events: Id,
    flights: Id,
    hotels: Id,
    fees: Id,
    gifts: Id,
    childcare: Id,
    insurance: Id,
}

/// Generate a complete household ledger deterministically for `today`.
pub fn generate(years: i32, today: Day) -> Ledger {
    let start = date::month_start(date::add_months(today, -(years * 12 - 1)));
    let currency = EUR;
    let mut ledger = Ledger { base_currency: currency, ..Ledger::default() };

    let accounts = insert_accounts(&mut ledger.accounts, currency, start);
    let cats = insert_categories(&mut ledger.categories.categories);
    let mut rng = Rng::new(0x5EED_F1_A2_C3);

    let mut txns: Vec<Transaction> = Vec::new();
    let mut transfer_group = 1i64;

    monthly_income(&mut txns, &accounts, &cats, start, today);
    housing(&mut txns, &accounts, &cats, start, today, &mut rng);
    subscriptions(&mut txns, &accounts, &cats, start, today);
    daily_life(&mut txns, &accounts, &cats, start, today, &mut rng);
    occasional(&mut txns, &accounts, &cats, start, today, &mut rng);
    card_payments(&mut txns, &accounts, start, today, &mut transfer_group);
    savings_transfers(&mut txns, &accounts, start, today, &mut transfer_group);
    savings_interest(&mut txns, &accounts, &cats, start, today);
    mortgage(&mut txns, &accounts, &cats, start, today, &mut transfer_group);

    // Age determines what the bank has seen: anything older than a few
    // days has cleared, the oldest year has been reconciled, and the last
    // few days are still in flight. That gives the reconcile screen and
    // the "cleared vs current" balances something true to show.
    for txn in txns.iter_mut() {
        let age = today - txn.date;
        txn.cleared = if age > 365 {
            Cleared::Reconciled
        } else if age > 4 {
            Cleared::Cleared
        } else {
            Cleared::Uncleared
        };
    }
    txns.sort_by_key(|t| t.date);
    for (index, txn) in txns.iter_mut().enumerate() {
        txn.id = index as Id + 1;
    }
    add_splits(&mut txns, &cats);

    ledger.transactions = txns;
    ledger.budgets = generate_budgets(&cats, today, years);
    ledger.rules = generate_rules(&cats);
    ledger.scheduled = generate_scheduled(&accounts, &cats, today);
    ledger.categories.categories.sort_by_key(|category| (category.sort_order, category.id));
    ledger.scheduled.sort_by_key(|item| (item.next_due, item.id));
    ledger
}

struct Accounts {
    checking: Id,
    savings: Id,
    card: Id,
    brokerage: Id,
    mortgage: Id,
    house: Id,
    cash: Id,
}

impl Accounts {
    fn all(&self) -> [Id; 7] {
        [
            self.checking,
            self.savings,
            self.card,
            self.brokerage,
            self.mortgage,
            self.house,
            self.cash,
        ]
    }
}

fn insert_accounts(accounts: &mut Vec<Account>, currency: Currency, start: Day) -> Accounts {
    let mut make = |name: &str,
                    kind: AccountKind,
                    institution: &str,
                    opening: i64,
                    order: i32| {
        let mut account = Account::new(name, kind, currency);
        account.id = accounts.len() as Id + 1;
        account.institution = institution.to_string();
        account.opening_balance = opening;
        account.opening_date = start - 1;
        account.sort_order = order;
        let id = account.id;
        accounts.push(account);
        id
    };
    Accounts {
        checking: make("Everyday", AccountKind::Checking, "ING", 342_150, 0),
        savings: make("Savings", AccountKind::Savings, "ING", 1_480_000, 1),
        card: make("Rewards Card", AccountKind::CreditCard, "Amex", -84_320, 2),
        cash: make("Cash", AccountKind::Cash, "", 12_000, 3),
        brokerage: make("Brokerage", AccountKind::Investment, "DEGIRO", 2_650_000, 4),
        // A mortgage is a debt: negative, and paid down over the run.
        mortgage: make("Mortgage", AccountKind::Loan, "Rabobank", -24_800_000, 5),
        house: make("Apartment", AccountKind::Asset, "", 41_500_000, 6),
    }
}

fn insert_categories(categories: &mut Vec<Category>) -> Cats {
    let mut group = |name: &str, kind: CategoryKind, order: i32| {
        let mut category = Category::group(name, kind);
        category.id = categories.len() as Id + 1;
        category.sort_order = order;
        let id = category.id;
        categories.push(category);
        id
    };
    let income = group("Income", CategoryKind::Income, 0);
    let housing = group("Housing", CategoryKind::Expense, 1);
    let food = group("Food", CategoryKind::Expense, 2);
    let transport = group("Transport", CategoryKind::Expense, 3);
    let shopping = group("Shopping", CategoryKind::Expense, 4);
    let health = group("Health", CategoryKind::Expense, 5);
    let fun = group("Fun", CategoryKind::Expense, 6);
    let travel = group("Travel", CategoryKind::Expense, 7);
    let money = group("Money", CategoryKind::Expense, 8);
    let family = group("Family", CategoryKind::Expense, 9);
    drop(group);

    let mut child = |name: &str,
                     parent: Id,
                     kind: CategoryKind,
                     rollover: bool,
                     order: i32| {
        let mut category = Category::child(name, parent, kind);
        category.id = categories.len() as Id + 1;
        category.budgeted = kind == CategoryKind::Expense;
        category.rollover = rollover;
        category.sort_order = order;
        let id = category.id;
        categories.push(category);
        id
    };

    Cats {
        salary: child("Salary", income, CategoryKind::Income, false, 0),
        interest: child("Interest", income, CategoryKind::Income, false, 1),
        housing: child("Mortgage", housing, CategoryKind::Expense, false, 0),
        utilities: child("Energy", housing, CategoryKind::Expense, false, 1),
        internet: child("Internet", housing, CategoryKind::Expense, false, 2),
        phone: child("Phone", housing, CategoryKind::Expense, false, 3),
        groceries: child("Groceries", food, CategoryKind::Expense, false, 0),
        restaurants: child("Restaurants", food, CategoryKind::Expense, false, 1),
        coffee: child("Coffee", food, CategoryKind::Expense, false, 2),
        household: child("Household", shopping, CategoryKind::Expense, false, 0),
        fuel: child("Fuel", transport, CategoryKind::Expense, false, 0),
        transit: child("Transit", transport, CategoryKind::Expense, false, 1),
        // Car maintenance is lumpy, so it rolls over: three quiet months
        // pay for the fourth.
        car: child("Car upkeep", transport, CategoryKind::Expense, true, 2),
        clothing: child("Clothing", shopping, CategoryKind::Expense, false, 1),
        electronics: child("Electronics", shopping, CategoryKind::Expense, true, 2),
        pharmacy: child("Pharmacy", health, CategoryKind::Expense, false, 0),
        gym: child("Gym", health, CategoryKind::Expense, false, 1),
        streaming: child("Streaming", fun, CategoryKind::Expense, false, 0),
        events: child("Going out", fun, CategoryKind::Expense, false, 1),
        flights: child("Flights", travel, CategoryKind::Expense, true, 0),
        hotels: child("Hotels", travel, CategoryKind::Expense, true, 1),
        fees: child("Bank fees", money, CategoryKind::Expense, false, 0),
        insurance: child("Insurance", money, CategoryKind::Expense, false, 1),
        gifts: child("Gifts", family, CategoryKind::Expense, true, 0),
        childcare: child("Childcare", family, CategoryKind::Expense, false, 1),
    }
}

/// A payday that lands on a working day: paid on the 25th, moved back to
/// the Friday when that is a weekend, which is what employers do.
fn payday(month_start: Day) -> Day {
    let (y, m, _) = date::to_ymd(month_start);
    let mut day = date::from_ymd(y, m, 25);
    while date::is_weekend(day) {
        day -= 1;
    }
    day
}

fn each_month(start: Day, end: Day, mut body: impl FnMut(Day)) {
    let mut month = date::month_start(start);
    while month <= end {
        body(month);
        month = date::add_months(month, 1);
    }
}

fn monthly_income(txns: &mut Vec<Transaction>, accounts: &Accounts, cats: &Cats, start: Day, end: Day) {
    // A raise a third of the way in, so year-over-year has something to
    // show and the budget screen has a reason to change.
    let raise_at = date::add_months(start, 14);
    each_month(start, end, |month| {
        let day = payday(month);
        if day > end || day < start {
            return;
        }
        let amount = if day >= raise_at { 492_400 } else { 465_000 };
        let mut txn = Transaction::new(accounts.checking, day, "Bergman Design BV", amount);
        txn.category = Some(cats.salary);
        txn.memo = "Salary".into();
        txns.push(txn);
    });
}

fn housing(
    txns: &mut Vec<Transaction>,
    accounts: &Accounts,
    cats: &Cats,
    start: Day,
    end: Day,
    rng: &mut Rng,
) {
    each_month(start, end, |month| {
        let (y, m, _) = date::to_ymd(month);
        let mut push = |day: u32, payee: &str, amount: i64, category: Id| {
            let date = date::from_ymd(y, m, day.min(date::days_in_month(y, m)));
            if date > end || date < start {
                return;
            }
            let mut txn = Transaction::new(accounts.checking, date, payee, -amount);
            txn.category = Some(category);
            txns.push(txn);
        };
        // Energy swings with the season: a Dutch winter costs roughly
        // double a summer month.
        let winter = matches!(m, 11 | 12 | 1 | 2 | 3);
        let energy = if winter { rng.between(18_500, 24_000) } else { rng.between(8_500, 12_500) };
        push(3, "Eneco", energy, cats.utilities);
        push(5, "KPN Internet", 5_450, cats.internet);
        push(8, "Vodafone", 2_890, cats.phone);
        push(12, "Centraal Beheer", 8_640, cats.insurance);
        push(2, "Kinderopvang Zonnetje", 54_000, cats.childcare);
    });
}

fn subscriptions(txns: &mut Vec<Transaction>, accounts: &Accounts, cats: &Cats, start: Day, end: Day) {
    // Charged to the card, like most subscriptions are.
    let monthly: [(u32, &str, i64, fn(&Cats) -> Id); 5] = [
        (4, "Netflix", 1_399, |c| c.streaming),
        (7, "Spotify", 1_099, |c| c.streaming),
        (15, "Apple iCloud", 299, |c| c.streaming),
        (18, "SportCity", 2_995, |c| c.gym),
        (22, "Adobe", 2_399, |c| c.electronics),
    ];
    each_month(start, end, |month| {
        let (y, m, _) = date::to_ymd(month);
        for (day, payee, amount, category) in monthly {
            let date = date::from_ymd(y, m, day);
            if date > end || date < start {
                continue;
            }
            let mut txn = Transaction::new(accounts.card, date, payee, -amount);
            txn.category = Some(category(cats));
            txns.push(txn);
        }
    });
}

fn daily_life(
    txns: &mut Vec<Transaction>,
    accounts: &Accounts,
    cats: &Cats,
    start: Day,
    end: Day,
    rng: &mut Rng,
) {
    const SUPERMARKETS: [&str; 5] = ["Albert Heijn", "Jumbo", "Lidl", "Dirk", "Ekoplaza"];
    const CAFES: [&str; 5] =
        ["Coffee Company", "Bocca Koffie", "Lot Sixty One", "Toki", "Screaming Beans"];
    const RESTAURANTS: [&str; 6] = [
        "Café de Klos",
        "Bar Bukowski",
        "Thai Bird",
        "De Biertuin",
        "Pizzeria Sugo",
        "Sushi Ran",
    ];
    const SHOPS: [&str; 5] = ["HEMA", "Bol.com", "Zara", "Decathlon", "MediaMarkt"];

    let mut day = start;
    while day <= end {
        let weekday = date::weekday(day);
        // Groceries: a big weekend shop and one or two top-ups.
        if weekday == 5 || (rng.chance(35) && weekday != 6) {
            let big = weekday == 5;
            let amount = if big { rng.between(5_800, 12_400) } else { rng.between(1_200, 4_500) };
            let mut txn = Transaction::new(
                if rng.chance(15) { accounts.cash } else { accounts.card },
                day,
                rng.pick(&SUPERMARKETS),
                -amount,
            );
            txn.category = Some(cats.groceries);
            txns.push(txn);
        }
        // Coffee on working days.
        if weekday < 5 && rng.chance(55) {
            let mut txn =
                Transaction::new(accounts.card, day, rng.pick(&CAFES), -rng.between(280, 720));
            txn.category = Some(cats.coffee);
            txns.push(txn);
        }
        // Eating out, mostly at the weekend.
        let eats_out = if weekday >= 4 { rng.chance(45) } else { rng.chance(12) };
        if eats_out {
            let mut txn = Transaction::new(
                accounts.card,
                day,
                rng.pick(&RESTAURANTS),
                -rng.between(2_200, 8_900),
            );
            txn.category = Some(cats.restaurants);
            txns.push(txn);
        }
        // Transit and fuel.
        if weekday < 5 && rng.chance(30) {
            let mut txn = Transaction::new(accounts.card, day, "NS Reizigers", -rng.between(320, 2_450));
            txn.category = Some(cats.transit);
            txns.push(txn);
        }
        if rng.chance(6) {
            let mut txn = Transaction::new(accounts.card, day, "Shell", -rng.between(4_500, 8_800));
            txn.category = Some(cats.fuel);
            txns.push(txn);
        }
        // Odds and ends.
        if rng.chance(9) {
            let shop = rng.pick(&SHOPS);
            let (category, amount) = match *shop {
                "MediaMarkt" => (cats.electronics, rng.between(3_900, 45_000)),
                "Zara" | "Decathlon" => (cats.clothing, rng.between(2_500, 14_000)),
                _ => (cats.household, rng.between(800, 6_500)),
            };
            let mut txn = Transaction::new(accounts.card, day, shop, -amount);
            txn.category = Some(category);
            txns.push(txn);
        }
        if rng.chance(4) {
            let mut txn = Transaction::new(accounts.card, day, "Etos", -rng.between(600, 3_400));
            txn.category = Some(cats.pharmacy);
            txns.push(txn);
        }
        if rng.chance(5) {
            let mut txn =
                Transaction::new(accounts.cash, day, "Albert Cuyp Markt", -rng.between(500, 2_500));
            txn.category = Some(cats.groceries);
            txns.push(txn);
        }
        day += 1;
    }
}

fn occasional(
    txns: &mut Vec<Transaction>,
    accounts: &Accounts,
    cats: &Cats,
    start: Day,
    end: Day,
    rng: &mut Rng,
) {
    // One holiday a year, in the summer, plus a winter weekend away.
    let mut year = date::year_of(start);
    while year <= date::year_of(end) {
        for (month, day, flight, hotel, place) in [
            (7, 12, 118_000i64, 96_500i64, "Lisbon"),
            (2, 8, 42_000, 38_000, "Vienna"),
        ] {
            let date = date::from_ymd(year, month, day);
            if date < start || date > end {
                continue;
            }
            let mut air = Transaction::new(accounts.card, date, "KLM", -flight);
            air.category = Some(cats.flights);
            air.memo = format!("{place} trip");
            txns.push(air);
            let mut stay = Transaction::new(accounts.card, date + 1, "Booking.com", -hotel);
            stay.category = Some(cats.hotels);
            stay.memo = format!("{place} trip");
            txns.push(stay);
            // Spending abroad, on the card.
            for offset in 2..7 {
                if date + offset > end {
                    break;
                }
                let mut meal = Transaction::new(
                    accounts.card,
                    date + offset,
                    "Restaurante Ramiro",
                    -rng.between(2_800, 9_500),
                );
                meal.category = Some(cats.restaurants);
                txns.push(meal);
            }
        }
        // Car maintenance, once or twice a year — the lumpy category the
        // rollover exists for.
        let service = date::from_ymd(year, 5, 14);
        if service >= start && service <= end {
            let mut txn = Transaction::new(accounts.checking, service, "Garage Van Dijk", -rng.between(28_000, 62_000));
            txn.category = Some(cats.car);
            txns.push(txn);
        }
        // Birthdays and December.
        for (month, day, payee) in [(12, 18, "Bol.com"), (6, 4, "Bloemenwinkel")] {
            let date = date::from_ymd(year, month, day);
            if date >= start && date <= end {
                let mut txn =
                    Transaction::new(accounts.card, date, payee, -rng.between(4_500, 22_000));
                txn.category = Some(cats.gifts);
                txn.memo = "Gift".into();
                txns.push(txn);
            }
        }
        // A concert or two.
        for (month, day) in [(9, 21), (3, 15)] {
            let date = date::from_ymd(year, month, day);
            if date >= start && date <= end && rng.chance(70) {
                let mut txn =
                    Transaction::new(accounts.card, date, "Paradiso", -rng.between(3_500, 9_000));
                txn.category = Some(cats.events);
                txns.push(txn);
            }
        }
        year += 1;
    }
}

/// The card is paid off in full each month, from checking — a transfer
/// pair, which is what gives the transfer screens something real.
fn card_payments(
    txns: &mut Vec<Transaction>,
    accounts: &Accounts,
    start: Day,
    end: Day,
    group: &mut i64,
) {
    each_month(start, end, |month| {
        let (y, m, _) = date::to_ymd(month);
        let date = date::from_ymd(y, m, 28.min(date::days_in_month(y, m)));
        if date > end || date < start {
            return;
        }
        // What the card ran up in the previous month, near enough.
        let previous = date::add_months(date, -1);
        let spent: i64 = txns
            .iter()
            .filter(|t| {
                t.account == accounts.card
                    && date::month_key(t.date) == date::month_key(previous)
            })
            .map(|t| t.amount)
            .sum();
        let amount = -spent;
        if amount <= 0 {
            return;
        }
        *group += 1;
        let mut out = Transaction::new(accounts.checking, date, "Amex", -amount);
        out.transfer_group = Some(*group);
        out.memo = "Card payment".into();
        let mut into = Transaction::new(accounts.card, date, "Payment received", amount);
        into.transfer_group = Some(*group);
        into.memo = "Card payment".into();
        txns.push(out);
        txns.push(into);
    });
}

fn savings_transfers(
    txns: &mut Vec<Transaction>,
    accounts: &Accounts,
    start: Day,
    end: Day,
    group: &mut i64,
) {
    each_month(start, end, |month| {
        let date = payday(month) + 1;
        if date > end || date < start {
            return;
        }
        *group += 1;
        let amount = 40_000;
        let mut out = Transaction::new(accounts.checking, date, "Savings", -amount);
        out.transfer_group = Some(*group);
        out.memo = "Monthly saving".into();
        let mut into = Transaction::new(accounts.savings, date, "From Everyday", amount);
        into.transfer_group = Some(*group);
        into.memo = "Monthly saving".into();
        txns.push(out);
        txns.push(into);
    });
}

fn savings_interest(
    txns: &mut Vec<Transaction>,
    accounts: &Accounts,
    cats: &Cats,
    start: Day,
    end: Day,
) {
    let mut balance = 1_480_000i64;
    each_month(start, end, |month| {
        let (y, m, _) = date::to_ymd(month);
        let date = date::from_ymd(y, m, date::days_in_month(y, m));
        if date > end || date < start {
            return;
        }
        balance += 40_000;
        // 1.8% a year, paid monthly.
        let interest = balance * 18 / 1000 / 12;
        let mut txn = Transaction::new(accounts.savings, date, "ING", interest);
        txn.category = Some(cats.interest);
        txn.memo = "Interest".into();
        txns.push(txn);
    });
}

/// A mortgage payment is two things at once: interest (an expense) and
/// principal (a transfer that shrinks the debt). Modelling it as a split
/// would hide the debt movement, so it is a transfer pair for the
/// principal and a plain expense for the interest — which is how the loan
/// balance ends up actually going down on the net-worth chart.
fn mortgage(
    txns: &mut Vec<Transaction>,
    accounts: &Accounts,
    cats: &Cats,
    start: Day,
    end: Day,
    group: &mut i64,
) {
    let mut owed = 24_800_000i64;
    each_month(start, end, |month| {
        let (y, m, _) = date::to_ymd(month);
        let date = date::from_ymd(y, m, 2);
        if date > end || date < start {
            return;
        }
        // 3.4% a year on the outstanding balance.
        let interest = owed * 34 / 1000 / 12;
        let principal = 92_400 - interest.min(92_400);
        let mut cost = Transaction::new(accounts.checking, date, "Rabobank", -interest);
        cost.category = Some(cats.housing);
        cost.memo = "Mortgage interest".into();
        txns.push(cost);
        if principal > 0 {
            *group += 1;
            let mut out = Transaction::new(accounts.checking, date, "Rabobank", -principal);
            out.transfer_group = Some(*group);
            out.memo = "Mortgage principal".into();
            let mut down = Transaction::new(accounts.mortgage, date, "Payment", principal);
            down.transfer_group = Some(*group);
            down.memo = "Mortgage principal".into();
            txns.push(out);
            txns.push(down);
            owed -= principal;
        }
    });
}

/// Turn a handful of supermarket trips into split transactions, so the
/// split UI has real examples the moment the app opens.
fn add_splits(transactions: &mut [Transaction], cats: &Cats) {
    let mut next_split_id = 1;
    for txn in transactions
        .iter_mut()
        .filter(|t| {
            t.category == Some(cats.groceries) && t.amount < -7_000 && t.splits.is_empty()
        })
        .take(6)
    {
        // A third of a big shop was household goods, not food.
        let household = txn.amount / 3;
        let food = txn.amount - household;
        txn.splits = vec![
            Split {
                id: next_split_id,
                category: Some(cats.groceries),
                amount: food,
                memo: "Food".into(),
            },
            Split {
                id: next_split_id + 1,
                category: Some(cats.household),
                amount: household,
                memo: "Cleaning, paper".into(),
            },
        ];
        next_split_id += 2;
        debug_assert_eq!(txn.split_imbalance(), 0);
    }
}

/// Budgets for every month of history, so the budget screen opens on real
/// numbers and the "assigned vs spent" bars mean something.
fn generate_budgets(cats: &Cats, today: Day, years: i32) -> Vec<BudgetEntry> {
    let plan: [(Id, i64); 17] = [
        (cats.housing, 92_400),
        (cats.utilities, 14_000),
        (cats.internet, 5_450),
        (cats.phone, 2_890),
        (cats.childcare, 54_000),
        (cats.insurance, 8_640),
        (cats.groceries, 52_000),
        (cats.restaurants, 18_000),
        (cats.coffee, 6_000),
        (cats.household, 8_000),
        (cats.transit, 9_000),
        (cats.fuel, 12_000),
        (cats.car, 15_000),
        (cats.clothing, 10_000),
        (cats.streaming, 5_200),
        (cats.gym, 2_995),
        (cats.events, 8_000),
    ];
    let months = years * 12;
    let first = date::month_key(date::add_months(today, -(months - 1)));
    let mut budgets = Vec::new();
    for offset in 0..months {
        let month = first + offset;
        for (category, assigned) in plan {
            budgets.push(BudgetEntry {
                category,
                month,
                assigned,
                rollover: matches!(category, c if c == cats.car),
            });
        }
    }
    budgets
}

/// The rules a person would have written after a month of imports.
fn generate_rules(cats: &Cats) -> Vec<Rule> {
    let rules = [
        ("Albert Heijn", "AH TO GO", Some(cats.groceries), Some("Albert Heijn")),
        ("Shell", "SHELL NEDERLAND", Some(cats.fuel), Some("Shell")),
        ("NS", "NS GROEP", Some(cats.transit), Some("NS Reizigers")),
        ("Netflix", "NETFLIX.COM", Some(cats.streaming), Some("Netflix")),
        ("Amazon", "AMZN MKTP", Some(cats.household), Some("Amazon")),
    ];
    let mut generated = Vec::new();
    for (index, (name, pattern, category, rename)) in rules.into_iter().enumerate() {
        generated.push(Rule {
            id: index as Id + 1,
            name: name.to_string(),
            match_on: MatchOn::Raw,
            how: MatchHow::Contains,
            pattern: pattern.to_string(),
            amount_min: 0,
            amount_max: 0,
            set_category: category,
            rename_payee: rename.map(str::to_string),
            set_memo: None,
            flag: false,
            priority: index as i32,
            enabled: true,
            hits: 0,
        });
    }
    generated
}

/// The recurring bills, as the app's detector would have found them.
fn generate_scheduled(accounts: &Accounts, cats: &Cats, today: Day) -> Vec<Scheduled> {
    let next = |day: u32| -> Day {
        let (y, m, _) = date::to_ymd(today);
        let candidate = date::from_ymd(y, m, day.min(date::days_in_month(y, m)));
        if candidate >= today {
            candidate
        } else {
            date::add_months(candidate, 1)
        }
    };
    let items = [
        (accounts.checking, "Rabobank hypotheek", -92_400, cats.housing, next(2)),
        (accounts.checking, "Kinderopvang Zonnetje", -54_000, cats.childcare, next(2)),
        (accounts.checking, "Eneco", -14_000, cats.utilities, next(3)),
        (accounts.checking, "KPN Internet", -5_450, cats.internet, next(5)),
        (accounts.checking, "Vodafone", -2_890, cats.phone, next(8)),
        (accounts.card, "Netflix", -1_399, cats.streaming, next(4)),
        (accounts.card, "Spotify", -1_099, cats.streaming, next(7)),
        (accounts.card, "SportCity", -2_995, cats.gym, next(18)),
        (accounts.checking, "Bergman Design BV", 492_400, cats.salary, next(25)),
    ];
    items
        .into_iter()
        .enumerate()
        .map(|(index, (account, payee, amount, category, due))| Scheduled {
            id: index as Id + 1,
            account,
            payee: payee.to_string(),
            amount,
            category: Some(category),
            recurrence: Recurrence::Monthly,
            next_due: due,
            last_posted: None,
            auto_post: false,
            enabled: true,
            detected: true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn fingerprint(ledger: &Ledger) -> u64 {
        // Every model here derives Debug over all of its named fields. Sort
        // each table by its durable key, then length-frame and hash those
        // complete structural records so neither ordering nor concatenation
        // ambiguity can hide a difference.
        let mut rows = vec![format!("currency:{:?}", ledger.base_currency)];

        let mut accounts: Vec<_> = ledger.accounts.iter().collect();
        accounts.sort_by_key(|account| account.id);
        rows.extend(accounts.into_iter().map(|account| format!("account:{account:?}")));

        let mut categories: Vec<_> = ledger.categories.categories.iter().collect();
        categories.sort_by_key(|category| category.id);
        rows.extend(categories.into_iter().map(|category| format!("category:{category:?}")));

        let mut transactions = ledger.transactions.clone();
        transactions.sort_by_key(|txn| txn.id);
        for txn in &mut transactions {
            txn.splits.sort_by_key(|split| split.id);
        }
        rows.extend(transactions.into_iter().map(|txn| format!("transaction:{txn:?}")));

        let mut payees: Vec<_> = ledger.payees.iter().collect();
        payees.sort_by_key(|payee| payee.id);
        rows.extend(payees.into_iter().map(|payee| format!("payee:{payee:?}")));

        let mut budgets = ledger.budgets.clone();
        budgets.sort_by_key(|budget| (budget.category, budget.month));
        rows.extend(budgets.into_iter().map(|budget| format!("budget:{budget:?}")));

        let mut rules: Vec<_> = ledger.rules.iter().collect();
        rules.sort_by_key(|rule| rule.id);
        rows.extend(rules.into_iter().map(|rule| format!("rule:{rule:?}")));

        let mut scheduled: Vec<_> = ledger.scheduled.iter().collect();
        scheduled.sort_by_key(|item| item.id);
        rows.extend(scheduled.into_iter().map(|item| format!("scheduled:{item:?}")));

        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for row in rows {
            for byte in (row.len() as u64).to_le_bytes().into_iter().chain(row.bytes()) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    fn entity_ids(ledger: &Ledger) -> Vec<(&'static str, Vec<Id>)> {
        vec![
            ("accounts", ledger.accounts.iter().map(|item| item.id).collect()),
            (
                "categories",
                ledger.categories.categories.iter().map(|item| item.id).collect(),
            ),
            ("transactions", ledger.transactions.iter().map(|item| item.id).collect()),
            (
                "splits",
                ledger
                    .transactions
                    .iter()
                    .flat_map(|txn| txn.splits.iter().map(|split| split.id))
                    .collect(),
            ),
            ("payees", ledger.payees.iter().map(|item| item.id).collect()),
            ("rules", ledger.rules.iter().map(|item| item.id).collect()),
            ("scheduled", ledger.scheduled.iter().map(|item| item.id).collect()),
        ]
    }

    fn assert_unique_ids(ledger: &Ledger) {
        for (kind, ids) in entity_ids(ledger) {
            let mut unique = HashSet::new();
            for id in ids {
                assert_ne!(id, NO_ID, "{kind} contains an unassigned id");
                assert!(unique.insert(id), "duplicate {kind} id {id}");
            }
        }
        let budget_keys: HashSet<_> =
            ledger.budgets.iter().map(|entry| (entry.category, entry.month)).collect();
        assert_eq!(budget_keys.len(), ledger.budgets.len(), "duplicate budget key");
    }

    fn assert_references_resolve(ledger: &Ledger) {
        let accounts: HashSet<_> = ledger.accounts.iter().map(|account| account.id).collect();
        let categories: HashSet<_> =
            ledger.categories.categories.iter().map(|category| category.id).collect();
        for category in &ledger.categories.categories {
            if let Some(parent) = category.parent {
                assert!(categories.contains(&parent), "missing parent category {parent}");
            }
        }
        for txn in &ledger.transactions {
            assert!(accounts.contains(&txn.account), "missing transaction account {}", txn.account);
            if let Some(category) = txn.category {
                assert!(categories.contains(&category), "missing transaction category {category}");
            }
            for split in &txn.splits {
                if let Some(category) = split.category {
                    assert!(categories.contains(&category), "missing split category {category}");
                }
            }
        }
        for payee in &ledger.payees {
            if let Some(category) = payee.default_category {
                assert!(categories.contains(&category), "missing payee category {category}");
            }
        }
        for budget in &ledger.budgets {
            assert!(categories.contains(&budget.category), "missing budget category");
        }
        for rule in &ledger.rules {
            if let Some(category) = rule.set_category {
                assert!(categories.contains(&category), "missing rule category {category}");
            }
        }
        for item in &ledger.scheduled {
            assert!(accounts.contains(&item.account), "missing scheduled account");
            if let Some(category) = item.category {
                assert!(categories.contains(&category), "missing scheduled category");
            }
        }
    }

    #[test]
    fn the_demo_file_is_a_coherent_household() {
        let today = date::from_ymd(2026, 8, 28);
        let ledger = generate(DEFAULT_YEARS, today);

        // Enough to fill every screen.
        assert!(
            ledger.transactions.len() > 1_500,
            "two years should be thousands of rows, got {}",
            ledger.transactions.len()
        );
        assert_eq!(ledger.accounts.len(), 7);
        assert!(ledger.categories.categories.len() > 25);
        assert!(!ledger.budgets.is_empty());
        assert!(!ledger.rules.is_empty());
        assert!(!ledger.scheduled.is_empty());

        // Every transfer pair balances — the invariant the whole
        // net-worth number rests on.
        let mut groups: HashMap<Id, Vec<&Transaction>> = HashMap::new();
        for txn in ledger.transactions.iter().filter(|txn| txn.transfer_group.is_some()) {
            groups.entry(txn.transfer_group.unwrap()).or_default().push(txn);
        }
        assert!(groups.len() > 20, "expected many transfers, got {}", groups.len());
        for (group, rows) in groups {
            assert_eq!(rows.len(), 2, "transfer {group} must have exactly two rows");
            assert_eq!(rows[0].amount, -rows[1].amount, "transfer {group} must cancel");
        }

        assert_unique_ids(&ledger);
        assert_references_resolve(&ledger);

        // Splits sum to their transaction.
        let split_count = ledger.transactions.iter().filter(|t| t.is_split()).count();
        assert!(split_count >= 5, "expected split examples, got {split_count}");
        for txn in ledger.transactions.iter().filter(|t| t.is_split()) {
            assert_eq!(txn.split_imbalance(), 0);
        }

        // The story adds up: income arrives, the current account stays
        // solvent, and the mortgage is smaller than it started.
        let checking = ledger.accounts.iter().find(|a| a.name == "Everyday").unwrap();
        assert!(ledger.balance(checking.id) > 0, "the household should not be overdrawn");
        let mortgage = ledger.accounts.iter().find(|a| a.name == "Mortgage").unwrap();
        assert!(
            ledger.balance(mortgage.id) > mortgage.opening_balance,
            "the mortgage should have been paid down"
        );
        assert!(ledger.net_worth_on(today) > 0);

        // Nothing in the future, and history reaches back two years.
        assert!(ledger.transactions.iter().all(|t| t.date <= today));
        let oldest = ledger.transactions.iter().map(|t| t.date).min().unwrap();
        assert!(today - oldest > 660, "expected ~2 years of history");

        // The recent tail is still uncleared, the deep past is reconciled.
        assert!(ledger
            .transactions
            .iter()
            .any(|t| t.cleared == Cleared::Uncleared));
        assert!(ledger
            .transactions
            .iter()
            .any(|t| t.cleared == Cleared::Reconciled));
        assert!(ledger.cleared_balance(checking.id) != ledger.balance(checking.id));
    }

    #[test]
    fn the_same_seed_produces_the_same_file() {
        let today = date::from_ymd(2026, 8, 28);
        let one = generate(2, today);
        let two = generate(2, today);
        assert_eq!(fingerprint(&one), fingerprint(&two));
        assert_eq!(entity_ids(&one), entity_ids(&two), "generated ids must be stable");
        assert_unique_ids(&one);
        assert_unique_ids(&two);
    }
}
