//! Сводка месяца без ИИ: траты против обычного, подписки и регулярные траты, крупные покупки
//! и прогноз «Моих денег» на конец месяца. Всё считает «Карман»; Claude только пересказывает.
//!
//! Траты — личные и семейные (как в статистике): деньги компании, переводы и долги не в счёт.

use crate::book::Snapshot;
use crate::date::Date;
use crate::category;
use crate::money::Kop;
use crate::trip::{Op, OpKind};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Траты категории в этом месяце против обычного (среднее за три прошлых месяца).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CatTrend {
    pub cat: String,
    pub name: String,
    pub spent: Kop,
    pub usual: Kop,
}

/// Повторяющаяся трата: похоже на подписку или регулярный платёж.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Recurring {
    pub title: String,
    pub cat: String,
    /// Обычная сумма.
    pub amount: Kop,
    /// В скольких месяцах из последних шести встречалась.
    pub months: u32,
    pub last: Date,
    /// Когда ждать следующую.
    pub next: Date,
    /// Уже заведена как регулярный платёж.
    pub in_plans: bool,
}

/// Крупная разовая трата: втрое больше обычной покупки в этой категории.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BigSpend {
    pub id: String,
    pub date: Date,
    pub title: String,
    pub cat: String,
    pub amount: Kop,
}

/// Прогноз «Моих денег» на последний день месяца.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Forecast {
    pub now: Kop,
    /// Ожидаемые доходы до конца месяца: те, что приходят каждый месяц и ещё не пришли.
    pub income: Kop,
    /// Регулярные платежи до конца месяца по графику.
    pub planned: Kop,
    /// Обычные траты в день (за 90 дней, без регулярных платежей и крупных покупок) × оставшиеся дни.
    pub spending: Kop,
    pub per_day: Kop,
    pub days_left: u32,
    pub end: Kop,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Insights {
    /// «2026-11»
    pub month: String,
    /// Дней месяца прошло (с сегодняшним).
    pub days: u32,
    pub spent: Kop,
    pub income: Kop,
    /// Обычные траты за такое же число дней — среднее трёх прошлых месяцев.
    pub usual_to_date: Kop,
    pub cats: Vec<CatTrend>,
    /// Категории, где потрачено заметно больше обычного.
    pub unusual: Vec<CatTrend>,
    pub big: Vec<BigSpend>,
    pub recurring: Vec<Recurring>,
    pub forecast: Forecast,
}

fn spending(o: &Op) -> bool {
    o.kind == OpKind::Personal && o.debt.is_none() && o.goal.is_none()
}

fn month_key(d: Date) -> (i32, u32) {
    let (y, m, _) = d.ymd();
    (y, m)
}

fn median(v: &mut [Kop]) -> Kop {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2
    }
}

