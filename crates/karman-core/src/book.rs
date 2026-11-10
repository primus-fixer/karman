//! Счета, действия с поездками и общая сводка для экранов.

use crate::date::Date;
use crate::db::Store;
use crate::error::{format_err, Error, Result};
use crate::budget::{self, BudgetView};
use crate::finance::{self, DebtDir, DebtView, Due, PlanView};
use crate::savings::{self, Deposit, DepositView, GoalView};
use crate::money::Kop;
use crate::sync::EventBody;
use crate::trip::{self, Op, OpKind, ReportRow, Settlement, Trip, TripStatus, DEFAULT_RATE};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    /// Короткое имя для кнопок ввода.
    pub short: String,
    /// RUB, KZT, USD…
    pub currency: String,
    /// Остаток, с которого начат учёт, в валюте счёта.
    pub opening: Kop,
    /// work — рабочая карта (для поездок), personal — личная, cash — наличные,
    /// savings — накопительный, deposit — вклад.
    pub kind: String,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub archived: bool,
    /// Вклад: ставка, срок, капитализация.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit: Option<Deposit>,
    /// Поля из более новых версий.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn account(id: &str, name: &str, short: &str, kind: &str, order: i32) -> Account {
    Account { id: id.into(), name: name.into(), short: short.into(), currency: "RUB".into(), opening: 0, kind: kind.into(), order, archived: false, deposit: None, extra: Default::default() }
}

/// Счета, с которых начинается пустая база. Номера постоянные: если два устройства
/// заведут их до первой синхронизации, дублей не будет.
pub fn default_accounts() -> Vec<Account> {
    vec![
        account("acc-work", "Рабочая карта", "Рабочая", "work", 1),
        account("acc-personal", "Личная карта", "Личная", "personal", 2),
        account("acc-cash", "Наличные", "Наличные", "cash", 3),
    ]
}

pub fn ensure_defaults(store: &mut Store) -> Result<bool> {
    if !store.accounts()?.is_empty() {
        return Ok(false);
    }
    store.append_all(default_accounts().into_iter().map(|account| EventBody::AccountPut { account }).collect())?;
    Ok(true)
}

pub fn new_id() -> Result<String> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|e| Error::Random(e.to_string()))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Курс к рублю: сколько рублей за единицу валюты.
pub fn fx_rates(settings: &BTreeMap<String, String>) -> BTreeMap<String, f64> {
    let mut fx = BTreeMap::from([("RUB".to_string(), 1.0), ("KZT".to_string(), 0.154), ("USD".to_string(), 80.5)]);
    for (k, v) in settings {
        // Только коды валют (fx.KZT), не другие настройки курсов (fx.auto).
        if let Some(cur) = k.strip_prefix("fx.").filter(|c| c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase())) {
            if let Ok(r) = v.replace(',', ".").parse::<f64>() {
                if r > 0.0 {
                    fx.insert(cur.to_string(), r);
                }
            }
        }
    }
    fx
}

