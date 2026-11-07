//! Долги, кредиты и регулярные платежи: график, остатки, ближайшие сроки и напоминания.
//!
//! Сам долг «Мои деньги» не уменьшает: деньги двигают только записанные операции
//! (получил в долг, вернул, платёж по кредиту). Платёж по графику записывается кнопкой
//! «Оплачено», автосписание — само в свой день. Номер такой операции постоянный
//! (`plan-<платёж>-<дата>`), поэтому два устройства не запишут один платёж дважды.

use crate::book::{fx_now, Account};
use crate::date::Date;
use crate::db::Store;
use crate::error::Result;
use crate::money::Kop;
use crate::trip::{Op, OpKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Кто кому должен.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebtDir {
    /// Я должен: занял, кредит, рассрочка.
    IOwe,
    /// Мне должны.
    OwedToMe,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Debt {
    pub id: String,
    /// Кто: человек, банк, магазин.
    pub who: String,
    pub dir: DebtDir,
    /// Сколько нужно вернуть, в рублях. Для кредита — сколько осталось выплатить на день записи.
    pub amount: Kop,
    pub date: Date,
    /// Срок возврата.
    #[serde(default)]
    pub due: Option<Date>,
    #[serde(default)]
    pub note: String,
    /// Закрыт вручную: простили, списали.
    #[serde(default)]
    pub closed: bool,
    /// Поля из более новых версий.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Как часто повторяется платёж.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Every {
    Month,
    Week,
    Year,
}

/// Регулярный платёж: связь, аренда, подписка, платёж по кредиту.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub title: String,
    /// Сумма платежа в валюте счёта.
    pub amount: Kop,
    pub account: String,
    /// Категория личного расхода (у платежа по кредиту не нужна).
    #[serde(default)]
    pub cat: String,
    pub every: Every,
    /// Первый платёж: число месяца (день недели) берётся из этой даты.
    pub start: Date,
    /// Последний платёж (например, конец кредита).
    #[serde(default)]
    pub until: Option<Date>,
    /// Автосписание: операция записывается сама в день платежа.
    #[serde(default)]
    pub auto: bool,
    /// За сколько дней напоминать; нет — как в настройках.
    #[serde(default)]
    pub remind: Option<i32>,
    /// Кредит или рассрочка: платёж уменьшает этот долг.
    #[serde(default)]
    pub debt: Option<String>,
    /// Пропущенные платежи.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Date>,
    /// Приостановлен: не напоминает и не списывается.
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub note: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Больше платежей одного графика не перебираем (недельный — почти 20 лет).
const MAX_STEPS: i32 = 1000;
/// На сколько дней вперёд показывать ближайшие платежи.
pub const AHEAD_DAYS: i32 = 31;
/// Напоминание по умолчанию: за 3 дня и в день платежа.
pub const DEFAULT_REMIND: i32 = 3;

impl Plan {
    /// Дата платежа номер `n` (с нуля). 31-е число в коротком месяце — последний день.
    pub fn occurrence(&self, n: i32) -> Date {
        match self.every {
            Every::Month => self.start.add_months(n),
            Every::Week => self.start.add_days(7 * n),
            Every::Year => self.start.add_months(12 * n),
        }
    }

    /// Даты платежей от начала графика до `to` включительно.
    pub fn dates_until(&self, to: Date) -> Vec<Date> {
        let last = self.until.map_or(to, |u| u.min(to));
        (0..MAX_STEPS).map(|n| self.occurrence(n)).take_while(|d| *d <= last).collect()
    }

    /// За сколько дней напоминать об этом платеже.
    pub fn lead(&self, settings_lead: i32) -> i32 {
        self.remind.unwrap_or(settings_lead).clamp(0, 30)
    }
}

/// Номер операции, которой оплачен платёж за эту дату.
pub fn pay_id(plan: &str, date: Date) -> String {
    format!("plan-{plan}-{date}")
}

/// Настройка «за сколько дней напоминать».
pub fn settings_lead(settings: &BTreeMap<String, String>) -> i32 {
    settings.get("remind.days").and_then(|v| v.parse::<i32>().ok()).unwrap_or(DEFAULT_REMIND).clamp(0, 30)
}

#[derive(Clone, Debug, Serialize)]
pub struct DebtView {
    pub debt: Debt,
    /// Сколько уже вернули.
    pub repaid: Kop,
    /// Сколько осталось.
    pub left: Kop,
    /// Срок прошёл, а долг не закрыт.
    pub overdue: bool,
    /// Кредит: регулярный платёж по нему.
    pub plan: Option<String>,
    /// Ближайший платёж по графику.
    pub next: Option<Date>,
}

impl DebtView {
    /// Долг ещё не возвращён и не закрыт.
    pub fn open(&self) -> bool {
        !self.debt.closed && self.left > 0
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanView {
    pub plan: Plan,
    /// Ближайший неоплаченный платёж; если он раньше сегодняшнего дня — просрочен.
    pub next: Option<Date>,
    /// Сколько платежей просрочено.
    pub overdue: usize,
    /// Сколько уже оплачено.
    pub paid: usize,
    /// Сумма платежа в рублях.
    pub amount_rub: Kop,
}

/// Ближайший срок: платёж по графику или возврат долга.
#[derive(Clone, Debug, Serialize)]
pub struct Due {
    /// plan — регулярный платёж, debt — срок долга.
    pub kind: &'static str,
    pub id: String,
    pub title: String,
    pub date: Date,
    /// В рублях: платёж или остаток долга.
    pub amount: Kop,
    /// Сколько дней до срока; меньше нуля — просрочено.
    pub days: i32,
    /// За сколько дней напоминать.
    pub lead: i32,
    /// Пора напоминать: срок близко или прошёл.
    pub soon: bool,
    /// Автосписание: запишется само.
    pub auto: bool,
    /// Для долга: я должен; для платежа — всегда да.
    pub i_owe: bool,
}

/// Повторы платежей, которые оплачены операциями.
fn paid_dates(ops: &[Op]) -> BTreeMap<&str, BTreeSet<Date>> {
    let mut out: BTreeMap<&str, BTreeSet<Date>> = BTreeMap::new();
    for o in ops {
        if let (Some(p), Some(d)) = (o.plan.as_deref(), o.plan_date) {
            out.entry(p).or_default().insert(d);
        }
    }
    out
}

/// Возврат долга: для «я должен» — деньги ушли, для «мне должны» — пришли.
fn repays(o: &Op, d: &Debt) -> bool {
    o.kind == OpKind::Debt && o.debt.as_deref() == Some(d.id.as_str()) && (o.inflow == (d.dir == DebtDir::OwedToMe))
}

pub fn plan_views(plans: &[Plan], ops: &[Op], accounts: &[Account], fx: &BTreeMap<String, f64>, today: Date) -> Vec<PlanView> {
    let paid = paid_dates(ops);
    let empty = BTreeSet::new();
    plans
        .iter()
        .map(|p| {
            let done = paid.get(p.id.as_str()).unwrap_or(&empty);
            let skipped: BTreeSet<Date> = p.skipped.iter().copied().collect();
            let open = |d: &Date| !done.contains(d) && !skipped.contains(d);
            let next = (0..MAX_STEPS).map(|n| p.occurrence(n)).take_while(|d| p.until.is_none_or(|u| *d <= u)).find(open);
            let overdue = if p.paused { 0 } else { p.dates_until(today.add_days(-1)).iter().filter(|d| open(d)).count() };
            let rate = currency_of(accounts, &p.account).map_or(1.0, |c| if c == "RUB" { 1.0 } else { fx.get(c).copied().unwrap_or(0.0) });
            PlanView { plan: p.clone(), next: if p.paused { None } else { next }, overdue, paid: done.len(), amount_rub: (p.amount as f64 * rate).round() as Kop }
        })
        .collect()
}

pub fn debt_views(debts: &[Debt], plans: &[PlanView], ops: &[Op], today: Date) -> Vec<DebtView> {
    debts
        .iter()
        .map(|d| {
            let repaid: Kop = ops.iter().filter(|o| repays(o, d)).map(|o| o.amount).sum();
            let left = (d.amount - repaid).max(0);
            let plan = plans.iter().find(|p| p.plan.debt.as_deref() == Some(d.id.as_str()));
            DebtView {
                debt: d.clone(),
                repaid,
                left,
                overdue: !d.closed && left > 0 && d.due.is_some_and(|due| due < today),
                plan: plan.map(|p| p.plan.id.clone()),
                next: plan.and_then(|p| p.next),
            }
        })
        .collect()
}

fn currency_of<'a>(accounts: &'a [Account], id: &str) -> Option<&'a str> {
    accounts.iter().find(|a| a.id == id).map(|a| a.currency.as_str())
}

/// Платежи и сроки долгов на ближайший месяц и всё просроченное, по дате.
pub fn upcoming(plans: &[PlanView], debts: &[DebtView], ops: &[Op], today: Date, settings_lead: i32) -> Vec<Due> {
    let paid = paid_dates(ops);
    let empty = BTreeSet::new();
    let horizon = today.add_days(AHEAD_DAYS);
    let mut out = Vec::new();
    for v in plans.iter().filter(|v| !v.plan.paused) {
        let p = &v.plan;
        let done = paid.get(p.id.as_str()).unwrap_or(&empty);
        let lead = p.lead(settings_lead);
        // Кредит закрыт — по графику больше не напоминаем.
        if let Some(dv) = p.debt.as_ref().and_then(|id| debts.iter().find(|d| &d.debt.id == id)) {
            if !dv.open() {
                continue;
            }
        }
        for d in p.dates_until(horizon) {
            if done.contains(&d) || p.skipped.contains(&d) {
                continue;
            }
            let days = today.days_until(d);
            out.push(Due { kind: "plan", id: p.id.clone(), title: p.title.clone(), date: d, amount: v.amount_rub, days, lead, soon: days <= lead, auto: p.auto, i_owe: true });
        }
    }
    for v in debts.iter().filter(|v| v.open()) {
        if let Some(due) = v.debt.due.filter(|d| *d <= horizon) {
            let days = today.days_until(due);
            out.push(Due {
                kind: "debt",
                id: v.debt.id.clone(),
                title: v.debt.who.clone(),
                date: due,
                amount: v.left,
                days,
                lead: settings_lead,
                soon: days <= settings_lead,
                auto: false,
                i_owe: v.debt.dir == DebtDir::IOwe,
            });
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.title.cmp(&b.title)));
    out
}

/// Конец срока вкладов в ближайший месяц: вклад закрыть или продлить.
pub fn deposit_ends(accounts: &[crate::book::AccountView], deposits: &[crate::savings::DepositView], today: Date, settings_lead: i32) -> Vec<Due> {
    let horizon = today.add_days(AHEAD_DAYS);
    deposits
        .iter()
        .filter_map(|v| {
            let end = v.until.filter(|u| *u >= today && *u <= horizon)?;
            let a = accounts.iter().find(|a| a.account.id == v.account)?;
            let days = today.days_until(end);
            Some(Due { kind: "deposit", id: v.account.clone(), title: a.account.name.clone(), date: end, amount: v.at_end, days, lead: settings_lead, soon: days <= settings_lead, auto: false, i_owe: false })
        })
        .collect()
}

/// Напоминание для уведомления: в какой день и что написать. Сумм нет —
/// уведомление видно на заблокированном экране, суммы — только в приложении после входа.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reminder {
    /// Постоянный ключ: по нему уведомление не повторяется и снимается, когда платёж оплачен.
    pub key: String,
    pub day: Date,
    pub text: String,
}

fn days_word(n: i32) -> &'static str {
    let (m, l) = (n % 100, n % 10);
    if (11..=14).contains(&m) {
        "дней"
    } else if l == 1 {
        "день"
    } else if (2..=4).contains(&l) {
        "дня"
    } else {
        "дней"
    }
}

