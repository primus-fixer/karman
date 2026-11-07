//! Накопления: строка «Отложено», вклады с прогнозом процентов и цели.
//!
//! Накопительные счета и вклады — отдельная строка «Отложено». В «Мои деньги» они входят,
//! только если так задано в настройках («savings.in_mine», по умолчанию нет).
//! На цель откладывают переводом на накопительный счёт с меткой цели; на одном счёте
//! может копиться несколько целей. Проценты по вкладу банк начисляет сам — их записывают
//! доходом, а «Карман» показывает прогноз.

use crate::book::AccountView;
use crate::date::Date;
use crate::money::Kop;
use crate::trip::{Op, OpKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const KIND_SAVINGS: &str = "savings";
pub const KIND_DEPOSIT: &str = "deposit";

/// Счёт для накоплений: накопительный или вклад.
pub fn is_saving(kind: &str) -> bool {
    kind == KIND_SAVINGS || kind == KIND_DEPOSIT
}

/// Отложенное считать в «Моих деньгах» (настройка, по умолчанию нет).
pub fn in_mine(settings: &BTreeMap<String, String>) -> bool {
    settings.get("savings.in_mine").is_some_and(|v| v == "1")
}

/// Условия вклада.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Deposit {
    /// Ставка, процентов годовых.
    pub rate: f64,
    /// Дата открытия.
    pub opened: Date,
    /// Дата окончания; нет — бессрочный (накопительный счёт со ставкой).
    #[serde(default)]
    pub until: Option<Date>,
    /// Капитализация: проценты каждый месяц прибавляются к вкладу.
    #[serde(default)]
    pub capitalize: bool,
}

/// Прогноз по вкладу на нынешнем остатке.
#[derive(Clone, Debug, Serialize)]
pub struct DepositView {
    pub account: String,
    /// Проценты за месяц по нынешнему остатку.
    pub month: Kop,
    /// Сколько процентов набежит с сегодняшнего дня до конца срока.
    pub income: Kop,
    /// Сколько будет на вкладе к концу срока.
    pub at_end: Kop,
    pub until: Option<Date>,
    /// Дней до конца срока.
    pub days_left: Option<i32>,
}

/// Проценты с сегодняшнего дня до конца срока: по дням (365 в году), помесячно;
/// с капитализацией начисленное каждый месяц прибавляется к остатку.
/// Возвращает (за месяц, до конца срока, к концу срока на вкладе).
pub fn forecast(balance: Kop, d: &Deposit, today: Date) -> (Kop, Kop, Kop) {
    let r = d.rate.max(0.0) / 100.0;
    let month = (balance.max(0) as f64 * r / 12.0).round() as Kop;
    let Some(end) = d.until.filter(|u| *u > today) else { return (month, 0, balance) };
    let mut bal = balance.max(0) as f64;
    let mut income = 0.0;
    let mut from = today;
    for k in 1..=1200 {
        let next = today.add_months(k).min(end);
        let interest = bal * r * from.days_until(next) as f64 / 365.0;
        income += interest;
        if d.capitalize {
            bal += interest;
        }
        if next >= end {
            break;
        }
        from = next;
    }
    (month, income.round() as Kop, balance + income.round() as Kop)
}

pub fn deposit_views(accounts: &[AccountView], today: Date) -> Vec<DepositView> {
    accounts
        .iter()
        .filter(|a| !a.account.archived && a.account.kind == KIND_DEPOSIT)
        .filter_map(|a| {
            let d = a.account.deposit.as_ref()?;
            let (month, income, at_end) = forecast(a.balance, d, today);
            Some(DepositView { account: a.account.id.clone(), month, income, at_end, until: d.until, days_left: d.until.map(|u| today.days_until(u)) })
        })
        .collect()
}