pub fn settings_rate(settings: &BTreeMap<String, String>) -> Kop {
    settings.get("rate").and_then(|v| v.parse::<Kop>().ok()).filter(|r| *r >= 0).unwrap_or(DEFAULT_RATE)
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountView {
    pub account: Account,
    /// Остаток в валюте счёта.
    pub balance: Kop,
    pub balance_rub: Kop,
}

#[derive(Clone, Debug, Serialize)]
pub struct TripView {
    pub trip: Trip,
    pub rate: Kop,
    /// own — своя ставка поездки, fixed — из сданного отчёта, settings — из настроек.
    pub rate_source: &'static str,
    pub s: Settlement,
    pub rows: Vec<ReportRow>,
    pub over_total: Kop,
    /// Подставленное обоснование сверхбюджета.
    pub auto_just: String,
    /// Наряды и переработки поездки.
    pub ot: crate::overtime::TripOt,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub today: Date,
    pub device: String,
    pub rate: Kop,
    pub fx: BTreeMap<String, f64>,
    /// Курсы берутся с сайта ЦБ (настройка «fx.auto», по умолчанию да).
    pub fx_auto: bool,
    /// Дата курсов ЦБ, если они уже загружены.
    pub fx_date: Option<String>,
    /// Курсы из настроек: запасные, если курс ЦБ не загрузился.
    pub fx_manual: BTreeMap<String, f64>,
    pub accounts: Vec<AccountView>,
    /// Всего на счетах, в рублях.
    pub total: Kop,
    /// Деньги компании в идущих поездках.
    pub held: Kop,
    /// Вернуть компании по завершённым поездкам.
    pub due: Kop,
    /// Компания должна.
    pub owed: Kop,
    /// Долг перед компанией (входит в «вернуть»).
    pub debt_due: Kop,
    /// На счетах минус подотчётные и то, что нужно вернуть.
    pub mine: Kop,
    pub trips: Vec<TripView>,
    pub ops: Vec<Op>,
    pub settings: BTreeMap<String, String>,
    /// Долги и кредиты с остатками.
    pub debts: Vec<DebtView>,
    /// Регулярные платежи с ближайшей датой.
    pub plans: Vec<PlanView>,
    /// Платежи и сроки долгов на месяц вперёд и всё просроченное.
    pub upcoming: Vec<Due>,
    /// Всего я должен и мне должны — отдельно от «Моих денег».
    pub i_owe: Kop,
    pub owed_to_me: Kop,
    /// За сколько дней напоминать (настройка «remind.days»).
    pub remind_days: i32,
    /// На накопительных счетах и вкладах — строка «Отложено».
    pub saved: Kop,
    /// Отложенное входит в «Мои деньги» (настройка «savings.in_mine», по умолчанию нет).
    pub saved_in_mine: bool,
    /// Прогноз по вкладам.
    pub deposits: Vec<DepositView>,
    /// Цели и сколько на них отложено.
    pub goals: Vec<GoalView>,
    /// Бюджет на месяц: личный и семейный.
    pub budgets: Vec<BudgetView>,
    /// С какого числа начинается месяц бюджета.
    pub budget_day: u32,
    /// Свои категории личных трат (скрытые — в настройках «cat.hidden.<код>»).
    pub categories: Vec<crate::category::Category>,
}

pub fn trip_view(t: &Trip, ops: &[Op], settings_rate: Kop, today: Date) -> TripView {
    let (rate, rate_source) = trip::rate_with_source(t, t.status, settings_rate, t.own_rate);
    let s = trip::settle(t, t.status, ops, rate, today);
    let rows = trip::report(t, &s, ops);
    let over_total = rows.iter().map(|r| r.over).sum();
    let auto_just = trip::over_budget_note(t, &s, &rows);
    TripView { trip: t.clone(), rate, rate_source, s, rows, over_total, auto_just, ot: Default::default() }
}

/// Как операция меняет остаток счёта (в валюте счёта). Перевод списывает со счёта-источника
/// и зачисляет на счёт-получатель; остальные операции двигают только свой счёт.
pub fn account_delta(o: &Op, account: &str, foreign: bool) -> Kop {
    if o.kind == OpKind::Transfer {
        let mut d = 0;
        if o.account == account {
            d -= if foreign { o.orig.unwrap_or(o.amount) } else { o.amount };
        }
        if o.to.as_deref() == Some(account) {
            // Между счетами в одной валюте сумма та же, что ушла.
            d += if foreign { o.to_amount.or(o.orig).unwrap_or(o.amount) } else { o.amount };
        }
        return d;
    }
    if o.account != account {
        return 0;
    }
    let v = if foreign { o.orig.unwrap_or(o.amount) } else { o.amount };
    if o.is_inflow() {
        v
    } else {
        -v
    }
}

/// Курсы ЦБ, сохранённые на этом устройстве: дата и рублей за единицу валюты.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CbrRates {
    pub date: String,
    pub rates: BTreeMap<String, f64>,
}

/// Ключ meta для последних курсов ЦБ и для курсов на дату.
pub const META_CBR: &str = "fx.cbr";
pub fn meta_cbr_on(date: &str) -> String {
    format!("fx.cbr.{date}")
}

