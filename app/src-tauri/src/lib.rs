//! «Карман»: мост между экранами и ядром.
//!
//! Экраны получают одну сводку (`snapshot`) и после каждого изменения — новую.
//! Вся логика расчётов — в ядре `karman-core`. Вход по PIN-коду, отпечатку и Windows Hello и ключи
//! в хранилище системы — из общего модуля kit (`kit_tauri::guard`, `names`): пока «Карман» заблокирован,
//! команды с данными отвечают `locked`.

mod ai;
mod backup;
mod claude;
mod cloud;
mod fx;
mod lan;
mod names;
mod notify;
mod overtime;

use karman_core::book::{self, Account, Snapshot};
use karman_core::budget::{Budget, FAMILY, PERSONAL};
use karman_core::finance::{self, Debt, DebtDir, Plan};
use karman_core::savings::Goal;
use karman_core::db::{is_plaintext_sqlite, Store};
use karman_core::money::{fmt2, rub_text};
use karman_core::receipt::parse_fiscal_qr;
use karman_core::pairing::random_key;
use karman_core::sync::EventBody;
use karman_core::trip::{self, Op, Trip, DEFAULT_RATE};
use karman_core::{demo, Date};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::time::Instant;
use tauri::{AppHandle, Manager, State};

use kit_tauri::guard::{self, Lock};

struct AppState {
    store: Arc<Mutex<Store>>,
    me: Identity,
    data_dir: PathBuf,
    db_path: PathBuf,
    /// Связь по Wi-Fi: код, связанные устройства, сервер обмена.
    /// Вход по PIN-коду: заблокирован ли «Карман».
    guard: Lock,
}

impl AppState {
    /// База для команд с данными: пока «Карман» заблокирован, ответ — `locked`.
    fn store(&self) -> Res<MutexGuard<'_, Store>> {
        self.unlocked()?;
        self.store.lock().map_err(err)
    }

    /// Пока «Карман» заблокирован, команды с данными отвечают `locked`.
    fn unlocked(&self) -> Res<()> {
        self.guard.check()
    }
}

/// Местные настройки базы — для входа по PIN-коду: там лежат его хеш и счётчик ошибок.
struct StoreMeta(Arc<Mutex<Store>>);

impl guard::Meta for StoreMeta {
    fn get(&self, key: &str) -> Res<Option<String>> {
        self.0.lock().map_err(err)?.meta(key).map_err(err)
    }