/// Название для сравнения: без регистра, цифр и знаков — «Яндекс Плюс 11.2026» и «яндекс плюс» одно и то же.
fn norm_title(t: &str) -> String {
    t.to_lowercase().replace('ё', "е").chars().map(|c| if c.is_alphabetic() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

const RUB: Kop = 100;

/// Регулярный доход: в каких месяцах (год, месяц) приходил и суммы.
type Seen = (BTreeSet<(i32, u32)>, Vec<Kop>);

pub fn insights(s: &Snapshot) -> Insights {
    let today = s.today;
    let ms = today.month_start();
    let me = today.month_end();
    let days = (ms.days_until(today) + 1) as u32;
    let this: Vec<&Op> = s.ops.iter().filter(|o| o.date >= ms && o.date <= today).collect();
    let spent: Kop = this.iter().filter(|o| spending(o)).map(|o| o.amount).sum();
    let income: Kop = this.iter().filter(|o| o.kind == OpKind::Income).map(|o| o.amount).sum();

    // Три прошлых месяца: по категориям и «за столько же дней».
    let prev: Vec<(Date, Date)> = (1..=3).map(|k| (ms.add_months(-k), ms.add_months(-k).month_end())).collect();
    let mut usual_to_date: Kop = 0;
    let mut by_cat_prev: BTreeMap<String, Kop> = BTreeMap::new();
    let mut per_cat_ops: BTreeMap<String, Vec<Kop>> = BTreeMap::new();
    for (a, b) in &prev {
        let cut = a.add_days(days as i32 - 1).min(*b);
        for o in s.ops.iter().filter(|o| spending(o) && o.date >= *a && o.date <= *b) {
            *by_cat_prev.entry(o.cat.clone()).or_default() += o.amount;
            per_cat_ops.entry(o.cat.clone()).or_default().push(o.amount);
            if o.date <= cut {
                usual_to_date += o.amount;
            }
        }
    }
    usual_to_date /= 3;
    let mut by_cat: BTreeMap<String, Kop> = BTreeMap::new();
    for o in this.iter().filter(|o| spending(o)) {
        *by_cat.entry(o.cat.clone()).or_default() += o.amount;
    }
    let cats_all: BTreeSet<String> = by_cat.keys().chain(by_cat_prev.keys()).cloned().collect();
    let mut cats: Vec<CatTrend> = cats_all
        .into_iter()
        .map(|c| CatTrend { name: category::name(&c, &s.categories), spent: by_cat.get(&c).copied().unwrap_or(0), usual: by_cat_prev.get(&c).copied().unwrap_or(0) / 3, cat: c })
        .filter(|c| c.spent > 0 || c.usual > 0)
        .collect();
    cats.sort_by(|a, b| b.spent.cmp(&a.spent).then_with(|| b.usual.cmp(&a.usual)));
    // Заметно больше обычного: в полтора раза и на 3 000 ₽; новая категория — от 5 000 ₽.
    let unusual: Vec<CatTrend> = cats.iter().filter(|c| (c.usual > 0 && c.spent * 2 > c.usual * 3 && c.spent - c.usual >= 3_000 * RUB) || (c.usual == 0 && c.spent >= 5_000 * RUB)).cloned().collect();

    // Крупные разовые: от 5 000 ₽ и втрое больше обычной покупки в категории.
    let mut big: Vec<BigSpend> = this
        .iter()
        .filter(|o| spending(o) && o.plan.is_none() && o.amount >= 5_000 * RUB)
        .filter(|o| {
            let mut v = per_cat_ops.get(&o.cat).cloned().unwrap_or_default();
            let m = median(&mut v);
            m == 0 || o.amount >= m * 3
        })
        .map(|o| BigSpend { id: o.id.clone(), date: o.date, title: o.title.clone(), cat: o.cat.clone(), amount: o.amount })
        .collect();
    big.sort_by_key(|b| std::cmp::Reverse(b.amount));
    big.truncate(5);

    // Повторяющиеся траты за полгода: одно название, похожая сумма, в трёх месяцах и больше.
    let since = ms.add_months(-5);
    let mut groups: BTreeMap<String, Vec<&Op>> = BTreeMap::new();
    for o in s.ops.iter().filter(|o| spending(o) && o.date >= since && o.date <= today) {
        let k = norm_title(&o.title);
        if !k.is_empty() {
            groups.entry(k).or_default().push(o);
        }
    }
    let plan_titles: BTreeSet<String> = s.plans.iter().map(|p| norm_title(&p.plan.title)).collect();
    let mut recurring: Vec<Recurring> = Vec::new();
    for (k, mut list) in groups {
        list.sort_by_key(|o| o.date);
        let mut amounts: Vec<Kop> = list.iter().map(|o| o.amount).collect();
        let typical = median(&mut amounts);
        let similar: Vec<&&Op> = list.iter().filter(|o| typical > 0 && (o.amount - typical).abs() * 10 <= typical).collect();
        let months: BTreeSet<(i32, u32)> = similar.iter().map(|o| month_key(o.date)).collect();
        // Раз в месяц: не больше двух раз в одном месяце.
        if months.len() < 3 || similar.len() > months.len() * 2 {
            continue;
        }
        let last = similar.last().expect("есть").date;
        let in_plans = plan_titles.contains(&k) || similar.iter().any(|o| o.plan.is_some());
        recurring.push(Recurring { title: similar.last().expect("есть").title.clone(), cat: similar[0].cat.clone(), amount: typical, months: months.len() as u32, last, next: last.add_months(1), in_plans });
    }
    recurring.sort_by(|a, b| a.in_plans.cmp(&b.in_plans).then_with(|| b.amount.cmp(&a.amount)));
    recurring.truncate(10);

    // Прогноз на конец месяца.
    let days_left = today.days_until(me).max(0) as u32;
    let planned: Kop = s.upcoming.iter().filter(|d| d.kind == "plan" && d.date > today && d.date <= me).map(|d| d.amount).sum();
    // Доходы, что приходят каждый месяц (в двух из трёх прошлых) и в этом ещё не пришли.
    let mut inc: BTreeMap<String, Seen> = BTreeMap::new();
    for o in s.ops.iter().filter(|o| o.kind == OpKind::Income && o.date >= prev[2].0 && o.date <= prev[0].1) {
        let e = inc.entry(norm_title(&o.title)).or_default();
        e.0.insert(month_key(o.date));
        e.1.push(o.amount);
    }
    let got: BTreeSet<String> = this.iter().filter(|o| o.kind == OpKind::Income).map(|o| norm_title(&o.title)).collect();
    let income_left: Kop = inc.into_iter().filter(|(k, (m, _))| m.len() >= 2 && !got.contains(k)).map(|(_, (_, mut v))| median(&mut v)).sum();
    let from90 = today.add_days(-90);
    let big_ids: BTreeSet<&str> = big.iter().map(|b| b.id.as_str()).collect();
    let usual90: Kop = s.ops.iter().filter(|o| spending(o) && o.plan.is_none() && o.date > from90 && o.date <= today && !big_ids.contains(o.id.as_str()) && o.amount < 20_000 * RUB).map(|o| o.amount).sum();
    let per_day = usual90 / 90;
    let spend_left = per_day * days_left as Kop;
    let forecast = Forecast { now: s.mine, income: income_left, planned, spending: spend_left, per_day, days_left, end: s.mine + income_left - planned - spend_left };

    let (y, m, _) = today.ymd();
    Insights { month: format!("{y}-{m:02}"), days, spent, income, usual_to_date, cats, unusual, big, recurring, forecast }
}

fn rub(k: Kop) -> serde_json::Value {
    serde_json::json!((k as f64) / 100.0)
}

/// Цифры месяца для Claude — в рублях, с понятными названиями. Бюджеты — лимиты и траты.
pub fn facts(i: &Insights, s: &Snapshot) -> serde_json::Value {
    use serde_json::json;
    json!({
        "month": i.month,
        "days_passed": i.days,
        "spent_rub": rub(i.spent),
        "usual_to_this_day_rub": rub(i.usual_to_date),
        "income_rub": rub(i.income),
        "by_category": i.cats.iter().take(12).map(|c| json!({ "category": c.name, "spent": rub(c.spent), "usual_per_month": rub(c.usual) })).collect::<Vec<_>>(),
        "more_than_usual": i.unusual.iter().map(|c| json!({ "category": c.name, "spent": rub(c.spent), "usual_per_month": rub(c.usual) })).collect::<Vec<_>>(),
        "big_purchases": i.big.iter().map(|b| json!({ "date": b.date, "title": b.title, "category": category::name(&b.cat, &s.categories), "amount": rub(b.amount) })).collect::<Vec<_>>(),
        "recurring": i.recurring.iter().map(|r| json!({ "title": r.title, "category": category::name(&r.cat, &s.categories), "amount": rub(r.amount), "months_of_last_6": r.months, "next_expected": r.next, "is_regular_payment_in_karman": r.in_plans })).collect::<Vec<_>>(),
        "forecast_end_of_month": {
            "my_money_now": rub(i.forecast.now),
            "expected_income": rub(i.forecast.income),
            "regular_payments_left": rub(i.forecast.planned),
            "usual_spending_left": rub(i.forecast.spending),
            "usual_spending_per_day": rub(i.forecast.per_day),
            "days_left": i.forecast.days_left,
            "my_money_at_month_end": rub(i.forecast.end),
        },
        "budgets": s.budgets.iter().map(|b| json!({ "budget": b.id, "limit": b.total.map(rub), "spent": rub(b.spent), "days_left": b.days_left })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book;
    use crate::db::Store;
    use crate::sync::EventBody;
    use crate::trip::Op;

    fn op(id: &str, date: &str, kind: OpKind, cat: &str, title: &str, rub: i64) -> Op {
        let mut o: Op = serde_json::from_value(serde_json::json!({
            "id": id, "date": date, "kind": "personal", "cat": cat, "title": title, "amount": rub * 100,
            "account": "acc-personal", "trip": null, "doc": null, "sent": false
        }))
        .unwrap();
        o.kind = kind;
        o
    }

    #[test]
    fn subscriptions_unusual_big_and_forecast() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = Store::open(&dir.path().join("t.db"), &[7u8; 32], "dev").unwrap();
        book::ensure_defaults(&mut st).unwrap();
        let mut ops = Vec::new();
        // Полгода: подписка 399 ₽ каждый месяц, зарплата 10-го, продукты по 2 000 ₽ раз в неделю.
        for (i, m) in ["2026-06", "2026-07", "2026-08", "2026-09", "2026-10"].iter().enumerate() {
            ops.push(op(&format!("s{i}"), &format!("{m}-05"), OpKind::Personal, "telecom", "Яндекс Плюс", 399));
            ops.push(op(&format!("z{i}"), &format!("{m}-10"), OpKind::Income, "salary", "Зарплата", 95_000));
            for w in 0..4 {
                ops.push(op(&format!("g{i}{w}"), &format!("{m}-{:02}", 3 + w * 7), OpKind::Personal, "groceries", "Продукты", 2_000));
            }
            ops.push(op(&format!("c{i}"), &format!("{m}-08"), OpKind::Personal, "cafe", "Кафе", 1_500));
        }
        // Ноябрь (сегодня — 20-е): подписка, продукты, кафе втрое больше обычного, крупная покупка.
        ops.push(op("sn", "2026-11-05", OpKind::Personal, "telecom", "Яндекс Плюс 11.2026", 399));
        ops.push(op("gn1", "2026-11-03", OpKind::Personal, "groceries", "Продукты", 2_000));
        ops.push(op("gn2", "2026-11-10", OpKind::Personal, "groceries", "Продукты", 2_000));
        ops.push(op("cn1", "2026-11-08", OpKind::Personal, "cafe", "Кафе", 3_000));
        ops.push(op("cn2", "2026-11-15", OpKind::Personal, "cafe", "Ресторан, день рождения", 4_500));
        ops.push(op("tv", "2026-11-12", OpKind::Personal, "home", "Телевизор", 48_000));
        st.append_all(ops.into_iter().map(|op| EventBody::OpPut { op }).collect()).unwrap();
        let today = Date::parse("2026-11-20").unwrap();
        let s = book::snapshot(&st, today).unwrap();
        let i = insights(&s);
        assert_eq!((i.month.as_str(), i.days), ("2026-11", 20));
        assert_eq!(i.spent, (399 + 4_000 + 7_500 + 48_000) * RUB);
        let cafe = i.cats.iter().find(|c| c.cat == "cafe").unwrap();
        assert_eq!((cafe.spent, cafe.usual), (7_500 * RUB, 1_500 * RUB));
        assert!(i.unusual.iter().any(|c| c.cat == "cafe") && i.unusual.iter().any(|c| c.cat == "home"), "{:?}", i.unusual);
        assert!(!i.unusual.iter().any(|c| c.cat == "groceries"), "продукты — как обычно");
        assert_eq!(i.big.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(), ["tv"], "крупная покупка — только телевизор");
        let sub = i.recurring.iter().find(|r| r.cat == "telecom").expect("подписка найдена");
        assert_eq!((sub.amount, sub.months, sub.in_plans, sub.next), (399 * RUB, 6, false, Date::parse("2026-12-05").unwrap()));
        assert!(!i.recurring.iter().any(|r| r.cat == "groceries"), "продукты по 4 раза в месяц — не подписка");
        // Зарплата в ноябре ещё не пришла — ждём её в прогнозе.
        assert_eq!(i.forecast.income, 95_000 * RUB);
        assert_eq!(i.forecast.days_left, 10);
        assert!(i.forecast.per_day > 0 && i.forecast.end == i.forecast.now + i.forecast.income - i.forecast.planned - i.forecast.spending);
        assert_eq!(norm_title("Яндекс Плюс 11.2026"), "яндекс плюс");
        let f = facts(&i, &s);
        assert_eq!(f["spent_rub"], serde_json::json!(59899.0));
        assert_eq!(f["recurring"][0]["is_regular_payment_in_karman"], false);
        assert_eq!(f["forecast_end_of_month"]["expected_income"], serde_json::json!(95000.0));
    }
}