/// Курсы для пересчёта остатков: по умолчанию — ЦБ, а ручные из настроек — запасные.
fn fx_with_cbr(settings: &BTreeMap<String, String>, cbr: Option<&CbrRates>) -> BTreeMap<String, f64> {
    let mut fx = fx_rates(settings);
    let auto = settings.get("fx.auto").map(|v| v != "0").unwrap_or(true);
    if auto {
        if let Some(c) = cbr {
            for (k, v) in &c.rates {
                if *v > 0.0 {
                    fx.insert(k.clone(), *v);
                }
            }
        }
    }
    fx
}

/// Курсы для пересчёта сейчас: ЦБ, если включены и загружены, иначе из настроек.
pub fn fx_now(store: &Store) -> Result<BTreeMap<String, f64>> {
    let cbr: Option<CbrRates> = store.meta(META_CBR)?.and_then(|t| serde_json::from_str(&t).ok());
    Ok(fx_with_cbr(&store.settings()?, cbr.as_ref()))
}

/// Всё, что нужно экранам, одним ответом.
pub fn snapshot(store: &Store, today: Date) -> Result<Snapshot> {
    let settings = store.settings()?;
    let cbr: Option<CbrRates> = store.meta(META_CBR)?.and_then(|t| serde_json::from_str(&t).ok());
    let fx = fx_with_cbr(&settings, cbr.as_ref());
    let fx_manual = fx_rates(&settings);
    let fx_auto = settings.get("fx.auto").map(|v| v != "0").unwrap_or(true);
    let fx_date = if fx_auto { cbr.as_ref().map(|c| c.date.clone()) } else { None };
    let rate = settings_rate(&settings);
    let ops = store.ops()?;
    let accounts: Vec<AccountView> = store
        .accounts()?
        .into_iter()
        .map(|a| {
            let foreign = a.currency != "RUB";
            let moved: Kop = ops.iter().map(|o| account_delta(o, &a.id, foreign)).sum();
            let balance = a.opening + moved;
            let rate = fx.get(&a.currency).copied().unwrap_or(1.0);
            let balance_rub = if foreign { (balance as f64 * rate).round() as Kop } else { balance };
            AccountView { account: a, balance, balance_rub }
        })
        .collect();
    let total = accounts.iter().filter(|a| !a.account.archived).map(|a| a.balance_rub).sum();
    let (orders, days) = (store.orders()?, store.work_days()?);
    let trips: Vec<TripView> = store
        .trips()?
        .iter()
        .map(|t| TripView { ot: crate::overtime::trip_ot(t, &orders, &days, &settings), ..trip_view(t, &ops, rate, today) })
        .collect();
    let (mut held, mut due, mut owed, mut debt_due) = (0, 0, 0, 0);
    for v in &trips {
        let b = v.s.balance;
        if b > 0 && v.trip.status <= TripStatus::InTrip {
            held += b;
        } else if b > 0 {
            due += b;
            debt_due += v.s.debt_due;
        } else if b < 0 {
            owed -= b;
        }
    }
    let plain: Vec<Account> = accounts.iter().map(|a| a.account.clone()).collect();
    let plans = finance::plan_views(&store.plans()?, &ops, &plain, &fx, today);
    let debts = finance::debt_views(&store.debts()?, &plans, &ops, today);
    let remind_days = finance::settings_lead(&settings);
    let deposits = savings::deposit_views(&accounts, today);
    let mut upcoming = finance::upcoming(&plans, &debts, &ops, today, remind_days);
    upcoming.extend(finance::deposit_ends(&accounts, &deposits, today, remind_days));
    upcoming.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.title.cmp(&b.title)));
    let saved: Kop = accounts.iter().filter(|a| !a.account.archived && savings::is_saving(&a.account.kind)).map(|a| a.balance_rub).sum();
    let saved_in_mine = savings::in_mine(&settings);
    let goals = savings::goal_views(&store.goals()?, &ops, today);
    let budget_day = budget::settings_day(&settings);
    let budgets = budget::budget_views(&store.budgets()?, &ops, today, budget_day);
    let open = |dir: DebtDir| debts.iter().filter(|v| v.open() && v.debt.dir == dir).map(|v| v.left).sum::<Kop>();
    let (i_owe, owed_to_me) = (open(DebtDir::IOwe), open(DebtDir::OwedToMe));
    Ok(Snapshot {
        today,
        device: store.device().to_string(),
        rate,
        fx,
        fx_auto,
        fx_date,
        fx_manual,
        accounts,
        total,
        held,
        due,
        owed,
        debt_due,
        mine: total - held - due - if saved_in_mine { 0 } else { saved },
        trips,
        ops,
        settings,
        debts,
        plans,
        upcoming,
        i_owe,
        owed_to_me,
        remind_days,
        saved,
        saved_in_mine,
        deposits,
        goals,
        budgets,
        budget_day,
        categories: store.categories()?,
    })
}

