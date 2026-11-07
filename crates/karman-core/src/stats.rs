//! Статистика: расходы по категориям, доходы и расходы по месяцам, командировки за год.
//!
//! Правила те же, что у бюджета и «Моих денег»: расходы — личные и семейные (в том числе
//! с меткой поездки и оплаты по графику); доходы — зарплата, проценты и прочие доходы.
//! Деньги компании (аванс, возмещение, расходы поездок), переводы и долги ни расходом,
//! ни доходом не считаются; поездки — отдельно, в «Командировках за год».
//! «Отложено за месяц» — переводы на накопительные счета и вклады минус взятое с них.
//!
//! Пока период идёт, он сравнивается с теми же днями прошлого периода (1–7 ноября
//! с 1–7 октября), а не с целым месяцем — иначе расходы «падают» в начале каждого месяца.

use crate::book::{trip_view, Account};
use crate::date::Date;
use crate::money::Kop;
use crate::savings::is_saving;
use crate::trip::{Op, OpKind, Trip, TripStatus};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Период статистики: месяц, квартал или год.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Span {
    Month,
    Quarter,
    Year,
}

impl Span {
    pub fn parse(s: &str) -> Span {
        match s {
            "quarter" => Span::Quarter,
            "year" => Span::Year,
            _ => Span::Month,
        }
    }

    fn months(self) -> i32 {
        match self {
            Span::Month => 1,
            Span::Quarter => 3,
            Span::Year => 12,
        }
    }
}

/// Первый и последний день периода, в который попадает `any`.
pub fn period(span: Span, any: Date) -> (Date, Date) {
    let (y, m, _) = any.ymd();
    let first_month = match span {
        Span::Month => m,
        Span::Quarter => (m - 1) / 3 * 3 + 1,
        Span::Year => 1,
    };
    let from = Date::new(y, first_month, 1).unwrap_or(any);
    (from, from.add_months(span.months()).add_days(-1))
}

/// Расход для статистики: личный или семейный.
pub fn is_spending(o: &Op) -> bool {
    o.kind == OpKind::Personal
}

/// Доход для статистики: зарплата, проценты, прочее (не деньги компании и не долги).
pub fn is_income(o: &Op) -> bool {
    o.kind == OpKind::Income
}

#[derive(Clone, Debug, Serialize)]
pub struct CatStat {
    pub cat: String,
    pub amount: Kop,
    /// Столько же за прошлый такой же период.
    pub prev: Kop,
}

#[derive(Clone, Debug, Serialize)]
pub struct MonthStat {
    /// «2026-11»
    pub month: String,
    pub income: Kop,
    pub spent: Kop,
    /// Отложено на накопительные счета и вклады (может быть меньше нуля — взяли).
    pub saved: Kop,
}

