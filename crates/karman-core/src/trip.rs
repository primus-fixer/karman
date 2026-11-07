//! Командировки: операции, статусы и расчёт с компанией по правилам из `docs/rules.md`.
//!
//! Коротко:
//! - аванс равен сумме заявки;
//! - суточные всегда по ставке (по умолчанию 700 ₽ в день), без перерасчётов;
//! - рабочий расход с документом идёт в авансовый отчёт;
//! - питание и рабочие траты без документа вычитаются из суточных, а то,
//!   что суточные не покрыли, становится долгом перед компанией;
//! - неизрасходованный аванс возвращается, перерасход по документам компания возмещает.

use crate::date::Date;
use crate::money::Kop;
use serde::{Deserialize, Serialize};

/// Ставка суточных по умолчанию, 700 ₽ в день.
pub const DEFAULT_RATE: Kop = 70_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    /// Рабочий расход в поездке (в отчёт входит только с документом).
    Work,
    /// Питание в поездке, за счёт суточных.
    Food,
    /// Личный расход.
    Personal,
    /// Личный доход.
    Income,
    /// Аванс от компании.
    Advance,
    /// Возмещение от компании.
    Reimbursement,
    /// Возврат денег компании.
    Return,
    /// Платёж по личному долгу или возврат долга мне.
    Debt,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    Photo,
    Qr,
    File,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Op {
    pub id: String,
    pub date: Date,
    pub kind: OpKind,
    pub cat: String,
    pub title: String,
    /// В рублях (копейках), уже пересчитано из валюты счёта.
    pub amount: Kop,
    pub account: String,
    #[serde(default)]
    pub trip: Option<String>,
    #[serde(default)]
    pub doc: Option<DocKind>,
    #[serde(default)]
    pub sent: bool,
    #[serde(default)]
    pub family: bool,
    #[serde(default)]
    pub debt: Option<String>,
    #[serde(default)]
    pub inflow: bool,
    /// Сумма в валюте счёта, если счёт не в рублях.
    #[serde(default)]
    pub orig: Option<Kop>,
    #[serde(default)]
    pub cur: Option<String>,
    /// Комментарий к операции.
    #[serde(default)]
    pub note: String,
}