fn find_trip(store: &Store, id: &str) -> Result<Trip> {
    store.trips()?.into_iter().find(|t| t.id == id).ok_or_else(|| format_err(format!("нет поездки {id}")))
}

fn work_account(store: &Store) -> Result<String> {
    let acc = store.accounts()?;
    Ok(acc
        .iter()
        .find(|a| a.kind == "work" && !a.archived)
        .or_else(|| acc.first())
        .map(|a| a.id.clone())
        .unwrap_or_else(|| "acc-work".into()))
}

fn personal_account(store: &Store) -> Result<String> {
    let acc = store.accounts()?;
    Ok(acc
        .iter()
        .find(|a| a.kind == "personal" && !a.archived)
        .or_else(|| acc.first())
        .map(|a| a.id.clone())
        .unwrap_or_else(|| "acc-personal".into()))
}

fn company_op(id: String, date: Date, kind: OpKind, cat: &str, title: String, amount: Kop, account: String, trip: &str) -> Op {
    Op {
        id,
        date,
        kind,
        cat: cat.into(),
        title,
        amount,
        account,
        trip: Some(trip.to_string()),
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

/// Следующий статус поездки. При получении аванса записывается аванс в сумме заявки,
/// при сдаче отчёта фиксируется ставка суточных.
pub fn advance_trip(store: &mut Store, id: &str, today: Date) -> Result<Trip> {
    let mut t = find_trip(store, id)?;
    let next = t.status.next().ok_or_else(|| format_err("поездка уже закрыта"))?;
    let mut bodies = Vec::new();
    if next == TripStatus::AdvanceReceived && t.plan_total() > 0 {
        let acc = work_account(store)?;
        bodies.push(EventBody::OpPut {
            op: company_op(new_id()?, today, OpKind::Advance, "advance", format!("Аванс по заявке {}", t.id), t.plan_total(), acc, &t.id),
        });
    }
    if next == TripStatus::Review && t.own_rate.is_none() && t.fixed_rate.is_none() {
        t.fixed_rate = Some(settings_rate(&store.settings()?));
    }
    t.status = next;
    bodies.push(EventBody::TripPut { trip: t.clone() });
    store.append_all(bodies)?;
    Ok(t)
}

/// Отмечает расчёт с компанией: возврат остатка или полученное возмещение.
pub fn settle_trip(store: &mut Store, id: &str, today: Date) -> Result<Option<Op>> {
    let t = find_trip(store, id)?;
    let settings = store.settings()?;
    let v = trip_view(&t, &store.ops()?, settings_rate(&settings), today);
    if !v.s.finished || v.s.balance == 0 {
        return Ok(None);
    }
    let op = if v.s.balance > 0 {
        company_op(new_id()?, today, OpKind::Return, "ret", format!("Возврат по заявке {}", t.id), v.s.balance, work_account(store)?, &t.id)
    } else {
        company_op(new_id()?, today, OpKind::Reimbursement, "reimb", format!("Возмещение по заявке {}", t.id), -v.s.balance, personal_account(store)?, &t.id)
    };
    store.put_op(op.clone())?;
    Ok(Some(op))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;
    use crate::money::rub;

    fn demo_store(dir: &std::path::Path) -> Store {
        let mut s = Store::open(&dir.join("k.db"), &[5u8; 32], "dev-a").unwrap();
        ensure_defaults(&mut s).unwrap();
        let mut bodies: Vec<EventBody> = demo::trips().into_iter().map(|trip| EventBody::TripPut { trip }).collect();
        bodies.extend(demo::ops().into_iter().map(|mut op| {
            op.account = format!("acc-{}", op.account);
            EventBody::OpPut { op }
        }));
        s.append_all(bodies).unwrap();
        s
    }

    #[test]
    fn snapshot_matches_mockup_totals() {
        let dir = tempfile::tempdir().unwrap();
        let s = demo_store(dir.path());
        let snap = snapshot(&s, demo::today()).unwrap();
        assert_eq!(snap.held, rub(27300.0));
        assert_eq!(snap.due, rub(75858.90));
        assert_eq!(snap.owed, rub(1000.0));
        assert_eq!(snap.debt_due, rub(8600.0));
        assert_eq!(snap.mine, snap.total - snap.held - snap.due);
        let work = snap.accounts.iter().find(|a| a.account.id == "acc-work").unwrap();
        // Авансы на рабочую карту минус расходы и возврат с неё.
        let expected: Kop = demo::ops()
            .iter()
            .filter(|o| o.account == "work")
            .map(|o| if o.is_inflow() { o.amount } else { -o.amount })
            .sum();
        assert_eq!(work.balance, expected);
        let t573 = snap.trips.iter().find(|t| t.trip.id == "573").unwrap();
        assert_eq!(t573.s.balance, rub(66358.90));
        assert_eq!(t573.over_total, rub(700.0));
        assert_eq!(t573.rate_source, "settings");
    }

    #[test]
    fn currency_accounts_convert_to_rubles() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("c.db"), &[6u8; 32], "dev-a").unwrap();
        let mut kzt = account("acc-kzt", "Наличные, тенге", "₸", "cash", 4);
        kzt.currency = "KZT".into();
        kzt.opening = rub(18000.0);
        s.append(EventBody::AccountPut { account: kzt }).unwrap();
        s.append(EventBody::SettingPut { key: "fx.KZT".into(), value: "0,2".into() }).unwrap();
        let mut op = demo::ops().remove(1);
        op.trip = None;
        op.kind = OpKind::Personal;
        op.account = "acc-kzt".into();
        op.orig = Some(rub(3000.0));
        op.cur = Some("KZT".into());
        op.amount = rub(600.0);
        s.put_op(op).unwrap();
        let snap = snapshot(&s, demo::today()).unwrap();
        let a = &snap.accounts[0];
        assert_eq!(a.balance, rub(15000.0), "остаток в тенге");
        assert_eq!(a.balance_rub, rub(3000.0), "по курсу 0,2");
    }

    #[test]
    fn advance_and_settle_create_company_operations() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("d.db"), &[7u8; 32], "dev-a").unwrap();
        ensure_defaults(&mut s).unwrap();
        let mut t = demo::trips().into_iter().find(|t| t.id == "573").unwrap();
        t.status = TripStatus::Request;
        s.append(EventBody::TripPut { trip: t }).unwrap();
        let t = advance_trip(&mut s, "573", demo::today()).unwrap();
        assert_eq!(t.status, TripStatus::AdvanceReceived);
        let adv: Vec<Op> = s.ops().unwrap().into_iter().filter(|o| o.kind == OpKind::Advance).collect();
        assert_eq!(adv.len(), 1);
        assert_eq!(adv[0].amount, rub(122000.0), "аванс равен заявке");
        assert_eq!(adv[0].account, "acc-work");
        advance_trip(&mut s, "573", demo::today()).unwrap();
        advance_trip(&mut s, "573", demo::today()).unwrap();
        // Отчёт ещё не сдан, расходов нет: вернуть весь аванс без суточных.
        let op = settle_trip(&mut s, "573", demo::today()).unwrap().unwrap();
        assert_eq!(op.kind, OpKind::Return);
        assert_eq!(op.amount, rub(122000.0) - rub(7700.0));
        let t = advance_trip(&mut s, "573", demo::today()).unwrap();
        assert_eq!(t.status, TripStatus::Review);
        assert_eq!(t.fixed_rate, Some(DEFAULT_RATE), "ставка фиксируется при сдаче отчёта");
        let snap = snapshot(&s, demo::today()).unwrap();
        assert_eq!(snap.trips[0].s.balance, 0);
    }

    #[test]
    fn transfers_move_money_between_own_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("t.db"), &[6u8; 32], "dev").unwrap();
        ensure_defaults(&mut s).unwrap();
        let mut kzt = default_accounts().remove(2);
        kzt.id = "acc-kzt".into();
        kzt.currency = "KZT".into();
        kzt.short = "₸".into();
        s.append(EventBody::AccountPut { account: kzt }).unwrap();
        s.append(EventBody::SettingPut { key: "fx.KZT".into(), value: "0.2".into() }).unwrap();
        let mut base = crate::demo::ops().remove(0);
        base.trip = None;
        base.doc = None;
        // Снял 5 000 ₽ с личной карты наличными.
        let cash = Op { id: "t1".into(), kind: OpKind::Transfer, cat: "transfer".into(), title: "Снял наличные".into(), amount: crate::money::rub(5000.0), account: "acc-personal".into(), to: Some("acc-cash".into()), ..base.clone() };
        s.put_op(cash.clone()).unwrap();
        // Обменял 3 000 ₽ на 15 000 ₸.
        let fx = Op { id: "t2".into(), kind: OpKind::Transfer, cat: "transfer".into(), title: "Обмен".into(), amount: crate::money::rub(3000.0), account: "acc-cash".into(), to: Some("acc-kzt".into()), to_amount: Some(crate::money::rub(15000.0)), ..base.clone() };
        s.put_op(fx).unwrap();
        let snap = snapshot(&s, crate::demo::today()).unwrap();
        let bal = |id: &str| snap.accounts.iter().find(|a| a.account.id == id).unwrap().balance;
        assert_eq!(bal("acc-personal"), -crate::money::rub(5000.0));
        assert_eq!(bal("acc-cash"), crate::money::rub(2000.0));
        assert_eq!(bal("acc-kzt"), crate::money::rub(15000.0), "в тенге");
        assert_eq!(snap.total, crate::money::rub(0.0), "по курсу 0,2 перевод ничего не меняет в сумме: −5000 + 2000 + 3000");
        // Курсы ЦБ важнее ручных, пока не выключены.
        s.set_meta(META_CBR, &serde_json::to_string(&CbrRates { date: "2026-11-06".into(), rates: BTreeMap::from([("KZT".into(), 0.18)]) }).unwrap()).unwrap();
        let snap = snapshot(&s, crate::demo::today()).unwrap();
        assert_eq!((snap.fx["KZT"], snap.fx_date.as_deref()), (0.18, Some("2026-11-06")));
        s.append(EventBody::SettingPut { key: "fx.auto".into(), value: "0".into() }).unwrap();
        let snap = snapshot(&s, crate::demo::today()).unwrap();
        assert_eq!((snap.fx["KZT"], snap.fx_date), (0.2, None), "ЦБ выключен — ручной курс");
        s.append(EventBody::SettingPut { key: "fx.auto".into(), value: "1".into() }).unwrap();
        let snap = snapshot(&s, crate::demo::today()).unwrap();
        assert!(!snap.fx.contains_key("auto"), "настройка fx.auto — не валюта");
        assert_eq!((snap.fx["KZT"], snap.fx_manual["KZT"]), (0.18, 0.2), "ручной курс виден как запасной");
        // Между счетами в одной валюте приходит столько же, сколько ушло.
        let same = Op { to: Some("acc-kzt2".into()), orig: Some(700_00), cur: Some("KZT".into()), ..cash.clone() };
        assert_eq!(account_delta(&same, "acc-kzt2", true), 700_00);
        // Удалённый перевод ничего не двигает.
        s.delete_op("t1").unwrap();
        let snap = snapshot(&s, crate::demo::today()).unwrap();
        assert_eq!(snap.accounts.iter().find(|a| a.account.id == "acc-personal").unwrap().balance, 0);
    }

}

