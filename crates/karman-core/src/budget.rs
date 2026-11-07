//! Бюджет на месяц: личный и семейный, лимиты по категориям и общий.
//!
//! В личный бюджет идут личные расходы — в том числе с меткой поездки и оплаты по графику,
//! в семейный — расходы с меткой «Семья». Переводы, долги и расходы поездок (деньги компании)
//! в бюджет не входят. Месяц бюджета начинается с числа из настроек («budget.day»,
//! по умолчанию 1-е): например, с дня зарплаты.

use crate::date::Date;
use crate::money::Kop;
use crate::trip::{Op, OpKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PERSONAL: &str = "personal";
pub const FAMILY: &str = "family";

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Budget {
    /// personal — личный, family — семейный.
    pub id: String,
    /// Общий лимит на месяц; нет — не задан.
    #[serde(default)]
    pub total: Option<Kop>,
    /// Лимиты по категориям.
    #[serde(default)]
    pub cats: BTreeMap<String, Kop>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CatView {
    pub cat: String,
    pub limit: Option<Kop>,
    pub spent: Kop,
}

#[derive(Clone, Debug, Serialize)]
pub struct BudgetView {
    pub id: String,
    /// Первый и последний день месяца бюджета.
    pub start: Date,
    pub end: Date,
    pub total: Option<Kop>,
    pub spent: Kop,
    /// Категории с лимитом или тратами; сначала с лимитом.
    pub cats: Vec<CatView>,
    /// Дней до конца месяца бюджета, включая сегодня.
    pub days_left: i32,
}

/// Число месяца, с которого начинается бюджет (1–28).
pub fn settings_day(settings: &BTreeMap<String, String>) -> u32 {
    settings.get("budget.day").and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).clamp(1, 28)
}

/// `day`-е число месяца, в котором лежит `any`.
fn day_of(any: Date, day: u32) -> Date {
    let (y, m, _) = any.ymd();
    Date::new(y, m, day).unwrap_or(any)
}

/// Месяц бюджета, в который попадает `today`: с `day`-го числа до дня перед следующим `day`-м.
pub fn period(today: Date, day: u32) -> (Date, Date) {
    let this = day_of(today, day);
    let start = if this <= today { this } else { day_of(today.add_months(-1), day) };
    (start, day_of(start.add_months(1), day).add_days(-1))
}

/// Расход для бюджета: личный или семейный.
pub fn budget_of(o: &Op) -> Option<&'static str> {
    match o.kind {
        OpKind::Personal if o.family => Some(FAMILY),
        OpKind::Personal => Some(PERSONAL),
        _ => None,
    }
}

pub fn budget_views(budgets: &[Budget], ops: &[Op], today: Date, day: u32) -> Vec<BudgetView> {
    let (start, end) = period(today, day);
    [PERSONAL, FAMILY]
        .iter()
        .map(|id| {
            let b = budgets.iter().find(|b| b.id == *id);
            let mut spent_by: BTreeMap<String, Kop> = BTreeMap::new();
            for o in ops.iter().filter(|o| o.date >= start && o.date <= end && budget_of(o) == Some(id)) {
                *spent_by.entry(o.cat.clone()).or_default() += o.amount;
            }
            let mut cats: Vec<CatView> = b.map(|b| b.cats.iter().map(|(c, l)| CatView { cat: c.clone(), limit: Some(*l), spent: spent_by.get(c).copied().unwrap_or(0) }).collect()).unwrap_or_default();
            for (c, s) in &spent_by {
                if !cats.iter().any(|v| &v.cat == c) {
                    cats.push(CatView { cat: c.clone(), limit: None, spent: *s });
                }
            }
            BudgetView {
                id: id.to_string(),
                start,
                end,
                total: b.and_then(|b| b.total),
                spent: spent_by.values().sum(),
                cats,
                days_left: today.days_until(end) + 1,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    #[test]
    fn budget_month_from_any_day() {
        assert_eq!(period(d("2026-11-07"), 1), (d("2026-11-01"), d("2026-11-30")));
        assert_eq!(period(d("2026-11-07"), 10), (d("2026-10-10"), d("2026-11-09")), "до 10-го — ещё прошлый месяц бюджета");
        assert_eq!(period(d("2026-11-10"), 10), (d("2026-11-10"), d("2026-12-09")));
        assert_eq!(period(d("2027-01-05"), 25), (d("2026-12-25"), d("2027-01-24")), "через Новый год");
    }

    #[test]
    fn personal_and_family_budgets_apart() {
        let base = crate::demo::ops().remove(0);
        let op = |kind: OpKind, cat: &str, amount: Kop, date: &str, family: bool| Op { kind, cat: cat.into(), amount, date: d(date), family, trip: None, ..base.clone() };
        let ops = vec![
            op(OpKind::Personal, "groceries", 2_000_00, "2026-11-03", false),
            op(OpKind::Personal, "groceries", 3_000_00, "2026-11-05", true),
            op(OpKind::Personal, "cafe", 700_00, "2026-11-06", false),
            op(OpKind::Personal, "groceries", 9_999_00, "2026-10-30", false),
            op(OpKind::Work, "taxi", 500_00, "2026-11-06", false),
            op(OpKind::Transfer, "transfer", 5_000_00, "2026-11-06", false),
        ];
        let budgets = vec![Budget { id: PERSONAL.into(), total: Some(30_000_00), cats: BTreeMap::from([("groceries".to_string(), 15_000_00)]), extra: Default::default() }];
        let v = budget_views(&budgets, &ops, d("2026-11-07"), 1);
        let (p, f) = (&v[0], &v[1]);
        assert_eq!((p.id.as_str(), p.spent, p.total), (PERSONAL, 2_700_00, Some(30_000_00)), "поездка, перевод и прошлый месяц не считаются");
        assert_eq!((p.cats[0].cat.as_str(), p.cats[0].limit, p.cats[0].spent), ("groceries", Some(15_000_00), 2_000_00));
        assert_eq!((p.cats[1].cat.as_str(), p.cats[1].limit, p.cats[1].spent), ("cafe", None, 700_00));
        assert_eq!((f.id.as_str(), f.spent, f.total), (FAMILY, 3_000_00, None), "семейные — отдельно");
        assert_eq!(p.days_left, 24);
    }
}