fn when(n: i32) -> String {
    match n {
        0 => "Сегодня".into(),
        1 => "Завтра".into(),
        n => format!("Через {n} {}", days_word(n)),
    }
}

/// Напоминания по ближайшим срокам: за `lead` дней и в сам день. О просроченном —
/// только в приложении, без уведомлений.
pub fn reminders(due: &[Due], today: Date) -> Vec<Reminder> {
    let mut out = Vec::new();
    for d in due.iter().filter(|d| d.date >= today) {
        let mut days = vec![0];
        if d.lead > 0 {
            days.push(d.lead);
        }
        for ahead in days {
            let day = d.date.add_days(-ahead);
            if day < today {
                continue;
            }
            let text = match (d.kind, d.auto, d.i_owe) {
                ("deposit", _, _) => format!("{} заканчивается вклад: {}", when(ahead), d.title),
                ("plan", true, _) => format!("{} автосписание: {}", when(ahead), d.title),
                ("plan", false, _) => format!("{} платёж: {}", when(ahead), d.title),
                (_, _, true) => format!("{} срок: вернуть долг — {}", when(ahead), d.title),
                (_, _, false) => format!("{} срок: {} возвращает долг", when(ahead), d.title),
            };
            out.push(Reminder { key: format!("{}:{}:{}:{}", d.kind, d.id, d.date, ahead), day, text });
        }
    }
    out
}