#[cfg(test)]
mod savings_tests {
    use super::*;
    use crate::savings::{Deposit, KIND_DEPOSIT, KIND_SAVINGS};

    /// Накопления — строкой «Отложено», вне «Моих денег»; вклад напоминает о конце срока.
    #[test]
    fn savings_apart_from_my_money() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("s.db"), &[5u8; 32], "dev").unwrap();
        ensure_defaults(&mut s).unwrap();
        let today = Date::parse("2026-11-07").unwrap();
        let mut card = default_accounts().remove(1);
        card.opening = crate::money::rub(50000.0);
        let mut save = account("acc-save", "Копилка", "Копилка", KIND_SAVINGS, 4);
        save.opening = crate::money::rub(20000.0);
        let mut dep = account("acc-dep", "Вклад Сбер", "Вклад", KIND_DEPOSIT, 5);
        dep.opening = crate::money::rub(100000.0);
        dep.deposit = Some(Deposit { rate: 12.0, opened: Date::parse("2026-05-10").unwrap(), until: Some(Date::parse("2026-11-10").unwrap()), capitalize: false });
        s.append_all(vec![EventBody::AccountPut { account: card }, EventBody::AccountPut { account: save }, EventBody::AccountPut { account: dep }]).unwrap();
        let snap = snapshot(&s, today).unwrap();
        assert_eq!((snap.total, snap.saved, snap.mine), (crate::money::rub(170000.0), crate::money::rub(120000.0), crate::money::rub(50000.0)), "по умолчанию отложенное — отдельно");
        let dv = &snap.deposits[0];
        assert_eq!((dv.account.as_str(), dv.days_left, dv.month), ("acc-dep", Some(3), crate::money::rub(1000.0)));
        let end = snap.upcoming.iter().find(|u| u.kind == "deposit").expect("конец вклада в «Скоро»");
        assert_eq!((end.title.as_str(), end.days, end.soon), ("Вклад Сбер", 3, true));
        let texts: Vec<String> = finance::reminders(&snap.upcoming, today).into_iter().map(|r| r.text).collect();
        assert!(texts.contains(&"Через 3 дня заканчивается вклад: Вклад Сбер".to_string()), "{texts:?}");
        s.append(EventBody::SettingPut { key: "savings.in_mine".into(), value: "1".into() }).unwrap();
        let snap = snapshot(&s, today).unwrap();
        assert_eq!((snap.mine, snap.saved_in_mine), (crate::money::rub(170000.0), true), "настройка: отложенное в «Моих деньгах»");
    }
}

/// Чеки поездки для архива: имена «дата название сумма», без повторов.
pub fn trip_receipts(store: &Store, trip_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let mut ops: Vec<Op> = store.ops()?.into_iter().filter(|o| o.trip.as_deref() == Some(trip_id) && !o.att.is_empty()).collect();
    ops.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for o in ops {
        for (i, a) in o.att.iter().enumerate() {
            let Some((mime, data)) = store.blob(&a.id)? else { continue };
            let ext = match mime.as_str() {
                "image/png" => "png",
                "image/webp" => "webp",
                "application/pdf" => "pdf",
                _ => "jpg",
            };
            let base = crate::zip::safe_name(&format!("{} {} {}", o.date, o.title, crate::money::fmt2(o.amount).replace('\u{a0}', " ")));
            let mut name = if o.att.len() > 1 { format!("{base} ({}).{ext}", i + 1) } else { format!("{base}.{ext}") };
            let mut n = 2;
            while !used.insert(name.clone()) {
                name = format!("{base} ({n}).{ext}");
                n += 1;
            }
            out.push((name, data));
        }
    }
    Ok(out)

}