impl Op {
    /// Приход на счёт (доход, аванс, возмещение, возврат долга мне).
    pub fn is_inflow(&self) -> bool {
        matches!(self.kind, OpKind::Income | OpKind::Advance | OpKind::Reimbursement) || (self.kind == OpKind::Debt && self.inflow)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripStatus {
    Request = 0,
    AdvanceReceived = 1,
    InTrip = 2,
    AwaitingReport = 3,
    Review = 4,
    Closed = 5,
}

impl TripStatus {
    pub fn title(self) -> &'static str {
        match self {
            TripStatus::Request => "Заявка",
            TripStatus::AdvanceReceived => "Аванс получен",
            TripStatus::InTrip => "В поездке",
            TripStatus::AwaitingReport => "Ожидает отчёт",
            TripStatus::Review => "На проверке",
            TripStatus::Closed => "Закрыта",
        }
    }
}

/// Статьи авансового отчёта в 1С.
pub const ARTICLES: [(&str, &str); 5] = [
    ("03.01.04.01", "Билеты"),
    ("03.01.04.02", "Представительские"),
    ("03.01.04.03", "Проживание"),
    ("03.01.04.04", "Суточные"),
    ("03.01.04.05", "Транспорт, бензин"),
];
pub const PER_DIEM_ARTICLE: usize = 3;

/// Статья для категории рабочего расхода. Материалы и инструменты — в статью 05.
pub fn article_of(cat: &str) -> Option<usize> {
    match cat {
        "tickets" => Some(0),
        "lodging" => Some(2),
        "taxi" | "car" | "tools" => Some(4),
        _ => None,
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Trip {
    pub id: String,
    pub city: String,
    /// Фактические даты.
    pub start: Date,
    pub end: Date,
    /// Даты по заявке.
    pub plan_start: Date,
    pub plan_end: Date,
    pub status: TripStatus,
    /// Заявка по статьям 01–05. Аванс равен её сумме.
    pub plan: [Kop; 5],
    /// Ставка, зафиксированная в сданном отчёте.
    #[serde(default)]
    pub fixed_rate: Option<Kop>,
    /// Своя ставка поездки — важнее настроек и зафиксированной.
    #[serde(default)]
    pub own_rate: Option<Kop>,
    /// Номер заявки в 1С.
    #[serde(default)]
    pub req_no: String,
    #[serde(default)]
    pub req_date: Option<Date>,
    /// «Описание цели» из заявки.
    #[serde(default)]
    pub goal: String,
    /// Регион для статьи «Проживание».
    #[serde(default)]
    pub region: String,
    /// Примечания заявки и авансового отчёта по статьям.
    #[serde(default)]
    pub plan_notes: [String; 5],
    #[serde(default)]
    pub notes: [String; 5],
    /// Срок сдачи отчёта.
    #[serde(default)]
    pub deadline: Option<Date>,
    /// Своё обоснование сверхбюджета вместо подставленного.
    #[serde(default)]
    pub just: Option<String>,
}

impl Trip {
    pub fn plan_total(&self) -> Kop {
        self.plan.iter().sum()
    }
}

/// Какая ставка суточных действует для поездки: своя у поездки, затем
/// зафиксированная в сданном отчёте, затем ставка из настроек.
pub fn effective_rate(trip: &Trip, status: TripStatus, settings_rate: Kop, own_rate: Option<Kop>) -> Kop {
    rate_with_source(trip, status, settings_rate, own_rate).0
}

/// Ставка и откуда она: «own» — своя у поездки, «fixed» — из сданного отчёта, «settings» — из настроек.
pub fn rate_with_source(trip: &Trip, status: TripStatus, settings_rate: Kop, own_rate: Option<Kop>) -> (Kop, &'static str) {
    if let Some(r) = own_rate {
        return (r, "own");
    }
    if status >= TripStatus::Review {
        return (trip.fixed_rate.unwrap_or(DEFAULT_RATE), "fixed");
    }
    (settings_rate, "settings")
}

impl TripStatus {
    pub fn from_index(i: u8) -> Option<TripStatus> {
        Some(match i {
            0 => TripStatus::Request,
            1 => TripStatus::AdvanceReceived,
            2 => TripStatus::InTrip,
            3 => TripStatus::AwaitingReport,
            4 => TripStatus::Review,
            5 => TripStatus::Closed,
            _ => return None,
        })
    }

    pub fn next(self) -> Option<TripStatus> {
        TripStatus::from_index(self as u8 + 1)
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Settlement {
    pub days: i32,
    pub plan_days: i32,
    pub elapsed: i32,
    pub finished: bool,
    pub rate: Kop,
    /// Суточные, которые уже можно засчитать: за всю поездку после её конца, иначе за прошедшие дни.
    pub per_diem: Kop,
    pub per_diem_full: Kop,
    pub issued: Kop,
    pub reimbursed: Kop,
    pub returned: Kop,
    /// Рабочие расходы с документами.
    pub confirmed: Kop,
    /// Рабочие расходы без документов.
    pub no_doc: Kop,
    pub no_doc_count: usize,
    pub food: Kop,
    /// Личные траты с меткой поездки (в расчёт не входят).
    pub tagged: Kop,
    /// > 0: подотчётные (в поездке) или вернуть компании; < 0: компания должна.
    pub balance: Kop,
    /// Остаток суточных после питания и трат без документов; < 0 — не хватило.
    pub leftover: Kop,
    /// Траты, которые суточные не покрыли.
    pub debt: Kop,
    /// Та часть долга, которую нужно вернуть компании.
    pub debt_due: Kop,
    /// Траты сверх суточных, оплаченные своими деньгами: компания их не возместит.
    pub lost: Kop,
    pub days_left: i32,
}

/// Расчёт поездки на дату `today`.
pub fn settle(trip: &Trip, status: TripStatus, ops: &[Op], rate: Kop, today: Date) -> Settlement {
    let days = trip.start.days_until(trip.end) + 1;
    let plan_days = trip.plan_start.days_until(trip.plan_end) + 1;
    let elapsed = if today < trip.start {
        0
    } else if today > trip.end {
        days
    } else {
        trip.start.days_until(today) + 1
    };
    let finished = status >= TripStatus::AwaitingReport;
    let per_diem_full = days as Kop * rate;
    let per_diem = if finished {
        per_diem_full
    } else if status >= TripStatus::InTrip {
        elapsed as Kop * rate
    } else {
        0
    };
    let mine: Vec<&Op> = ops.iter().filter(|o| o.trip.as_deref() == Some(trip.id.as_str())).collect();
    let sum = |f: &dyn Fn(&Op) -> bool| -> Kop { mine.iter().filter(|o| f(o)).map(|o| o.amount).sum() };
    let issued = sum(&|o| o.kind == OpKind::Advance);
    let reimbursed = sum(&|o| o.kind == OpKind::Reimbursement);
    let returned = sum(&|o| o.kind == OpKind::Return);
    let confirmed = sum(&|o| o.kind == OpKind::Work && o.doc.is_some());
    let no_doc = sum(&|o| o.kind == OpKind::Work && o.doc.is_none());
    let no_doc_count = mine.iter().filter(|o| o.kind == OpKind::Work && o.doc.is_none()).count();
    let food = sum(&|o| o.kind == OpKind::Food);
    let tagged = sum(&|o| o.kind == OpKind::Personal);
    let balance = issued + reimbursed - returned - confirmed - per_diem;
    let leftover = per_diem_full - food - no_doc;
    let debt = (-leftover).max(0);
    let final_balance = issued + reimbursed - returned - confirmed - per_diem_full;
    let debt_due = debt.min(final_balance.max(0));
    let lost = debt - debt_due;
    let days_left = if finished { 0 } else { today.days_until(trip.end).max(0) };
    Settlement {
        days,
        plan_days,
        elapsed,
        finished,
        rate,
        per_diem,
        per_diem_full,
        issued,
        reimbursed,
        returned,
        confirmed,
        no_doc,
        no_doc_count,
        food,
        tagged,
        balance,
        leftover,
        debt,
        debt_due,
        lost,
        days_left,
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ReportRow {
    pub code: String,
    pub name: String,
    pub plan: Kop,
    pub fact: Kop,
    /// Сверх заявки по статье.
    pub over: Kop,
}

/// Строки авансового отчёта: план из заявки, факт — расходы с документами и суточные.
pub fn report(trip: &Trip, s: &Settlement, ops: &[Op]) -> Vec<ReportRow> {
    ARTICLES
        .iter()
        .enumerate()
        .map(|(i, (code, name))| {
            let fact = if i == PER_DIEM_ARTICLE {
                s.per_diem
            } else {
                ops.iter()
                    .filter(|o| o.trip.as_deref() == Some(trip.id.as_str()) && o.kind == OpKind::Work && o.doc.is_some() && article_of(&o.cat) == Some(i))
                    .map(|o| o.amount)
                    .sum()
            };
            let plan = trip.plan[i];
            ReportRow { code: code.to_string(), name: name.to_string(), plan, fact, over: (fact - plan).max(0) }
        })
        .collect()
}

/// Обоснование для полей «Сверх бюджета» и «Обоснование» в заявке.
pub fn over_budget_note(trip: &Trip, s: &Settlement, rows: &[ReportRow]) -> String {
    let parts: Vec<String> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.over > 0)
        .map(|(i, r)| {
            if i == PER_DIEM_ARTICLE && s.days > s.plan_days {
                format!(
                    "Суточные: {} вместо {} по плану, поездка продлилась до {}",
                    days_text(s.days),
                    s.plan_days,
                    trip.end.day_month()
                )
            } else {
                format!("«{}»: на {} больше плана", r.name, crate::money::rub_text(r.over))
            }
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        parts.join("; ") + "."
    }
}

/// «11 дней», «2 дня», «1 день»
pub fn days_text(n: i32) -> String {
    let m = n.rem_euclid(100);
    let l = n.rem_euclid(10);
    let w = if (11..=14).contains(&m) {
        "дней"
    } else if l == 1 {
        "день"
    } else if (2..=4).contains(&l) {
        "дня"
    } else {
        "дней"
    };
    format!("{n}\u{a0}{w}")
}

/// Короткая сводка для экрана: что с деньгами по поездке.
pub fn headline(s: &Settlement) -> String {
    use crate::money::rub_text;
    if s.balance > 0 && !s.finished {
        format!("Подотчётные {}", rub_text(s.balance))
    } else if s.balance > 0 && s.debt_due > 0 {
        format!(
            "Вернуть компании {}: остаток аванса {} и долг {}",
            rub_text(s.balance),
            rub_text(s.balance - s.debt_due),
            rub_text(s.debt_due)
        )
    } else if s.balance > 0 {
        format!("Вернуть компании {}", rub_text(s.balance))
    } else if s.balance < 0 {
        format!("Компания должна {}", rub_text(-s.balance))
    } else {
        "Рассчитано".to_string()
    }
}

/// Сводка по суточным.
pub fn per_diem_line(s: &Settlement) -> String {
    use crate::money::rub_text;
    if s.debt_due > 0 {
        format!("Долг перед компанией {}", rub_text(s.debt_due))
    } else if s.debt > 0 {
        format!("Сверх суточных из своих {}", rub_text(s.debt))
    } else if s.finished {
        format!("Осталось от суточных {}", rub_text(s.leftover))
    } else {
        format!("Останется от суточных {}", rub_text(s.leftover))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;
    use crate::money::rub;

    fn calc(id: &str) -> (Trip, Settlement, Vec<Op>) {
        let trips = demo::trips();
        let ops = demo::ops();
        let t = trips.into_iter().find(|t| t.id == id).unwrap();
        let rate = effective_rate(&t, t.status, DEFAULT_RATE, None);
        let s = settle(&t, t.status, &ops, rate, demo::today());
        (t, s, ops)
    }

    #[test]
    fn trip_573_matches_1c() {
        let (t, s, ops) = calc("573");
        assert_eq!(t.plan_total(), rub(122000.0));
        assert_eq!(s.issued, rub(122000.0), "аванс равен заявке");
        assert_eq!(s.days, 11);
        assert_eq!(s.per_diem, rub(7700.0));
        assert_eq!(s.confirmed, rub(47941.10));
        assert_eq!(s.balance, rub(66358.90), "вернуть компании, как в 1С");
        assert_eq!(s.leftover, rub(1460.0));
        assert_eq!(s.debt, 0);
        let rows = report(&t, &s, &ops);
        let fact: Kop = rows.iter().map(|r| r.fact).sum();
        assert_eq!(fact, rub(55641.10));
        assert_eq!(rows[0].fact, rub(11741.10));
        assert_eq!(rows[2].fact, rub(35000.0));
        assert_eq!(rows[4].fact, rub(1200.0));
        assert_eq!(rows[3].over, rub(700.0), "сверх бюджета по суточным");
        assert_eq!(
            over_budget_note(&t, &s, &rows),
            "Суточные: 11\u{a0}дней вместо 10 по плану, поездка продлилась до 31\u{a0}октября."
        );
    }

    #[test]
    fn trip_552_debt_beyond_per_diem() {
        let (_, s, _) = calc("552");
        assert_eq!(s.balance, rub(9500.0));
        assert_eq!(s.leftover, -rub(8600.0));
        assert_eq!(s.debt, rub(8600.0));
        assert_eq!(s.debt_due, rub(8600.0));
        assert_eq!(s.balance - s.debt_due, rub(900.0), "остаток аванса");
        assert_eq!(s.lost, 0);
        assert_eq!(s.no_doc_count, 3);
    }

    #[test]
    fn trip_561_company_owes() {
        let (t, s, ops) = calc("561");
        assert_eq!(s.balance, -rub(1000.0));
        assert_eq!(s.leftover, rub(4200.0));
        let rows = report(&t, &s, &ops);
        assert_eq!(rows[4].over, rub(1000.0));
    }

    #[test]
    fn trip_540_closed_with_return() {
        let (_, s, _) = calc("540");
        assert_eq!(s.returned, rub(4100.0));
        assert_eq!(s.balance, 0);
        assert_eq!(s.leftover, rub(800.0));
    }

    #[test]
    fn trip_588_in_progress() {
        let (t, s, ops) = calc("588");
        assert_eq!(s.elapsed, 4);
        assert_eq!(s.per_diem, rub(2800.0));
        assert_eq!(s.balance, rub(27300.0), "подотчётные");
        assert_eq!(s.leftover, rub(5910.0), "останется от суточных");
        assert_eq!(s.days_left, 8);
        let rows = report(&t, &s, &ops);
        assert_eq!(rows[4].fact, rub(1500.0), "такси и материалы в статье 05");
    }

    #[test]
    fn rate_rules() {
        let trips = demo::trips();
        let t573 = trips.iter().find(|t| t.id == "573").unwrap();
        let t552 = trips.iter().find(|t| t.id == "552").unwrap();
        assert_eq!(effective_rate(t573, t573.status, rub(800.0), None), rub(800.0), "отчёт не сдан — ставка из настроек");
        assert_eq!(effective_rate(t552, t552.status, rub(800.0), None), rub(700.0), "отчёт сдан — ставка зафиксирована");
        assert_eq!(effective_rate(t552, t552.status, rub(800.0), Some(rub(900.0))), rub(900.0), "своя ставка поездки");
        let s = settle(t573, t573.status, &demo::ops(), rub(800.0), demo::today());
        assert_eq!(s.balance, rub(65258.90));
    }

    #[test]
    fn own_money_beyond_advance_is_lost_not_owed() {
        let mut t = demo::trips().into_iter().find(|t| t.id == "552").unwrap();
        t.plan = [rub(16000.0), 0, rub(10000.0), rub(3500.0), rub(500.0)];
        let mut ops: Vec<Op> = demo::ops().into_iter().filter(|o| o.trip.as_deref() == Some("552")).collect();
        for o in ops.iter_mut() {
            if o.kind == OpKind::Advance {
                o.amount = rub(30000.0);
            }
        }
        let s = settle(&t, t.status, &ops, DEFAULT_RATE, demo::today());
        assert_eq!(s.balance, 0);
        assert_eq!(s.debt, rub(8600.0));
        assert_eq!(s.debt_due, 0);
        assert_eq!(s.lost, rub(8600.0));
    }

    #[test]
    fn days_words() {
        assert_eq!(days_text(1), "1\u{a0}день");
        assert_eq!(days_text(3), "3\u{a0}дня");
        assert_eq!(days_text(11), "11\u{a0}дней");
        assert_eq!(days_text(21), "21\u{a0}день");
    }
}