/// Операция для оплаты платежа за `date`. Платёж по кредиту — возврат долга.
pub fn plan_op(plan: &Plan, date: Date, accounts: &[Account], fx: &BTreeMap<String, f64>, debts: &[Debt]) -> Op {
    let cur = currency_of(accounts, &plan.account).unwrap_or("RUB").to_string();
    let foreign = cur != "RUB";
    let rate = if foreign { fx.get(&cur).copied().unwrap_or(0.0) } else { 1.0 };
    let debt = plan.debt.as_ref().and_then(|id| debts.iter().find(|d| &d.id == id));
    let (kind, cat, inflow) = match debt {
        Some(d) => (OpKind::Debt, "debt".to_string(), d.dir == DebtDir::OwedToMe),
        None => (OpKind::Personal, if plan.cat.is_empty() { "other".to_string() } else { plan.cat.clone() }, false),
    };
    Op {
        id: pay_id(&plan.id, date),
        date,
        kind,
        cat,
        title: plan.title.clone(),
        amount: if foreign { (plan.amount as f64 * rate).round() as Kop } else { plan.amount },
        account: plan.account.clone(),
        trip: None,
        doc: None,
        sent: false,
        family: false,
        debt: debt.map(|d| d.id.clone()),
        inflow,
        orig: foreign.then_some(plan.amount),
        cur: foreign.then_some(cur),
        note: String::new(),
        att: Vec::new(),
        to: None,
        to_amount: None,
        plan: Some(plan.id.clone()),
        plan_date: Some(date),
        goal: None,
        extra: Default::default(),
    }
}

