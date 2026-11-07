//! «Карман»: мост между экранами и ядром.
//!
//! Экраны получают одну сводку (`snapshot`) и после каждого изменения — новую.
//! Вся логика расчётов — в ядре `karman-core`. Вход по PIN-коду и отпечатку — в `guard`.

mod guard;

use karman_core::book::{self, Account, Snapshot};
use karman_core::db::{is_plaintext_sqlite, Store};
use karman_core::money::{fmt2, rub_text};
use karman_core::pairing::{random_key, Pairing};
use karman_core::receipt::parse_fiscal_qr;
use karman_core::sync::{self, EventBody, Server};
use karman_core::trip::{self, Op, Trip, DEFAULT_RATE};
use karman_core::{demo, Date};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tauri::{Manager, State};

use guard::{Guard, LOCKED};

/// Порт для обмена по Wi-Fi. Если занят, берётся любой свободный.
const SYNC_PORT: u16 = 47_500;

struct AppState {
    store: Arc<Mutex<Store>>,
    me: Identity,
    data_dir: PathBuf,
    db_path: PathBuf,
    server: Mutex<Option<(Server, Pairing)>>,
    guard: Mutex<Guard>,
    notice: Mutex<Option<String>>,
}

impl AppState {
    /// База для команд с данными: пока «Карман» заблокирован, ответ — `locked`.
    fn store(&self) -> Res<MutexGuard<'_, Store>> {
        if self.guard.lock().map_err(err)?.locked {
            return Err(LOCKED.into());
        }
        self.store.lock().map_err(err)
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
    std::fs::write(&path, serde_json::to_string_pretty(&id).map_err(err)?).map_err(err)?;
    Ok(id)
}

/// Ключ базы. На Windows и Android он хранится зашифрованным средствами системы (key.sec):
/// DPAPI или Android Keystore. Открытый key.bin прежних версий переносится туда и стирается —
/// но только после того, как защищённая копия записана и проверена.
/// На Linux (разработка) ключ остаётся в key.bin.
fn load_key(dir: &Path) -> Res<[u8; 32]> {
    let sec = dir.join("key.sec");
    let plain = dir.join("key.bin");
    if let Ok(blob) = std::fs::read(&sec) {
        let k = karman_secure::unprotect(&blob).map_err(|e| format!("ключ базы не достать из хранилища системы: {e}"))?;
        return <[u8; 32]>::try_from(k.as_slice()).map_err(|_| "ключ базы повреждён".to_string());
    }
    let key = match std::fs::read(&plain).ok().and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()) {
        Some(k) => k,
        None => random_key().map_err(err)?,
    };
    if karman_secure::supported() {
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
    let blob = karman_secure::protect(key).map_err(err)?;
    if karman_secure::unprotect(&blob).map_err(err)?.as_slice() != key {
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
/// (не удаляются) и начинается чистая база — данные можно вернуть синхронизацией.
fn open_store(dir: &Path, db_path: &Path, device: &str) -> Res<(Store, Option<String>)> {
    let opened = load_key(dir).and_then(|k| Store::open(db_path, &k, device).map_err(err));
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
            let k = load_key(dir)?;
            let s = Store::open(db_path, &k, device).map_err(err)?;
            Ok((s, Some(format!("Прежнюю базу открыть не удалось ({e}). Её файлы сохранены рядом, с пометкой .old. Данные можно вернуть синхронизацией с другого устройства."))))
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
    book::snapshot(st, day(today)?).map_err(err)
}

// ——— Сводка и изменения ———

#[tauri::command]
fn snapshot(state: State<AppState>, today: String) -> Res<Snapshot> {
    let st = state.store()?;
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

#[tauri::command]
fn setting_set(state: State<AppState>, today: String, key: String, value: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    st.append(EventBody::SettingPut { key, value }).map_err(err)?;
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
    })
}

/// Отдельная проверочная база: запись, чтение, шифрование файла, отказ без ключа.
#[tauri::command]
fn db_check(state: State<AppState>) -> Res<Vec<Check>> {
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

#[derive(Serialize)]
struct PairingView {
    uri: String,
    qr_svg: String,
    hosts: Vec<String>,
    port: u16,
}

fn local_ipv4() -> Vec<String> {
    let mut out: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(v4) if !v4.is_link_local() => Some(v4.to_string()),
            _ => None,
        })
        .collect();
    // Сначала адреса домашних сетей и точки доступа телефона.
    out.sort_by_key(|ip| if ip.starts_with("192.168.") { 0 } else if ip.starts_with("10.") { 1 } else { 2 });
    out.dedup();
    out
}