    fn set(&self, key: &str, value: &str) -> Res<()> {
        self.0.lock().map_err(err)?.set_meta(key, value).map_err(err)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Identity {
    id: String,
    name: String,
}

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn day(s: &str) -> Res<Date> {
    Date::parse(s).ok_or_else(|| format!("неверная дата «{s}»"))
}

fn default_name() -> String {
    if cfg!(target_os = "android") {
        return "Телефон".into();
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Компьютер".into())
}

fn load_identity(dir: &Path) -> Res<Identity> {
    let path = dir.join("device.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(id) = serde_json::from_str::<Identity>(&text) {
            return Ok(id);
        }
    }
    let id = Identity { id: book::new_id().map_err(err)?, name: default_name() };
    save_identity(dir, &id)?;
    Ok(id)
}

fn save_identity(dir: &Path, id: &Identity) -> Res<()> {
    std::fs::write(dir.join("device.json"), serde_json::to_string_pretty(id).map_err(err)?).map_err(err)
}

/// Ключ базы. На Windows и Android он хранится зашифрованным средствами системы (key.sec):
/// DPAPI или Android Keystore. Открытый key.bin прежних версий переносится туда и стирается —
/// но только после того, как защищённая копия записана и проверена.
/// На Linux (разработка) ключ остаётся в key.bin.
fn load_key(dir: &Path) -> Res<[u8; 32]> {
    let sec = dir.join("key.sec");
    let plain = dir.join("key.bin");
    if let Ok(blob) = std::fs::read(&sec) {
        let k = names::unprotect(&blob).map_err(|e| format!("ключ базы не достать из хранилища системы: {e}"))?;
        return <[u8; 32]>::try_from(k.as_slice()).map_err(|_| "ключ базы повреждён".to_string());
    }
    let key = match std::fs::read(&plain).ok().and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()) {
        Some(k) => k,
        None => random_key().map_err(err)?,
    };
    if kit_secure::supported() {
        match protect_key(&sec, &key) {
            Ok(()) => wipe(&plain),
            Err(e) => {
                // Хранилище системы не сработало — ключ остаётся в файле, попробуем при следующем запуске.
                eprintln!("ключ базы не сохранён в хранилище системы: {e}");
                if !plain.exists() {
                    std::fs::write(&plain, key).map_err(err)?;
                }
            }
        }
    } else if !plain.exists() {
        std::fs::write(&plain, key).map_err(err)?;
    }
    Ok(key)
}

fn protect_key(path: &Path, key: &[u8; 32]) -> Res<()> {
    let blob = names::protect(key).map_err(err)?;
    if names::unprotect(&blob).map_err(err)?.as_slice() != key {
        return Err("проверка защищённого ключа не сошлась".into());
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &blob).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

/// Затирает файл нулями и удаляет.
fn wipe(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::write(path, vec![0u8; meta.len() as usize]);
        let _ = std::fs::remove_file(path);
    }
}

/// Открывает базу. Если ключ или база не открываются, прежние файлы откладываются в сторону
/// (не удаляются) и начинается чистая база — данные можно вернуть синхронизацией или из резервной копии.
/// Чистая база получает новый номер устройства: иначе её записи пошли бы под теми же номерами,
/// что и прежние, и при синхронизации или восстановлении часть записей потерялась бы.
fn open_store(dir: &Path, db_path: &Path, me: &mut Identity) -> Res<(Store, Option<String>)> {
    let opened = load_key(dir).and_then(|k| Store::open(db_path, &k, &me.id).map_err(err));
    match opened {
        Ok(s) => Ok((s, None)),
        Err(e) => {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            for name in ["karman.db", "karman.db-wal", "karman.db-shm", "key.sec", "key.bin"] {
                let p = dir.join(name);
                if p.exists() {
                    let _ = std::fs::rename(&p, dir.join(format!("{name}.{stamp}.old")));
                }
            }
            *me = Identity { id: book::new_id().map_err(err)?, name: me.name.clone() };
            save_identity(dir, me)?;
            let k = load_key(dir)?;
            let s = Store::open(db_path, &k, &me.id).map_err(err)?;
            Ok((s, Some(format!("Прежнюю базу открыть не удалось ({e}). Её файлы сохранены рядом, с пометкой .old. Данные можно вернуть синхронизацией с другого устройства или из резервной копии: Настройки → Резервная копия."))))
        }
    }
}

/// Тестовые операции этапа 0 больше не нужны: удаляем их один раз.
fn cleanup_stage0(store: &mut Store) -> Res<()> {
    if store.meta("cleanup-stage0").map_err(err)?.is_some() {
        return Ok(());
    }
    let old: Vec<EventBody> = store
        .ops()
        .map_err(err)?
        .into_iter()
        .filter(|o| o.title.starts_with("Проверка с устройства"))
        .map(|o| EventBody::OpDelete { id: o.id })
        .collect();
    if !old.is_empty() {
        store.append_all(old).map_err(err)?;
    }
    store.set_meta("cleanup-stage0", "1").map_err(err)
}

fn snap(st: &Store, today: &str) -> Res<Snapshot> {
    let s = book::snapshot(st, day(today)?).map_err(err)?;
    // Изменились платежи или долги — перепланировать уведомления телефона.
    notify::after_snapshot(&s);
    Ok(s)
}

// ——— Сводка и изменения ———

/// Сводка для экранов. Заодно записывает автосписания, чей день настал;
/// `tz` — смещение местного времени от UTC в минутах, для напоминаний.
#[tauri::command]
fn snapshot(state: State<AppState>, today: String, tz: Option<i32>) -> Res<Snapshot> {
    let mut st = state.store()?;
    if let Some(tz) = tz {
        notify::set_tz(&st, tz);
    }
    if let Err(e) = finance::auto_pay(&mut st, day(&today)?) {
        eprintln!("автосписания не записаны: {e}");
    }
    snap(&st, &today)
}

#[tauri::command]
fn op_save(state: State<AppState>, today: String, mut op: Op) -> Res<Snapshot> {
    if op.amount <= 0 {
        return Err("Введите сумму".into());
    }
    if op.id.is_empty() {
        op.id = book::new_id().map_err(err)?;
    }
    let mut st = state.store()?;
    st.put_op(op).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn op_delete(state: State<AppState>, today: String, id: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    st.delete_op(&id).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn trip_save(state: State<AppState>, today: String, trip: Trip) -> Res<Snapshot> {
    if trip.id.trim().is_empty() || trip.city.trim().is_empty() {
        return Err("Нужны номер поездки и город".into());
    }
    if trip.end < trip.start || trip.plan_end < trip.plan_start {
        return Err("Дата окончания раньше начала".into());
    }
    let mut st = state.store()?;
    st.append(EventBody::TripPut { trip }).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn trip_delete(state: State<AppState>, today: String, id: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    let mut bodies: Vec<EventBody> = st
        .ops()
        .map_err(err)?
        .into_iter()
        .filter(|o| o.trip.as_deref() == Some(id.as_str()))
        .map(|o| EventBody::OpDelete { id: o.id })
        .collect();
    // Вместе с поездкой — её наряды и рабочие дни.
    bodies.extend(st.orders().map_err(err)?.into_iter().filter(|o| o.trip == id).map(|o| EventBody::OrderDelete { id: o.id }));
    bodies.extend(st.work_days().map_err(err)?.into_iter().filter(|d| d.trip == id).map(|d| EventBody::DayDelete { id: d.id }));
    bodies.push(EventBody::TripDelete { id });
    st.append_all(bodies).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn trip_advance(state: State<AppState>, today: String, id: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    book::advance_trip(&mut st, &id, day(&today)?).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn trip_settle(state: State<AppState>, today: String, id: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    book::settle_trip(&mut st, &id, day(&today)?).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn account_save(state: State<AppState>, today: String, mut account: Account) -> Res<Snapshot> {
    if account.name.trim().is_empty() {
        return Err("Нужно название счёта".into());
    }
    if account.currency.trim().is_empty() {
        return Err("Нужна валюта счёта".into());
    }
    if account.id.is_empty() {
        account.id = format!("acc-{}", book::new_id().map_err(err)?);
    }
    let mut st = state.store()?;
    st.append(EventBody::AccountPut { account }).map_err(err)?;
    snap(&st, &today)
}

/// Своя категория личных трат: новая (без номера) или переименованная, с иконкой из набора.
#[tauri::command]
fn category_save(state: State<AppState>, today: String, mut category: karman_core::category::Category) -> Res<Snapshot> {
    let mut st = state.store()?;
    let custom = st.categories().map_err(err)?;
    karman_core::category::check(&mut category, &custom)?;
    st.append(EventBody::CategoryPut { category }).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn setting_set(state: State<AppState>, today: String, key: String, value: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    st.append(EventBody::SettingPut { key, value }).map_err(err)?;
    snap(&st, &today)
}

// ——— Долги и регулярные платежи ———

/// Деньги прошли через счёт, когда долг появился: получил в долг или дал в долг.
#[derive(Deserialize)]
struct DebtMoney {
    account: String,
}

#[tauri::command]
fn debt_save(state: State<AppState>, today: String, mut debt: Debt, money: Option<DebtMoney>) -> Res<Snapshot> {
    debt.who = debt.who.trim().to_string();
    if debt.who.is_empty() {
        return Err("Укажите, кто должен или кому".into());
    }
    if debt.amount <= 0 {
        return Err("Введите сумму долга".into());
    }
    if debt.id.is_empty() {
        debt.id = format!("debt-{}", book::new_id().map_err(err)?);
    }
    let mut st = state.store()?;
    // Номер нового долга может прийти с экрана (чтобы сразу связать с ним график платежей).
    let new = !st.debts().map_err(err)?.iter().any(|d| d.id == debt.id);
    let mut bodies = Vec::new();
    if let (true, Some(m)) = (new, money) {
        // «Я должен» — деньги пришли на счёт, «мне должны» — ушли со счёта.
        let i_owe = debt.dir == DebtDir::IOwe;
        let title = if i_owe { format!("В долг от — {}", debt.who) } else { format!("В долг — {}", debt.who) };
        bodies.push(EventBody::OpPut { op: debt_op(format!("{}-start", debt.id), debt.date, title, debt.amount, m.account, &debt.id, i_owe) });
    }
    bodies.insert(0, EventBody::DebtPut { debt });
    st.append_all(bodies).map_err(err)?;
    snap(&st, &today)
}

fn debt_op(id: String, date: Date, title: String, amount: i64, account: String, debt: &str, inflow: bool) -> Op {
    Op {
        id,
        date,
        kind: trip::OpKind::Debt,
        cat: "debt".into(),
        title,
        amount,
        account,
        trip: None,
        doc: None,
        sent: false,
        family: false,
        debt: Some(debt.into()),
        inflow,
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

/// Удаляет долг и его график платежей. Записанные операции остаются в истории.
#[tauri::command]
fn debt_delete(state: State<AppState>, today: String, id: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    let mut bodies: Vec<EventBody> = st.plans().map_err(err)?.into_iter().filter(|p| p.debt.as_deref() == Some(id.as_str())).map(|p| EventBody::PlanDelete { id: p.id }).collect();
    bodies.push(EventBody::DebtDelete { id });
    st.append_all(bodies).map_err(err)?;
    snap(&st, &today)
}

/// Возврат долга: я отдаю (я должен) или мне отдают (мне должны).
#[tauri::command]
fn debt_pay(state: State<AppState>, today: String, id: String, amount: i64, account: String, date: String) -> Res<Snapshot> {
    if amount <= 0 {
        return Err("Введите сумму".into());
    }
    let mut st = state.store()?;
    let d = st.debts().map_err(err)?.into_iter().find(|d| d.id == id).ok_or("долг не найден")?;
    let op = debt_op(book::new_id().map_err(err)?, day(&date)?, format!("Возврат долга — {}", d.who), amount, account, &d.id, d.dir == DebtDir::OwedToMe);
    st.put_op(op).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn plan_save(state: State<AppState>, today: String, mut plan: Plan) -> Res<Snapshot> {
    plan.title = plan.title.trim().to_string();
    if plan.title.is_empty() {
        return Err("Укажите название платежа".into());
    }
    if plan.amount <= 0 {
        return Err("Введите сумму платежа".into());
    }
    if plan.until.is_some_and(|u| u < plan.start) {
        return Err("Последний платёж раньше первого".into());
    }
    if plan.id.is_empty() {
        plan.id = book::new_id().map_err(err)?;
    }
    let mut st = state.store()?;
    if !st.accounts().map_err(err)?.iter().any(|a| a.id == plan.account) {
        return Err("Выберите счёт".into());
    }
    st.append(EventBody::PlanPut { plan }).map_err(err)?;
    // Автоплатёж с датой в прошлом или сегодня записывается сразу.
    if let Err(e) = finance::auto_pay(&mut st, day(&today)?) {
        eprintln!("автосписания не записаны: {e}");
    }
    snap(&st, &today)
}

#[tauri::command]
fn plan_delete(state: State<AppState>, today: String, id: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    st.append(EventBody::PlanDelete { id }).map_err(err)?;
    snap(&st, &today)
}

/// «Оплачено»: записывает платёж за эту дату. Сумму и счёт можно поправить.
#[tauri::command]
fn plan_pay(state: State<AppState>, today: String, id: String, date: String, amount: Option<i64>, account: Option<String>) -> Res<Snapshot> {
    let mut st = state.store()?;
    let mut plan = st.plans().map_err(err)?.into_iter().find(|p| p.id == id).ok_or("платёж не найден")?;
    if let Some(a) = amount.filter(|a| *a > 0) {
        plan.amount = a;
    }
    if let Some(acc) = account.filter(|a| !a.is_empty()) {
        plan.account = acc;
    }
    let fx = book::fx_now(&st).map_err(err)?;
    let op = finance::plan_op(&plan, day(&date)?, &st.accounts().map_err(err)?, &fx, &st.debts().map_err(err)?);
    if op.amount <= 0 {
        return Err("Нет курса валюты счёта: укажите его в настройках".into());
    }
    st.put_op(op).map_err(err)?;
    snap(&st, &today)
}

/// «Пропустить»: в этот раз платежа нет.
#[tauri::command]
fn plan_skip(state: State<AppState>, today: String, id: String, date: String) -> Res<Snapshot> {
    let d = day(&date)?;
    let mut st = state.store()?;
    let mut plan = st.plans().map_err(err)?.into_iter().find(|p| p.id == id).ok_or("платёж не найден")?;
    if !plan.skipped.contains(&d) {
        plan.skipped.push(d);
        plan.skipped.sort();
    }
    st.append(EventBody::PlanPut { plan }).map_err(err)?;
    snap(&st, &today)
}

// ——— Цели и бюджет ———

#[tauri::command]
fn goal_save(state: State<AppState>, today: String, mut goal: Goal) -> Res<Snapshot> {
    goal.title = goal.title.trim().to_string();
    if goal.title.is_empty() {
        return Err("Укажите, на что копите".into());
    }
    if goal.target <= 0 {
        return Err("Введите, сколько нужно".into());
    }
    if goal.id.is_empty() {
        goal.id = format!("goal-{}", book::new_id().map_err(err)?);
    }
    let mut st = state.store()?;
    if !st.accounts().map_err(err)?.iter().any(|a| a.id == goal.account) {
        return Err("Выберите счёт, куда откладывать".into());
    }
    st.append(EventBody::GoalPut { goal }).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn goal_delete(state: State<AppState>, today: String, id: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    st.append(EventBody::GoalDelete { id }).map_err(err)?;
    snap(&st, &today)
}

/// «Отложить» — перевод со счёта на счёт цели; `take` — «Взять из цели», обратно.
#[tauri::command]
fn goal_move(state: State<AppState>, today: String, id: String, amount: i64, account: String, date: String, take: bool) -> Res<Snapshot> {
    if amount <= 0 {
        return Err("Введите сумму".into());
    }
    let mut st = state.store()?;
    let goal = st.goals().map_err(err)?.into_iter().find(|g| g.id == id).ok_or("цель не найдена")?;
    if account == goal.account {
        return Err("Выберите другой счёт: этот — счёт цели".into());
    }
    let accounts = st.accounts().map_err(err)?;
    for a in [&account, &goal.account] {
        match accounts.iter().find(|x| &x.id == a) {
            None => return Err("Счёт не найден".into()),
            Some(x) if x.currency != "RUB" => return Err("Цели копятся в рублях: выберите рублёвый счёт".into()),
            _ => {}
        }
    }
    let (from, to, title) = if take { (goal.account.clone(), account, format!("С цели: {}", goal.title)) } else { (account, goal.account.clone(), format!("На цель: {}", goal.title)) };
    let op = Op {
        kind: trip::OpKind::Transfer,
        cat: "transfer".into(),
        to: Some(to),
        goal: Some(goal.id.clone()),
        ..debt_op(book::new_id().map_err(err)?, day(&date)?, title, amount, from, "", false)
    };
    st.put_op(Op { debt: None, ..op }).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
fn budget_save(state: State<AppState>, today: String, mut budget: Budget) -> Res<Snapshot> {
    if budget.id != PERSONAL && budget.id != FAMILY {
        return Err("неизвестный бюджет".into());
    }
    budget.total = budget.total.filter(|t| *t > 0);
    budget.cats.retain(|_, v| *v > 0);
    let mut st = state.store()?;
    st.append(EventBody::BudgetPut { budget }).map_err(err)?;
    snap(&st, &today)
}

// ——— Устройство и проверки ———

#[derive(Serialize)]
struct Check {
    ok: bool,
    text: String,
}

fn check(ok: bool, text: impl Into<String>) -> Check {
    Check { ok, text: text.into() }
}

#[derive(Serialize)]
struct DeviceInfo {
    id: String,
    name: String,
    os: String,
    arch: String,
    version: String,
    data_dir: String,
    encrypted: bool,
    cipher: String,
    events: u64,
    /// Записи из более новой версии «Кармана»: видны после обновления.
    unknown: usize,
}

#[tauri::command]
fn device_info(state: State<AppState>) -> Res<DeviceInfo> {
    let st = state.store()?;
    Ok(DeviceInfo {
        id: state.me.id.clone(),
        name: state.me.name.clone(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        data_dir: state.data_dir.display().to_string(),
        encrypted: !is_plaintext_sqlite(&state.db_path).unwrap_or(true),
        cipher: st.cipher_version().map_err(err)?,
        events: st.event_count().map_err(err)?,
        unknown: st.unknown_events().map_err(err)?,
    })
}

/// Отдельная проверочная база: запись, чтение, шифрование файла, отказ без ключа.
#[tauri::command]
fn db_check(state: State<AppState>) -> Res<Vec<Check>> {
    state.unlocked()?;
    let path = state.data_dir.join("check.db");
    let remove = |p: &Path| {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", p.display()));
        }
    };
    remove(&path);
    let key = random_key().map_err(err)?;
    let t0 = Instant::now();
    let rows = {
        let mut s = Store::open(&path, &key, "check").map_err(err)?;
        s.bench_rows(2_000).map_err(err)?
    };
    let write_ms = t0.elapsed().as_millis();
    let t1 = Instant::now();
    let reopened = Store::open(&path, &key, "check").map(|mut s| s.bench_rows(0).unwrap_or(0));
    let read_ms = t1.elapsed().as_millis();
    let plaintext = is_plaintext_sqlite(&path).map_err(err)?;
    let wrong = Store::open(&path, &random_key().map_err(err)?, "check");
    let mut checks = vec![
        check(rows == 2_000, format!("Записано {rows} строк за {write_ms} мс")),
        check(matches!(reopened, Ok(2_000)), format!("Открыто заново с ключом и прочитано за {read_ms} мс")),
        check(!plaintext, "Файл на диске зашифрован: без ключа это не база SQLite"),
        check(wrong.is_err(), "С чужим ключом база не открывается"),
    ];
    drop(wrong);
    remove(&path);
    checks.push(check(!is_plaintext_sqlite(&state.db_path).unwrap_or(true), "Основная база приложения тоже зашифрована"));
    Ok(checks)
}

/// Расчёт поездок из макета: должен совпасть с 1С до копейки.
#[tauri::command]
fn calc_check() -> Vec<Check> {
    let ops = demo::ops();
    let mut checks = Vec::new();
    for t in demo::trips() {
        let rate = trip::effective_rate(&t, t.status, DEFAULT_RATE, None);
        let s = trip::settle(&t, t.status, &ops, rate, demo::today());
        let rows = trip::report(&t, &s, &ops);
        let fact: i64 = rows.iter().map(|r| r.fact).sum();
        if t.id == "573" {
            checks.push(check(fact == 5_564_110, format!("573: израсходовано {} — в 1С 55 641,10", fmt2(fact))));
            checks.push(check(s.balance == 6_635_890, format!("573: вернуть {} — в 1С 66 358,90", fmt2(s.balance))));
            checks.push(check(rows[3].over == 70_000, "573: сверх бюджета 700,00 по суточным"));
        }
        if t.id == "552" {
            checks.push(check(s.debt_due == 860_000, format!("552: долг перед компанией {}", rub_text(s.debt_due))));
        }
    }
    checks
}

// ——— Связь устройств ———

// ——— Снимки и файлы чеков ———

#[derive(Serialize)]
struct BlobInfo {
    id: String,
    size: usize,
}

/// Сохраняет снимок или файл чека (base64) в зашифрованной базе.
#[tauri::command]
fn blob_put(state: State<AppState>, mime: String, data: String) -> Res<BlobInfo> {
    let bytes = B64.decode(data.as_bytes()).map_err(|_| "файл не прочитался".to_string())?;
    let st = state.store()?;
    let id = st.put_blob(&mime, &bytes).map_err(err)?;
    Ok(BlobInfo { id, size: bytes.len() })
}

/// Вложение для показа на экране (data URL). None — снимок ещё не пришёл с другого устройства.
#[tauri::command]
fn blob_url(state: State<AppState>, id: String) -> Res<Option<String>> {
    let st = state.store()?;
    Ok(st.blob(&id).map_err(err)?.map(|(mime, data)| format!("data:{mime};base64,{}", B64.encode(data))))
}

/// Архив чеков поездки в папку «Загрузки» (на компьютере). Возвращает путь к файлу.
#[tauri::command]
fn receipts_zip(app: AppHandle, state: State<AppState>, trip: String) -> Res<String> {
    let (files, city) = {
        let st = state.store()?;
        let city = st.trips().map_err(err)?.into_iter().find(|t| t.id == trip).map(|t| t.city).unwrap_or_default();
        (book::trip_receipts(&st, &trip).map_err(err)?, city)
    };
    if files.is_empty() {
        return Err("У поездки пока нет снимков и файлов чеков".into());
    }
    let zip = karman_core::zip::build(&files).map_err(err)?;
    save_download(&app, &format!("Чеки {trip} {city}"), "zip", &zip)
}

/// Файл в «Загрузки» (если система её не знает — Downloads в домашней папке) с именем без повторов;
/// на Windows он сразу показывается в Проводнике.
fn save_download(app: &AppHandle, name: &str, ext: &str, data: &[u8]) -> Res<String> {
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir().map(|h| h.join("Downloads")))
        .map_err(|_| "не нашлась папка «Загрузки»".to_string())?;
    std::fs::create_dir_all(&dir).map_err(err)?;
    let base = karman_core::zip::safe_name(name);
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    std::fs::write(&path, data).map_err(err)?;
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    }
    Ok(path.display().to_string())
}

// ——— Статистика и выгрузка ———

fn stats_for(st: &Store, today: &str, span: &str, anchor: &str) -> Res<karman_core::stats::Stats> {
    let ops = st.ops().map_err(err)?;
    let rate = book::settings_rate(&st.settings().map_err(err)?);
    Ok(karman_core::stats::stats(&ops, &st.accounts().map_err(err)?, &st.trips().map_err(err)?, rate, karman_core::stats::Span::parse(span), day(anchor)?, day(today)?))
}

/// Статистика за месяц, квартал или год, в который попадает `anchor`.
#[tauri::command]
fn stats(state: State<AppState>, today: String, span: String, anchor: String) -> Res<karman_core::stats::Stats> {
    let st = state.store()?;
    stats_for(&st, &today, &span, &anchor)
}

/// Операции периода и итоги — файлом Excel в «Загрузки» (на компьютере).
#[tauri::command]
fn export_xlsx(app: AppHandle, state: State<AppState>, today: String, span: String, anchor: String) -> Res<String> {
    if cfg!(mobile) {
        return Err("Выгрузка в Excel — на компьютере: свяжите устройства по Wi-Fi и выгрузите там".into());
    }
    let (bytes, name) = {
        let st = state.store()?;
        let s = stats_for(&st, &today, &span, &anchor)?;
        (karman_core::export::workbook(&st.ops().map_err(err)?, &st.accounts().map_err(err)?, &st.categories().map_err(err)?, &s).map_err(err)?, karman_core::export::file_name(&s))
    };
    save_download(&app, &name, "xlsx", &bytes)
}

// ——— Чек ———

#[derive(Serialize)]
struct ReceiptView {
    date: String,
    time: String,
    sum: i64,
    fn_number: String,
    fd: String,
    fp: String,
    kind: String,
}

#[tauri::command]
fn receipt_parse(text: String) -> Res<ReceiptView> {
    let r = parse_fiscal_qr(&text).map_err(err)?;
    Ok(ReceiptView {
        date: r.date.to_string(),
        time: r.time.clone(),
        sum: r.sum,
        fn_number: r.fn_number.clone(),
        fd: r.fd.clone(),
        fp: r.fp.clone(),
        kind: r.kind_title().into(),
    })
}

/// `karman --mcp`: коннектор для приложения Claude (посредник MCP через stdio), без окна.
#[cfg(desktop)]
pub fn mcp_relay() -> i32 {
    claude::relay()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // Второй запуск (например, из меню «Пуск», пока «Карман» в трее) открывает уже работающий.
    #[cfg(windows)]
    {
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| notify::show_main(app)))
            .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])));
    }
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_barcode_scanner::init()).plugin(tauri_plugin_biometric::init());
    }
    // Окно выбора папки для резервных копий.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_dialog::init());
    }
    builder
        .plugin(tauri_plugin_notification::init())
        // Страница входа в облако — в браузере системы (вызывается из ядра, экрану права не нужны).
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let mut me = load_identity(&dir)?;
            let db_path = dir.join("karman.db");
            let (mut store, notice) = open_store(&dir, &db_path, &mut me)?;
            backup::clean_uploads(&dir);
            cleanup_stage0(&mut store)?;
            book::ensure_defaults(&mut store)?;
            // Снимки, которые так и не прикрепили к операции, убираем через сутки.
            let _ = store.gc_blobs();
            let today = Date::today_utc().to_string();
            // С PIN-кодом «Карман» открывается заблокированным.
            let store = Arc::new(Mutex::new(store));
            let handle = app.handle().clone();
            let hooks = guard::Hooks {
                // Пока «Карман» заблокирован, по прежнему коду связи не подключиться.
                on_lock: Box::new(move || lan::on_lock(&handle)),
                // Вернулись в «Карман» — заодно обменяться с облаком, если давно не обменивались.
                on_resume: Box::new(cloud::resumed),
            };
            let guard = Lock::new(names::APP, StoreMeta(store.clone()), hooks);
            guard.set_notice(notice);
            app.manage(guard.clone());
            #[cfg(desktop)]
            let claude_dir = dir.clone();
            app.manage(cloud::Sky::new(cloud::settings(), store.clone(), Some(Box::new(cloud::ByCode))));
            app.manage(lan::Wifi::new(&karman_core::lan::LAN, &names::KEYS, store.clone(), me.id.clone(), me.name.clone()));
            app.manage(AppState {
                store,
                me,
                data_dir: dir,
                db_path,
                guard,
            });
            // Коннектор для приложения Claude: ждём посредника `karman --mcp` на 127.0.0.1 —
            // после того как база открыта, чтобы первый же запрос нашёл её.
            #[cfg(desktop)]
            claude::start(app.handle().clone(), claude_dir);
            fx::refresh_on_start(app.handle().clone(), today);
            notify::init(app.handle());
            notify::start_desktop_loop(app.handle().clone());
            backup::start_auto(app.handle().clone());
            cloud::start(app.handle().clone());
            lan::start(app.handle().clone());
            if notify::TRAY.load(std::sync::atomic::Ordering::Relaxed) {
                #[cfg(windows)]
                notify::tray(app.handle())?;
                // Запуск вместе с Windows — сразу в трей, без окна.
                if std::env::args().any(|a| a == "--hidden") {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(focused) => guard::on_focus(window.app_handle(), *focused),
            // В режиме «уведомлением Windows» крестик прячет окно в трей: напоминания продолжают работать.
            tauri::WindowEvent::CloseRequested { api, .. } if notify::TRAY.load(std::sync::atomic::Ordering::Relaxed) => {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            op_save,
            op_delete,
            trip_save,
            trip_delete,
            trip_advance,
            trip_settle,
            account_save,
            setting_set,
            category_save,
            device_info,
            db_check,
            calc_check,
            lan::lan_status,
            lan::lan_scan,
            lan::lan_code_start,
            lan::lan_code_stop,
            lan::lan_pair,
            lan::lan_pair_qr,
            lan::lan_sync_now,
            lan::lan_unpair,
            receipt_parse,
            blob_put,
            blob_url,
            receipts_zip,
            fx::fx_rate,
            fx::fx_refresh,
            debt_save,
            debt_delete,
            debt_pay,
            plan_save,
            plan_delete,
            plan_pay,
            plan_skip,
            goal_save,
            goal_delete,
            goal_move,
            budget_save,
            stats,
            export_xlsx,
            overtime::order_save,
            overtime::order_delete,
            overtime::day_save,
            overtime::ot_apply,
            overtime::overtime_month,
            overtime::overtime_export,
            claude::claude_status,
            claude::claude_set,
            claude::claude_undo,
            claude::claude_bundle,
            ai::ai_status,
            ai::ai_key_set,
            ai::ai_key_delete,
            ai::ai_workspace_set,
            ai::ai_set,
            ai::ai_check,
            ai::ai_order_scan,
            ai::ai_phrase,
            ai::ai_doc_scan,
            ai::ai_trip_scan,
            ai::ai_chat_send,
            ai::ai_chat_view,
            ai::ai_chat_reset,
            ai::ai_chat_confirm,
            ai::ai_trip_check,
            ai::month_insights,
            ai::ai_month_summary,
            backup::backup_status,
            backup::backup_password,
            backup::backup_every,
            backup::backup_dir,
            backup::backup_pick_dir,
            backup::backup_now,
            backup::backup_upload,
            backup::backup_inspect,
            backup::backup_restore,
            backup::backup_discard,
            cloud::cloud_status,
            cloud::cloud_login,
            cloud::cloud_open_page,
            cloud::cloud_app_passwords,
            cloud::cloud_dav_login,
            cloud::cloud_login_cancel,
            cloud::cloud_login_check,
            cloud::cloud_password,
            cloud::cloud_sync_now,
            cloud::cloud_every,
            cloud::cloud_disconnect,
            notify::remind_status,
            notify::remind_desktop,
            guard::lock_status,
            guard::pin_unlock,
            guard::pin_set,
            guard::pin_off,
            guard::bio_unlock,
            guard::bio_set,
            guard::pin_recover,
            guard::lock_now,
            guard::app_visible
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить «Карман»");
}