/// Автосписания: записывает платежи с галочкой «списывается само», чей день настал.
/// Удалённую вручную операцию не возвращает — удаление значит «списания не было».
pub fn auto_pay(store: &mut Store, today: Date) -> Result<usize> {
    let plans: Vec<Plan> = store.plans()?.into_iter().filter(|p| p.auto && !p.paused).collect();
    if plans.is_empty() {
        return Ok(0);
    }
    let ops = store.ops()?;
    let paid = paid_dates(&ops);
    let empty = BTreeSet::new();
    let accounts = store.accounts()?;
    let debts = store.debts()?;
    let fx = fx_now(store)?;
    let mut new = Vec::new();
    for p in &plans {
        let done = paid.get(p.id.as_str()).unwrap_or(&empty);
        for d in p.dates_until(today) {
            if done.contains(&d) || p.skipped.contains(&d) || store.knows_op(&pay_id(&p.id, d))? {
                continue;
            }
            new.push(plan_op(p, d, &accounts, &fx, &debts));
        }
    }
    let n = new.len();
    for op in new {
        store.put_op(op)?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::{ensure_defaults, snapshot};
    use crate::sync::EventBody;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn plan(id: &str, start: &str, every: Every) -> Plan {
        Plan {
            id: id.into(),
            title: "Интернет".into(),
            amount: 650_00,
            account: "acc-personal".into(),
            cat: "telecom".into(),
            every,
            start: d(start),
            until: None,
            auto: false,
            remind: None,
            debt: None,
            skipped: vec![],
            paused: false,
            note: String::new(),
            extra: Default::default(),
        }
    }

    #[test]
    fn schedule_keeps_day_of_month_and_stops_at_until() {
        let mut p = plan("p", "2026-01-31", Every::Month);
        let dates: Vec<String> = p.dates_until(d("2026-05-01")).iter().map(|x| x.to_string()).collect();
        assert_eq!(dates, ["2026-01-31", "2026-02-28", "2026-03-31", "2026-04-30"], "31-е в коротком месяце — последний день, потом снова 31-е");
        p.until = Some(d("2026-03-15"));
        assert_eq!(p.dates_until(d("2027-01-01")).len(), 2);
        let w = plan("w", "2026-11-02", Every::Week);
        assert_eq!(w.dates_until(d("2026-11-30")).len(), 5);
        let y = plan("y", "2024-02-29", Every::Year);
        assert_eq!(y.occurrence(1), d("2025-02-28"));
        assert_eq!(y.occurrence(4), d("2028-02-29"));
    }

    #[test]
    fn reminder_texts_have_no_amounts() {
        let due = vec![
            Due { kind: "plan", id: "p".into(), title: "Интернет".into(), date: d("2026-11-10"), amount: 650_00, days: 3, lead: 3, soon: true, auto: false, i_owe: true },
            Due { kind: "plan", id: "a".into(), title: "Подписка".into(), date: d("2026-11-08"), amount: 299_00, days: 1, lead: 1, soon: true, auto: true, i_owe: true },
            Due { kind: "debt", id: "x".into(), title: "Иван".into(), date: d("2026-11-07"), amount: 5000_00, days: 0, lead: 3, soon: true, auto: false, i_owe: false },
            Due { kind: "plan", id: "old".into(), title: "Аренда".into(), date: d("2026-11-01"), amount: 1_00, days: -6, lead: 3, soon: true, auto: false, i_owe: true },
        ];
        let r = reminders(&due, d("2026-11-07"));
        let texts: Vec<(String, String)> = r.iter().map(|x| (x.day.to_string(), x.text.clone())).collect();
        assert_eq!(
            texts,
            [
                ("2026-11-10".to_string(), "Сегодня платёж: Интернет".to_string()),
                ("2026-11-07".to_string(), "Через 3 дня платёж: Интернет".to_string()),
                ("2026-11-08".to_string(), "Сегодня автосписание: Подписка".to_string()),
                ("2026-11-07".to_string(), "Завтра автосписание: Подписка".to_string()),
                ("2026-11-07".to_string(), "Сегодня срок: Иван возвращает долг".to_string()),
            ],
            "о просроченном — без уведомлений; напоминание за 3 дня до срока у долга от 7-го не выходит раньше сегодня"
        );
        assert!(r.iter().all(|x| !x.text.contains('₽') && !x.text.chars().any(|c| c.is_ascii_digit() && !x.text.starts_with("Через"))));
        assert_eq!(when(5), "Через 5 дней");
        assert_eq!(when(21), "Через 21 день");
    }

    #[test]
    fn plans_debts_credit_and_auto_pay() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("f.db"), &[8u8; 32], "dev").unwrap();
        ensure_defaults(&mut s).unwrap();
        let today = d("2026-11-07");
        // Кредит: должен банку 30 000, платёж 10 000 каждое 5-е, списывается сам.
        let credit = Debt { id: "loan".into(), who: "Банк".into(), dir: DebtDir::IOwe, amount: 30_000_00, date: d("2026-09-01"), due: None, note: String::new(), closed: false, extra: Default::default() };
        let mut pay = plan("loan-pay", "2026-10-05", Every::Month);
        pay.title = "Кредит".into();
        pay.amount = 10_000_00;
        pay.auto = true;
        pay.debt = Some("loan".into());
        // Интернет 650 ₽ каждое 10-е, кнопкой; октябрь пропущен.
        let mut net = plan("net", "2026-10-10", Every::Month);
        net.skipped = vec![d("2026-10-10")];
        // Иван должен 5 000 до 9 ноября; уже вернул 2 000.
        let ivan = Debt { id: "ivan".into(), who: "Иван".into(), dir: DebtDir::OwedToMe, amount: 5_000_00, date: d("2026-10-01"), due: Some(d("2026-11-09")), note: String::new(), closed: false, extra: Default::default() };
        s.append_all(vec![EventBody::DebtPut { debt: credit }, EventBody::DebtPut { debt: ivan }, EventBody::PlanPut { plan: pay.clone() }, EventBody::PlanPut { plan: net.clone() }]).unwrap();
        let back = Op { id: "r1".into(), kind: OpKind::Debt, cat: "debt".into(), title: "Иван вернул".into(), amount: 2_000_00, account: "acc-cash".into(), debt: Some("ivan".into()), inflow: true, ..plan_op(&net, today, &[], &BTreeMap::new(), &[]) };
        s.put_op(Op { plan: None, plan_date: None, ..back }).unwrap();

        // Автосписание: 5 октября и 5 ноября, каждое один раз.
        assert_eq!(auto_pay(&mut s, today).unwrap(), 2);
        assert_eq!(auto_pay(&mut s, today).unwrap(), 0, "повторно не пишет");
        // Удалённое вручную не возвращается.
        s.delete_op(&pay_id("loan-pay", d("2026-11-05"))).unwrap();
        assert_eq!(auto_pay(&mut s, today).unwrap(), 0, "удалённое списание не возвращается");

        let snap = snapshot(&s, today).unwrap();
        let loan = snap.debts.iter().find(|v| v.debt.id == "loan").unwrap();
        assert_eq!((loan.repaid, loan.left, loan.plan.as_deref()), (10_000_00, 20_000_00, Some("loan-pay")), "платёж по кредиту уменьшает долг");
        let iv = snap.debts.iter().find(|v| v.debt.id == "ivan").unwrap();
        assert_eq!((iv.left, iv.overdue), (3_000_00, false));
        assert_eq!((snap.i_owe, snap.owed_to_me), (20_000_00, 3_000_00));
        // Долг не уменьшает «Мои деньги»: на личной карте минус 10 000 только от записанного платежа.
        let personal = snap.accounts.iter().find(|a| a.account.id == "acc-personal").unwrap();
        assert_eq!(personal.balance, -10_000_00);

        let net_view = snap.plans.iter().find(|v| v.plan.id == "net").unwrap();
        assert_eq!((net_view.next, net_view.overdue), (Some(d("2026-11-10")), 0), "пропущенный октябрь не просрочен");
        let pay_view = snap.plans.iter().find(|v| v.plan.id == "loan-pay").unwrap();
        assert_eq!((pay_view.next, pay_view.overdue, pay_view.paid), (Some(d("2026-11-05")), 1, 1), "удалённое списание — просроченный платёж");

        let up: Vec<(String, String, i32, bool)> = snap.upcoming.iter().map(|u| (u.kind.to_string(), u.date.to_string(), u.days, u.soon)).collect();
        assert_eq!(
            up[..4],
            [
                ("plan".to_string(), "2026-11-05".to_string(), -2, true),
                ("debt".to_string(), "2026-11-09".to_string(), 2, true),
                ("plan".to_string(), "2026-11-10".to_string(), 3, true),
                ("plan".to_string(), "2026-12-05".to_string(), 28, false),
            ]
        );
        // Оплата кнопкой: интернет за 10 ноября.
        let op = plan_op(&net, d("2026-11-10"), &s.accounts().unwrap(), &BTreeMap::new(), &[]);
        assert_eq!((op.id.as_str(), op.kind, op.cat.as_str(), op.amount), ("plan-net-2026-11-10", OpKind::Personal, "telecom", 650_00));
        s.put_op(op).unwrap();
        let snap = snapshot(&s, today).unwrap();
        assert_eq!(snap.plans.iter().find(|v| v.plan.id == "net").unwrap().next, Some(d("2026-12-10")));
        // Кредит выплачен — по графику больше не напоминаем.
        for m in ["2026-12-05", "2027-01-05"] {
            let op = plan_op(&pay, d(m), &s.accounts().unwrap(), &BTreeMap::new(), &s.debts().unwrap());
            s.put_op(op).unwrap();
        }
        s.put_op(plan_op(&pay, d("2026-11-05"), &s.accounts().unwrap(), &BTreeMap::new(), &s.debts().unwrap())).unwrap();
        let snap = snapshot(&s, today).unwrap();
        assert_eq!(snap.debts.iter().find(|v| v.debt.id == "loan").unwrap().left, 0);
        assert!(snap.upcoming.iter().all(|u| u.id != "loan-pay"), "закрытый кредит не напоминает");
    }

    #[test]
    fn settings_lead_and_plan_override() {
        let mut st = BTreeMap::new();
        assert_eq!(settings_lead(&st), 3, "по умолчанию за 3 дня");
        st.insert("remind.days".to_string(), "1".to_string());
        assert_eq!(settings_lead(&st), 1);
        let mut p = plan("p", "2026-11-10", Every::Month);
        assert_eq!(p.lead(1), 1);
        p.remind = Some(7);
        assert_eq!(p.lead(1), 7);
    }
}