fn qr_svg(text: &str) -> Res<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(err)?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(232, 232)
        .dark_color(svg::Color("#15232D"))
        .light_color(svg::Color("#FFFFFF"))
        .build())
}

fn pairing_view(p: &Pairing) -> Res<PairingView> {
    let uri = p.to_uri();
    Ok(PairingView { qr_svg: qr_svg(&uri)?, uri, hosts: p.hosts.clone(), port: p.port })
}

/// Начинает ждать подключения и отдаёт QR-код для другого устройства.
#[tauri::command]
fn sync_start(state: State<AppState>) -> Res<PairingView> {
    drop(state.store()?);
    let mut slot = state.server.lock().map_err(err)?;
    if let Some((_, p)) = slot.as_ref() {
        return pairing_view(p);
    }
    let hosts = local_ipv4();
    if hosts.is_empty() {
        return Err("Нет подключения к сети: включите Wi-Fi или точку доступа".into());
    }
    let key = random_key().map_err(err)?;
    let server = Server::start(state.store.clone(), key, state.me.name.clone(), SYNC_PORT)
        .or_else(|_| Server::start(state.store.clone(), key, state.me.name.clone(), 0))
        .map_err(err)?;
    let p = Pairing { device: state.me.id.clone(), name: state.me.name.clone(), hosts, port: server.port, key };
    let view = pairing_view(&p)?;
    *slot = Some((server, p));
    Ok(view)
}

#[derive(Serialize)]
struct SyncStatus {
    running: bool,
    log: Vec<Check>,
}

#[tauri::command]
fn sync_status(state: State<AppState>) -> Res<SyncStatus> {
    let slot = state.server.lock().map_err(err)?;
    let Some((server, _)) = slot.as_ref() else {
        return Ok(SyncStatus { running: false, log: vec![] });
    };
    let log = server.log.lock().map_err(err)?;
    Ok(SyncStatus {
        running: true,
        log: log
            .iter()
            .map(|r| match r {
                Ok(s) => check(true, format!("«{}»: отправлено {}, получено {}", s.peer_name, s.sent, s.received)),
                Err(e) => check(false, format!("Не удалось: {e}")),
            })
            .collect(),
    })
}

#[tauri::command]
fn sync_stop(state: State<AppState>) -> Res<()> {
    if let Some((server, _)) = state.server.lock().map_err(err)?.take() {
        server.stop();
    }
    Ok(())
}

#[derive(Serialize)]
struct SyncResult {
    peer_name: String,
    sent: usize,
    received: usize,
}

/// Подключается к устройству по QR-коду (или вставленной ссылке) и обменивается данными.
#[tauri::command]
async fn sync_connect(state: State<'_, AppState>, uri: String) -> Res<SyncResult> {
    drop(state.store()?);
    let p = Pairing::from_uri(&uri).map_err(err)?;
    if p.device == state.me.id {
        return Err("Это QR-код этого же устройства. Отсканируйте его другим устройством".into());
    }
    let store = state.store.clone();
    let name = state.me.name.clone();
    let report = tauri::async_runtime::spawn_blocking(move || sync::sync_with(&p, &store, &name))
        .await
        .map_err(err)?
        .map_err(|e| format!("Не удалось связаться: {e}. Устройства должны быть в одной сети Wi-Fi"))?;
    Ok(SyncResult { peer_name: report.peer_name, sent: report.sent, received: report.received })
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_barcode_scanner::init()).plugin(tauri_plugin_biometric::init());
    }
    builder
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let me = load_identity(&dir)?;
            let db_path = dir.join("karman.db");
            let (mut store, notice) = open_store(&dir, &db_path, &me.id)?;
            cleanup_stage0(&mut store)?;
            book::ensure_defaults(&mut store)?;
            let guard = Guard::new(guard::has_pin(&store));
            app.manage(AppState {
                store: Arc::new(Mutex::new(store)),
                me,
                data_dir: dir,
                db_path,
                server: Mutex::new(None),
                guard: Mutex::new(guard),
                notice: Mutex::new(notice),
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                guard::on_focus(window.app_handle(), *focused);
            }
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
            device_info,
            db_check,
            calc_check,
            sync_start,
            sync_status,
            sync_stop,
            sync_connect,
            receipt_parse,
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
