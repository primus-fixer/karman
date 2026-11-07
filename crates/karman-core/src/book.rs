//! Счета, действия с поездками и общая сводка для экранов.

use crate::date::Date;
use crate::db::Store;
use crate::error::{format_err, Error, Result};
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
    /// work — рабочая карта (для поездок), personal — личная, cash — наличные.
    pub kind: String,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub archived: bool,
    /// Поля из более новых версий.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn account(id: &str, name: &str, short: &str, kind: &str, order: i32) -> Account {
    Account { id: id.into(), name: name.into(), short: short.into(), currency: "RUB".into(), opening: 0, kind: kind.into(), order, archived: false, extra: Default::default() }
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
        if let Some(cur) = k.strip_prefix("fx.") {
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
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub today: Date,
    pub device: String,
    pub rate: Kop,
    pub fx: BTreeMap<String, f64>,
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
}

pub fn trip_view(t: &Trip, ops: &[Op], settings_rate: Kop, today: Date) -> TripView {
    let (rate, rate_source) = trip::rate_with_source(t, t.status, settings_rate, t.own_rate);
    let s = trip::settle(t, t.status, ops, rate, today);
    let rows = trip::report(t, &s, ops);
    let over_total = rows.iter().map(|r| r.over).sum();
    let auto_just = trip::over_budget_note(t, &s, &rows);
    TripView { trip: t.clone(), rate, rate_source, s, rows, over_total, auto_just }
}

/// Всё, что нужно экранам, одним ответом.
pub fn snapshot(store: &Store, today: Date) -> Result<Snapshot> {
    let settings = store.settings()?;
    let fx = fx_rates(&settings);
    let rate = settings_rate(&settings);
    let ops = store.ops()?;
    let accounts: Vec<AccountView> = store
        .accounts()?
        .into_iter()
        .map(|a| {
            let foreign = a.currency != "RUB";
            let moved: Kop = ops
                .iter()
                .filter(|o| o.account == a.id)
                .map(|o| {
                    let v = if foreign { o.orig.unwrap_or(o.amount) } else { o.amount };
                    if o.is_inflow() {
                        v
                    } else {
                        -v
                    }
                })
                .sum();
            let balance = a.opening + moved;
            let rate = fx.get(&a.currency).copied().unwrap_or(1.0);
            let balance_rub = if foreign { (balance as f64 * rate).round() as Kop } else { balance };
            AccountView { account: a, balance, balance_rub }
        })
        .collect();
    let total = accounts.iter().filter(|a| !a.account.archived).map(|a| a.balance_rub).sum();
    let trips: Vec<TripView> = store.trips()?.iter().map(|t| trip_view(t, &ops, rate, today)).collect();
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
    Ok(Snapshot { today, device: store.device().to_string(), rate, fx, accounts, total, held, due, owed, debt_due, mine: total - held - due, trips, ops, settings })
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