#[derive(Clone, Debug, Serialize)]
pub struct TripStat {
    pub id: String,
    pub city: String,
    pub start: Date,
    pub end: Date,
    pub days: i32,
    /// Суточные за поездку.
    pub per_diem: Kop,
    /// Осталось от суточных после питания и трат без документов.
    pub left: Kop,
    /// Долг перед компанией.
    pub debt: Kop,
    /// Расходы с документами (в отчёт).
    pub confirmed: Kop,
    /// Поездка завершена.
    pub finished: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TripsYear {
    pub year: i32,
    pub count: usize,
    pub days: i32,
    pub per_diem: Kop,
    pub left: Kop,
    pub debt: Kop,
    pub confirmed: Kop,
    pub items: Vec<TripStat>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub from: Date,
    pub to: Date,
    /// С чем сравнивается: прошлый такой же период, а пока текущий идёт — те же его дни.
    pub prev_from: Date,
    pub prev_to: Date,
    /// Период ещё идёт: сравнение — с теми же днями прошлого.
    pub partial: bool,
    /// Расходы за период и за то, с чем сравнивается.
    pub spent: Kop,
    pub prev_spent: Kop,
    pub income: Kop,
    /// По категориям, сначала крупные.
    pub cats: Vec<CatStat>,
    /// Двенадцать месяцев, последний — месяц конца периода (или текущий, если период идёт).
    pub months: Vec<MonthStat>,
    pub trips: TripsYear,
}

fn sum_by_cat(ops: &[Op], from: Date, to: Date) -> BTreeMap<String, Kop> {
    let mut out = BTreeMap::new();
    for o in ops.iter().filter(|o| is_spending(o) && o.date >= from && o.date <= to) {
        *out.entry(o.cat.clone()).or_default() += o.amount;
    }
    out
}

/// Сколько отложено операцией: на накопления — плюс, с накоплений — минус.
fn saved_by(o: &Op, saving: &BTreeSet<&str>) -> Kop {
    if o.kind != OpKind::Transfer {
        return 0;
    }
    let from = saving.contains(o.account.as_str());
    let to = o.to.as_deref().is_some_and(|t| saving.contains(t));
    match (from, to) {
        (false, true) => o.amount,
        (true, false) => -o.amount,
        _ => 0,
    }
}

pub fn stats(ops: &[Op], accounts: &[Account], trips: &[Trip], settings_rate: Kop, span: Span, any: Date, today: Date) -> Stats {
    let (from, to) = period(span, any);
    let prev_from = from.add_months(-span.months());
    let partial = today >= from && today < to;
    let prev_to = if partial { prev_from.add_days(from.days_until(today)).min(from.add_days(-1)) } else { from.add_days(-1) };
    let now = sum_by_cat(ops, from, to);
    let before = sum_by_cat(ops, prev_from, prev_to);
    let mut cats: Vec<CatStat> = now.iter().map(|(c, a)| CatStat { cat: c.clone(), amount: *a, prev: before.get(c).copied().unwrap_or(0) }).collect();
    for (c, p) in &before {
        if !now.contains_key(c) {
            cats.push(CatStat { cat: c.clone(), amount: 0, prev: *p });
        }
    }
    cats.sort_by(|a, b| b.amount.cmp(&a.amount).then(b.prev.cmp(&a.prev)).then_with(|| a.cat.cmp(&b.cat)));

    let saving: BTreeSet<&str> = accounts.iter().filter(|a| is_saving(&a.kind)).map(|a| a.id.as_str()).collect();
    // Будущие месяцы пустые — график кончается текущим.
    let end = to.min(today).max(from);
    let last = Date::new(end.ymd().0, end.ymd().1, 1).unwrap_or(end);
    let months = (0..12)
        .rev()
        .map(|k| {
            let m0 = last.add_months(-k);
            let m1 = m0.add_months(1).add_days(-1);
            let within = |o: &&Op| o.date >= m0 && o.date <= m1;
            let (y, m, _) = m0.ymd();
            MonthStat {
                month: format!("{y:04}-{m:02}"),
                income: ops.iter().filter(within).filter(|o| is_income(o)).map(|o| o.amount).sum(),
                spent: ops.iter().filter(within).filter(|o| is_spending(o)).map(|o| o.amount).sum(),
                saved: ops.iter().filter(within).map(|o| saved_by(o, &saving)).sum(),
            }
        })
        .collect();

    let year = to.ymd().0;
    let mut items: Vec<TripStat> = trips
        .iter()
        .filter(|t| t.start.ymd().0 == year)
        .map(|t| {
            let v = trip_view(t, ops, settings_rate, today);
            TripStat {
                id: t.id.clone(),
                city: t.city.clone(),
                start: t.start,
                end: t.end,
                days: v.s.days,
                per_diem: v.s.per_diem_full,
                left: v.s.leftover.max(0),
                debt: v.s.debt_due,
                confirmed: v.s.confirmed,
                finished: t.status >= TripStatus::AwaitingReport,
            }
        })
        .collect();
    items.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
    let trips = TripsYear {
        year,
        count: items.len(),
        days: items.iter().map(|t| t.days).sum(),
        per_diem: items.iter().map(|t| t.per_diem).sum(),
        left: items.iter().map(|t| t.left).sum(),
        debt: items.iter().map(|t| t.debt).sum(),
        confirmed: items.iter().map(|t| t.confirmed).sum(),
        items,
    };
    Stats {
        from,
        to,
        prev_from,
        prev_to,
        partial,
        spent: now.values().sum(),
        prev_spent: before.values().sum(),
        income: ops.iter().filter(|o| is_income(o) && o.date >= from && o.date <= to).map(|o| o.amount).sum(),
        cats,
        months,
        trips,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::default_accounts;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    #[test]
    fn periods() {
        assert_eq!(period(Span::Month, d("2026-11-07")), (d("2026-11-01"), d("2026-11-30")));
        assert_eq!(period(Span::Quarter, d("2026-11-07")), (d("2026-10-01"), d("2026-12-31")));
        assert_eq!(period(Span::Year, d("2026-11-07")), (d("2026-01-01"), d("2026-12-31")));
        assert_eq!(period(Span::Month, d("2028-02-10")).1, d("2028-02-29"));
    }

    #[test]
    fn spending_income_saved_and_trips() {
        let base = crate::demo::ops().remove(0);
        let op = |kind: OpKind, cat: &str, amount: Kop, date: &str| Op { kind, cat: cat.into(), amount, date: d(date), trip: None, family: false, ..base.clone() };
        let mut accounts = default_accounts();
        accounts.push(Account { id: "acc-save".into(), kind: "savings".into(), ..accounts[1].clone() });
        let mut to_save = op(OpKind::Transfer, "transfer", 10_000_00, "2026-11-03");
        to_save.account = "acc-personal".into();
        to_save.to = Some("acc-save".into());
        let mut from_save = op(OpKind::Transfer, "transfer", 2_000_00, "2026-11-20");
        from_save.account = "acc-save".into();
        from_save.to = Some("acc-cash".into());
        let ops = vec![
            op(OpKind::Personal, "groceries", 5_000_00, "2026-11-02"),
            Op { family: true, ..op(OpKind::Personal, "groceries", 3_000_00, "2026-11-12") },
            op(OpKind::Personal, "cafe", 1_000_00, "2026-11-15"),
            op(OpKind::Personal, "groceries", 4_000_00, "2026-10-10"),
            op(OpKind::Personal, "gifts", 2_500_00, "2026-10-20"),
            op(OpKind::Personal, "cafe", 700_00, "2026-10-03"),
            op(OpKind::Income, "salary", 80_000_00, "2026-11-05"),
            op(OpKind::Advance, "advance", 50_000_00, "2026-11-05"),
            op(OpKind::Work, "taxi", 700_00, "2026-11-06"),
            op(OpKind::Debt, "debt", 9_999_00, "2026-11-06"),
            to_save,
            from_save,
        ];
        // Ноябрь уже прошёл — сравнение с целым октябрём.
        let s = stats(&ops, &accounts, &crate::demo::trips(), crate::trip::DEFAULT_RATE, Span::Month, d("2026-11-07"), d("2026-12-05"));
        assert!(!s.partial);
        assert_eq!((s.prev_from, s.prev_to), (d("2026-10-01"), d("2026-10-31")));
        assert_eq!((s.spent, s.prev_spent, s.income), (9_000_00, 7_200_00, 80_000_00), "аванс, расходы поездки и долг не считаются");
        let c: Vec<(&str, Kop, Kop)> = s.cats.iter().map(|c| (c.cat.as_str(), c.amount, c.prev)).collect();
        assert_eq!(c, [("groceries", 8_000_00, 4_000_00), ("cafe", 1_000_00, 700_00), ("gifts", 0, 2_500_00)], "семейные — тоже расходы; прошлый период рядом");
        assert_eq!(s.months.len(), 12);
        let nov = s.months.last().unwrap();
        assert_eq!((nov.month.as_str(), nov.income, nov.spent, nov.saved), ("2026-11", 80_000_00, 9_000_00, 8_000_00), "отложено: 10 000 на копилку минус 2 000 взятых");
        assert_eq!(s.months[0].month, "2025-12");
        assert!(s.trips.count >= 3 && s.trips.year == 2026, "поездки 2026 года из демо: {}", s.trips.count);
        assert_eq!(s.trips.days, s.trips.items.iter().map(|t| t.days).sum::<i32>());

        // Ноябрь идёт (сегодня 7-е) — сравнение с 1–7 октября, а не с целым месяцем.
        let s = stats(&ops, &accounts, &[], crate::trip::DEFAULT_RATE, Span::Month, d("2026-11-07"), d("2026-11-07"));
        assert!(s.partial);
        assert_eq!((s.prev_from, s.prev_to, s.prev_spent), (d("2026-10-01"), d("2026-10-07"), 700_00));
        let c: Vec<(&str, Kop, Kop)> = s.cats.iter().map(|c| (c.cat.as_str(), c.amount, c.prev)).collect();
        assert_eq!(c, [("groceries", 8_000_00, 0), ("cafe", 1_000_00, 700_00)]);

        // Год идёт: те же дни прошлого года, график — по текущий месяц.
        let y = stats(&ops, &accounts, &[], crate::trip::DEFAULT_RATE, Span::Year, d("2026-11-07"), d("2026-11-07"));
        assert_eq!((y.from, y.to, y.prev_from, y.prev_to), (d("2026-01-01"), d("2026-12-31"), d("2025-01-01"), d("2025-11-07")));
        assert_eq!((y.months[0].month.as_str(), y.months[11].month.as_str()), ("2025-12", "2026-11"));
        // Прошлый год целиком — график ровно по его месяцам.
        let y = stats(&ops, &accounts, &[], crate::trip::DEFAULT_RATE, Span::Year, d("2025-06-01"), d("2026-11-07"));
        assert!(!y.partial);
        assert_eq!((y.months[0].month.as_str(), y.months[11].month.as_str()), ("2025-01", "2025-12"));

        // 30 марта: «те же дни» февраля кончаются его последним днём.
        let m = stats(&ops, &accounts, &[], crate::trip::DEFAULT_RATE, Span::Month, d("2027-03-30"), d("2027-03-30"));
        assert_eq!((m.prev_from, m.prev_to), (d("2027-02-01"), d("2027-02-28")));
        // Последний день месяца — сравнение уже с целым прошлым.
        assert!(!stats(&ops, &accounts, &[], crate::trip::DEFAULT_RATE, Span::Month, d("2027-03-31"), d("2027-03-31")).partial);
    }
}
