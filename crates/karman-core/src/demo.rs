//! Демо-данные — те же, что в макете. Настоящие здесь только суммы заявки 573 из 1С.

use crate::book::{self, Account};
use crate::date::Date;
use crate::budget::{self, Budget};
use crate::finance::{Debt, DebtDir, Every, Plan};
use crate::savings::{Deposit, Goal, KIND_DEPOSIT, KIND_SAVINGS};
use std::collections::BTreeMap;
use crate::money::rub;
use crate::sync::EventBody;
use crate::trip::{DocKind, Op, OpKind, Trip, TripStatus, DEFAULT_RATE};

/// «Сегодня» в демо-данных.
pub fn today() -> Date {
    d("2026-11-06")
}

fn d(s: &str) -> Date {
    Date::parse(s).expect("дата в демо-данных")
}

fn trip(id: &str, city: &str, dates: [&str; 4], status: TripStatus, plan: [f64; 5], fixed: bool) -> Trip {
    Trip {
        id: id.into(),
        city: city.into(),
        start: d(dates[0]),
        end: d(dates[1]),
        plan_start: d(dates[2]),
        plan_end: d(dates[3]),
        status,
        plan: plan.map(rub),
        fixed_rate: if fixed { Some(DEFAULT_RATE) } else { None },
        own_rate: None,
        req_no: String::new(),
        req_date: None,
        goal: String::new(),
        region: String::new(),
        plan_notes: Default::default(),
        notes: Default::default(),
        deadline: None,
        just: None,
        extra: Default::default(),
    }
}

pub fn trips() -> Vec<Trip> {
    vec![
        trip("588", "Сургут", ["2026-11-03", "2026-11-14", "2026-11-03", "2026-11-14"], TripStatus::InTrip, [28000.0, 0.0, 24000.0, 8400.0, 1600.0], false),
        trip("573", "Чекмагуш", ["2026-10-21", "2026-10-31", "2026-10-21", "2026-10-30"], TripStatus::AwaitingReport, [60000.0, 0.0, 50000.0, 7000.0, 5000.0], false),
        trip("561", "Нижневартовск", ["2026-09-28", "2026-10-09", "2026-09-28", "2026-10-09"], TripStatus::Review, [19700.0, 0.0, 19800.0, 8400.0, 400.0], true),
        trip("552", "Ноябрьск", ["2026-09-15", "2026-09-19", "2026-09-15", "2026-09-19"], TripStatus::Review, [16000.0, 0.0, 10000.0, 3500.0, 10000.0], true),
        trip("540", "Тобольск", ["2026-09-01", "2026-09-09", "2026-09-01", "2026-09-09"], TripStatus::Closed, [14200.0, 0.0, 14400.0, 6300.0, 6100.0], true),
    ]
}