/// Цель: отпуск, ремонт, подушка безопасности.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub title: String,
    /// Сколько нужно, в рублях.
    pub target: Kop,
    /// К какой дате.
    #[serde(default)]
    pub due: Option<Date>,
    /// Накопительный счёт, куда откладываются деньги.
    pub account: String,
    /// Цель достигнута или закрыта.
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub note: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GoalView {
    pub goal: Goal,
    /// Сколько отложено.
    pub saved: Kop,
    /// Сколько осталось.
    pub left: Kop,
    /// Месяцев до срока (не меньше одного).
    pub months_left: Option<i32>,
    /// Сколько откладывать в месяц, чтобы успеть к сроку.
    pub per_month: Option<Kop>,
}

/// Отложено на цель: переводы с её меткой на её счёт минус взятое с него.
pub fn goal_saved(g: &Goal, ops: &[Op]) -> Kop {
    let sum: Kop = ops
        .iter()
        .filter(|o| o.kind == OpKind::Transfer && o.goal.as_deref() == Some(g.id.as_str()))
        .map(|o| {
            if o.to.as_deref() == Some(g.account.as_str()) {
                o.amount
            } else if o.account == g.account {
                -o.amount
            } else {
                0
            }
        })
        .sum();
    sum.max(0)
}

/// Сколько полных месяцев до даты, но не меньше одного.
fn months_until(today: Date, due: Date) -> i32 {
    let mut m = 0;
    while m < 1200 && today.add_months(m + 1) <= due {
        m += 1;
    }
    m.max(1)
}

pub fn goal_views(goals: &[Goal], ops: &[Op], today: Date) -> Vec<GoalView> {
    goals
        .iter()
        .map(|g| {
            let saved = goal_saved(g, ops);
            let left = (g.target - saved).max(0);
            let months_left = g.due.map(|d| months_until(today, d));
            // В месяц — с округлением вверх до рубля, чтобы точно успеть.
            let per_month = months_left.filter(|_| left > 0 && !g.done).map(|m| (left + m as Kop * 100 - 1) / (m as Kop * 100) * 100);
            GoalView { goal: g.clone(), saved, left, months_left, per_month }
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
    fn deposit_forecast_simple_and_capitalized() {
        // 100 000 ₽ под 12% на год: без капитализации ровно 12 000, с капитализацией больше.
        let mut dep = Deposit { rate: 12.0, opened: d("2026-11-07"), until: Some(d("2027-11-07")), capitalize: false };
        let (month, income, at_end) = forecast(100_000_00, &dep, d("2026-11-07"));
        assert_eq!(month, 1_000_00);
        assert_eq!(income, 12_000_00);
        assert_eq!(at_end, 112_000_00);
        dep.capitalize = true;
        let (_, income_c, _) = forecast(100_000_00, &dep, d("2026-11-07"));
        assert!(income_c > 12_600_00 && income_c < 12_700_00, "капитализация: {income_c}");
        // Срок прошёл или не задан — процентов к концу нет, только за месяц.
        dep.until = None;
        assert_eq!(forecast(100_000_00, &dep, d("2026-11-07")), (1_000_00, 0, 100_000_00));
    }

    #[test]
    fn goal_progress_from_tagged_transfers() {
        let g = Goal { id: "trip".into(), title: "Отпуск".into(), target: 120_000_00, due: Some(d("2027-05-07")), account: "acc-save".into(), done: false, note: String::new(), extra: Default::default() };
        let mut op = crate::demo::ops().remove(0);
        op.kind = OpKind::Transfer;
        op.goal = Some("trip".into());
        let put = |amount: Kop, from: &str, to: &str| Op { amount, account: from.into(), to: Some(to.into()), ..op.clone() };
        let ops = vec![put(30_000_00, "acc-personal", "acc-save"), put(20_000_00, "acc-personal", "acc-save"), put(5_000_00, "acc-save", "acc-personal"), Op { goal: None, ..put(99_000_00, "acc-personal", "acc-save") }];
        let v = &goal_views(&[g], &ops, d("2026-11-07"))[0];
        assert_eq!((v.saved, v.left), (45_000_00, 75_000_00), "взятое с цели вычитается, переводы без метки не считаются");
        assert_eq!((v.months_left, v.per_month), (Some(6), Some(12_500_00)));
    }
}