// (дата, вид, категория, название, сумма, счёт, поездка, документ)
type Row = (&'static str, OpKind, &'static str, &'static str, f64, &'static str, Option<&'static str>, Option<DocKind>);

const ROWS: &[Row] = {
    use DocKind::*;
    use OpKind::*;
    &[
        ("2026-11-06", Work, "taxi", "Такси до объекта", 640.0, "work", Some("588"), Some(Qr)),
        ("2026-11-06", Food, "food", "Столовая", 480.0, "personal", Some("588"), None),
        ("2026-11-06", Personal, "gifts", "Сувениры домой", 1250.0, "personal", Some("588"), None),
        ("2026-11-05", Work, "lodging", "Гостиница, предоплата", 12000.0, "work", Some("588"), Some(File)),
        ("2026-11-05", Work, "tools", "Изолента и стяжки", 860.0, "work", Some("588"), Some(Qr)),
        ("2026-11-05", Food, "food", "Кафе", 260.0, "personal", Some("588"), None),
        ("2026-11-04", Work, "taxi", "Такси от вокзала", 1210.0, "personal", Some("588"), None),
        ("2026-11-04", Food, "food", "Продукты", 540.0, "personal", Some("588"), None),
        ("2026-11-03", Work, "tickets", "Билеты до Сургута", 18400.0, "work", Some("588"), Some(File)),
        ("2026-11-02", Advance, "advance", "Аванс по заявке 588", 62000.0, "work", Some("588"), None),
        ("2026-10-31", Work, "tickets", "Поезд из Уфы", 5870.55, "work", Some("573"), Some(File)),
        ("2026-10-31", Work, "taxi", "Автобус Чекмагуш – Уфа", 600.0, "cash", Some("573"), Some(Photo)),
        ("2026-10-30", Food, "food", "Столовая", 780.0, "personal", Some("573"), None),
        ("2026-10-29", Food, "food", "Столовая", 390.0, "personal", Some("573"), None),
        ("2026-10-28", Food, "food", "Кафе", 650.0, "personal", Some("573"), None),
        ("2026-10-26", Food, "food", "Продукты", 2140.0, "personal", Some("573"), None),
        ("2026-10-23", Food, "food", "Продукты", 1860.0, "personal", Some("573"), None),
        ("2026-10-22", Work, "lodging", "Съёмная квартира", 35000.0, "personal", Some("573"), Some(File)),
        ("2026-10-22", Food, "food", "Столовая", 420.0, "personal", Some("573"), None),
        ("2026-10-21", Work, "tickets", "Поезд до Уфы", 5870.55, "work", Some("573"), Some(File)),
        ("2026-10-21", Work, "taxi", "Автобус Уфа – Чекмагуш", 600.0, "cash", Some("573"), Some(Photo)),
        ("2026-10-20", Advance, "advance", "Аванс по заявке 573", 122000.0, "work", Some("573"), None),
        ("2026-10-09", Work, "tickets", "Поезд обратно", 9850.0, "work", Some("561"), Some(File)),
        ("2026-10-05", Food, "food", "Продукты", 2300.0, "personal", Some("561"), None),
        ("2026-10-02", Work, "taxi", "Такси на объект", 1400.0, "work", Some("561"), Some(Qr)),
        ("2026-10-01", Food, "food", "Столовая", 1900.0, "personal", Some("561"), None),
        ("2026-09-29", Work, "lodging", "Гостиница", 19800.0, "work", Some("561"), Some(File)),
        ("2026-09-28", Work, "tickets", "Поезд туда", 9850.0, "work", Some("561"), Some(File)),
        ("2026-09-26", Advance, "advance", "Аванс по заявке 561", 48300.0, "work", Some("561"), None),
        ("2026-09-19", Work, "tickets", "Поезд обратно", 7800.0, "work", Some("552"), Some(File)),
        ("2026-09-18", Food, "food", "Кафе", 600.0, "work", Some("552"), None),
        ("2026-09-18", Work, "taxi", "Такси обратно", 1400.0, "work", Some("552"), None),
        ("2026-09-18", Work, "taxi", "Такси до объекта", 900.0, "work", Some("552"), Some(Qr)),
        ("2026-09-17", Food, "food", "Продукты", 1100.0, "work", Some("552"), None),
        ("2026-09-17", Work, "taxi", "Такси", 2100.0, "work", Some("552"), None),
        ("2026-09-16", Food, "food", "Столовая", 900.0, "work", Some("552"), None),
        ("2026-09-16", Work, "lodging", "Гостиница", 10000.0, "work", Some("552"), Some(File)),
        ("2026-09-15", Work, "taxi", "Водитель до объекта, перевод", 6000.0, "work", Some("552"), None),
        ("2026-09-15", Work, "tickets", "Поезд туда", 7800.0, "work", Some("552"), Some(File)),
        ("2026-09-12", Advance, "advance", "Аванс по заявке 552", 39500.0, "work", Some("552"), None),
        ("2026-09-12", Return, "ret", "Возврат остатка, заявка 540", 4100.0, "work", Some("540"), None),
        ("2026-09-09", Work, "tickets", "Поезд обратно", 7100.0, "work", Some("540"), Some(File)),
        ("2026-09-08", Food, "food", "Продукты", 2600.0, "personal", Some("540"), None),
        ("2026-09-05", Work, "taxi", "Такси", 2000.0, "work", Some("540"), Some(Qr)),
        ("2026-09-04", Food, "food", "Столовая", 2900.0, "personal", Some("540"), None),
        ("2026-09-02", Work, "lodging", "Гостиница", 14400.0, "work", Some("540"), Some(File)),
        ("2026-09-01", Work, "tickets", "Поезд туда", 7100.0, "work", Some("540"), Some(File)),
        ("2026-08-30", Advance, "advance", "Аванс по заявке 540", 41000.0, "work", Some("540"), None),
    ]
};

pub fn ops() -> Vec<Op> {
    ROWS.iter()
        .enumerate()
        .map(|(i, r)| Op {
            id: format!("demo-{i}"),
            date: d(r.0),
            kind: r.1,
            cat: r.2.into(),
            title: r.3.into(),
            amount: rub(r.4),
            account: r.5.into(),
            trip: r.6.map(Into::into),
            doc: r.7,
            sent: false,
            family: false,
            debt: None,
            inflow: false,
            orig: None,
            cur: None,
            note: String::new(),
            att: Vec::new(),
            to: None,
            to_amount: None,
            plan: None,
            plan_date: None,
            goal: None,
            extra: Default::default(),
        })
        .collect()
}

fn own_op(id: &str, date: &str, kind: OpKind, cat: &str, title: &str, amount: f64) -> Op {
    Op {
        id: id.into(),
        date: d(date),
        kind,
        cat: cat.into(),
        title: title.into(),
        amount: rub(amount),
        account: "acc-personal".into(),
        trip: None,
        doc: None,
        sent: false,
        family: false,
        debt: None,
        inflow: false,
        orig: None,
        cur: None,
        note: String::new(),
        att: Vec::new(),
        to: None,
        to_amount: None,
        plan: None,
        plan_date: None,
        goal: None,
        extra: Default::default(),
    }
}

/// Демо-данные целиком, как события журнала: остатки счетов, счёт в тенге, поездки и операции из макета.
pub fn book_events() -> Vec<EventBody> {
    let mut bodies: Vec<EventBody> = Vec::new();
    // Личная карта: +25 000 к прежнему началу — ровно столько ушло на цель «Отпуск» (см. finance_events).
    for (id, opening) in [("acc-work", 2800.0), ("acc-personal", 86640.5), ("acc-cash", 12400.0)] {
        if let Some(mut a) = book::default_accounts().into_iter().find(|a| a.id == id) {
            a.opening = rub(opening);
            bodies.push(EventBody::AccountPut { account: a });
        }
    }
    bodies.push(EventBody::AccountPut {
        account: Account { id: "acc-kzt".into(), name: "Наличные, тенге".into(), short: "₸".into(), currency: "KZT".into(), opening: rub(18000.0), kind: "cash".into(), order: 4, archived: false, deposit: None, extra: Default::default() },
    });
    bodies.extend(trips().into_iter().map(|trip| EventBody::TripPut { trip }));
    bodies.extend(ops().into_iter().map(|mut op| {
        op.account = format!("acc-{}", op.account);
        EventBody::OpPut { op }
    }));
    for op in [
        own_op("p1", "2026-11-05", OpKind::Personal, "telecom", "Музыка, подписка", 299.0),
        own_op("p2", "2026-11-02", OpKind::Personal, "groceries", "Супермаркет", 3480.0),
        own_op("p3", "2026-11-01", OpKind::Personal, "home", "Коммунальные платежи", 7240.0),
        own_op("p4", "2026-10-30", OpKind::Income, "salary", "Зарплата", 78000.0),
    ] {
        bodies.push(EventBody::OpPut { op });
    }
    bodies.extend(finance_events());
    bodies
}

/// Долги и регулярные платежи для макета: кредит с автоплатежом, долг мне со сроком, два платежа.
pub fn finance_events() -> Vec<EventBody> {
    let debt = |id: &str, who: &str, dir: DebtDir, amount: f64, date: &str, due: Option<&str>| Debt {
        id: id.into(),
        who: who.into(),
        dir,
        amount: rub(amount),
        date: d(date),
        due: due.map(d),
        note: String::new(),
        closed: false,
        extra: Default::default(),
    };
    let plan = |id: &str, title: &str, amount: f64, cat: &str, start: &str, auto: bool, debt: Option<&str>| Plan {
        id: id.into(),
        title: title.into(),
        amount: rub(amount),
        account: "acc-personal".into(),
        cat: cat.into(),
        every: Every::Month,
        start: d(start),
        until: None,
        auto,
        remind: None,
        debt: debt.map(String::from),
        skipped: Vec::new(),
        paused: false,
        note: String::new(),
        extra: Default::default(),
    };
    vec![
        EventBody::DebtPut { debt: debt("debt-sber", "Сбербанк, кредит", DebtDir::IOwe, 240000.0, "2026-03-01", None) },
        EventBody::DebtPut { debt: debt("debt-andrey", "Андрей", DebtDir::OwedToMe, 5000.0, "2026-10-20", Some("2026-11-08")) },
        EventBody::PlanPut { plan: plan("plan-sber", "Кредит Сбербанк", 12400.0, "", "2026-11-15", true, Some("debt-sber")) },
        EventBody::PlanPut { plan: plan("plan-net", "Интернет и телефон", 890.0, "telecom", "2026-11-09", false, None) },
        EventBody::PlanPut { plan: plan("plan-garage", "Аренда гаража", 3000.0, "home", "2026-11-25", false, None) },
    ]
    .into_iter()
    .chain(savings_events())
    .collect()
}

/// Копилка с целью «Отпуск», вклад с капитализацией и бюджеты на месяц.
fn savings_events() -> Vec<EventBody> {
    let mut save = Account { id: "acc-save".into(), name: "Копилка".into(), short: "Копилка".into(), currency: "RUB".into(), opening: rub(20000.0), kind: KIND_SAVINGS.into(), order: 5, archived: false, deposit: None, extra: Default::default() };
    let mut dep = Account { id: "acc-dep".into(), name: "Вклад «Надёжный»".into(), short: "Вклад".into(), currency: "RUB".into(), opening: rub(300000.0), kind: KIND_DEPOSIT.into(), order: 6, archived: false, deposit: None, extra: Default::default() };
    dep.deposit = Some(Deposit { rate: 16.0, opened: d("2026-05-15"), until: Some(d("2027-05-15")), capitalize: true });
    save.order = 5;
    let goal = Goal { id: "goal-trip".into(), title: "Отпуск летом".into(), target: rub(150000.0), due: Some(d("2027-06-01")), account: "acc-save".into(), done: false, note: String::new(), extra: Default::default() };
    let mut put = own_op("g1", "2026-10-15", OpKind::Transfer, "transfer", "На цель: Отпуск летом", 25000.0);
    put.to = Some("acc-save".into());
    put.goal = Some("goal-trip".into());
    let personal = Budget { id: budget::PERSONAL.into(), total: Some(rub(60000.0)), cats: BTreeMap::from([("groceries".to_string(), rub(15000.0)), ("cafe".to_string(), rub(4000.0)), ("home".to_string(), rub(7000.0))]), extra: Default::default() };
    vec![
        EventBody::AccountPut { account: save },
        EventBody::AccountPut { account: dep },
        EventBody::GoalPut { goal },
        EventBody::OpPut { op: put },
        EventBody::BudgetPut { budget: personal },
    ]
}
