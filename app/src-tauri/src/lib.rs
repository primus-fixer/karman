//! «Карман», этап 0: проверки на настоящих устройствах.
//!
//! Экран проверок вызывает эти команды: зашифрованная база, расчёт поездок,
//! связь устройств по QR и обмен по Wi-Fi, разбор QR-кода чека.

use karman_core::db::{is_plaintext_sqlite, Store};
use karman_core::money::{fmt2, parse as parse_amount, rub_text};
use karman_core::pairing::{random_key, Pairing};
use karman_core::receipt::parse_fiscal_qr;
use karman_core::sync::{self, Server};
use karman_core::trip::{self, Op, OpKind, DEFAULT_RATE};
use karman_core::{demo, Date};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{Manager, State};

/// Порт для обмена по Wi-Fi. Если занят, берётся любой свободный.
const SYNC_PORT: u16 = 47_500;

struct AppState {
    store: Arc<Mutex<Store>>,
    me: Identity,
    data_dir: PathBuf,
    db_path: PathBuf,
    server: Mutex<Option<(Server, Pairing)>>,
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

fn random_hex(n: usize) -> Res<String> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(err)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
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
    let id = Identity { id: random_hex(8)?, name: default_name() };
    std::fs::write(&path, serde_json::to_string_pretty(&id).map_err(err)?).map_err(err)?;
    Ok(id)
}

/// Этап 0: ключ базы лежит рядом с ней. В этапе 1 он переедет в хранилище
/// ключей системы (Android Keystore, защита данных Windows) и будет закрыт PIN-кодом.
fn load_key(dir: &Path) -> Res<[u8; 32]> {
    let path = dir.join("key.bin");
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(k) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(k);
        }
    }
    let k = random_key().map_err(err)?;
    std::fs::write(&path, k).map_err(err)?;
    Ok(k)
}

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
    data_dir: String,
    encrypted: bool,
    cipher: String,
    events: u64,
    ops: usize,
}

#[tauri::command]
fn device_info(state: State<AppState>) -> Res<DeviceInfo> {
    let st = state.store.lock().map_err(err)?;
    Ok(DeviceInfo {
        id: state.me.id.clone(),
        name: state.me.name.clone(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        data_dir: state.data_dir.display().to_string(),
        encrypted: !is_plaintext_sqlite(&state.db_path).unwrap_or(true),
        cipher: st.cipher_version().map_err(err)?,
        events: st.event_count().map_err(err)?,
        ops: st.ops().map_err(err)?.len(),
    })
}

#[derive(Serialize)]
struct DbCheck {
    rows: usize,
    write_ms: u128,
    read_ms: u128,
    checks: Vec<Check>,
}

/// Отдельная проверочная база: запись, чтение, шифрование файла, отказ без ключа.
#[tauri::command]
fn db_check(state: State<AppState>) -> Res<DbCheck> {
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
    let main_plain = is_plaintext_sqlite(&state.db_path).unwrap_or(true);
    checks.push(check(!main_plain, "Основная база приложения тоже зашифрована"));
    Ok(DbCheck { rows, write_ms, read_ms, checks })
}

#[derive(Serialize)]
struct TripRow {
    name: String,
    status: String,
    headline: String,
    per_diem: String,
    fact: String,
    over_note: String,
}

#[derive(Serialize)]
struct CalcCheck {
    rate: String,
    trips: Vec<TripRow>,
    checks: Vec<Check>,
}

/// Расчёт поездок из макета на устройстве: должен совпасть с 1С до копейки.
#[tauri::command]
fn calc_check() -> CalcCheck {
    let ops = demo::ops();
    let today = demo::today();
    let mut checks = Vec::new();
    let trips = demo::trips()
        .into_iter()
        .map(|t| {
            let rate = trip::effective_rate(&t, t.status, DEFAULT_RATE, None);
            let s = trip::settle(&t, t.status, &ops, rate, today);
            let rows = trip::report(&t, &s, &ops);
            let fact: i64 = rows.iter().map(|r| r.fact).sum();
            if t.id == "573" {
                checks.push(check(s.issued == t.plan_total() && s.issued == 12_200_000, "573: аванс равен заявке, 122 000,00"));
                let same = |ok: bool, in_1c: &str| if ok { "совпадает с 1С".to_string() } else { format!("в 1С {in_1c}") };
                checks.push(check(fact == 5_564_110, format!("573: израсходовано {} — {}", fmt2(fact), same(fact == 5_564_110, "55 641,10"))));
                checks.push(check(s.balance == 6_635_890, format!("573: вернуть {} — {}", fmt2(s.balance), same(s.balance == 6_635_890, "66 358,90"))));
                checks.push(check(rows[3].over == 70_000, "573: сверх бюджета 700,00 по суточным"));
            }
            if t.id == "552" {
                checks.push(check(
                    s.debt_due == 860_000 && s.balance == 950_000,
                    "552: траты без документов сначала из суточных, остальное — долг 8 600 ₽",
                ));
            }
            TripRow {
                name: format!("{} {}", t.id, t.city),
                status: t.status.title().into(),
                headline: trip::headline(&s),
                per_diem: trip::per_diem_line(&s),
                fact: fmt2(fact),
                over_note: trip::over_budget_note(&t, &s, &rows),
            }
        })
        .collect();
    CalcCheck { rate: rub_text(DEFAULT_RATE) + " в день", trips, checks }
}

#[derive(Serialize)]
struct OpView {
    id: String,
    date: String,
    title: String,
    amount: String,
}

fn op_views(store: &Store) -> Res<Vec<OpView>> {
    Ok(store
        .ops()
        .map_err(err)?
        .into_iter()
        .map(|o| OpView { id: o.id, date: o.date.day_month(), title: o.title, amount: format!("−{}", rub_text(o.amount)) })
        .collect())
}

#[tauri::command]
fn ops_list(state: State<AppState>) -> Res<Vec<OpView>> {
    let st = state.store.lock().map_err(err)?;
    op_views(&st)
}

#[tauri::command]
fn op_add_test(state: State<AppState>, amount: String) -> Res<Vec<OpView>> {
    let a = parse_amount(&amount).filter(|a| *a > 0).ok_or("Введите сумму, например 350")?;
    let op = Op {
        id: random_hex(8)?,
        date: Date::today_utc(),
        kind: OpKind::Personal,
        cat: "other".into(),
        title: format!("Проверка с устройства «{}»", state.me.name),
        amount: a,
        account: "personal".into(),
        trip: None,
        doc: None,
        sent: false,
        family: false,
        debt: None,
        inflow: false,
    };
    let mut st = state.store.lock().map_err(err)?;
    st.put_op(op).map_err(err)?;
    op_views(&st)
}

#[tauri::command]
fn op_delete(state: State<AppState>, id: String) -> Res<Vec<OpView>> {
    let mut st = state.store.lock().map_err(err)?;
    st.delete_op(&id).map_err(err)?;
    op_views(&st)
}

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
    ops: Vec<OpView>,
}

/// Подключается к устройству по QR-коду (или вставленной ссылке) и обменивается данными.
#[tauri::command]
async fn sync_connect(state: State<'_, AppState>, uri: String) -> Res<SyncResult> {
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
    let st = state.store.lock().map_err(err)?;
    let ops = op_views(&st)?;
    Ok(SyncResult { peer_name: report.peer_name, sent: report.sent, received: report.received, ops })
}

#[derive(Serialize)]
struct ReceiptView {
    date: String,
    time: String,
    sum: String,
    fn_number: String,
    fd: String,
    fp: String,
    kind: String,
}

#[tauri::command]
fn receipt_parse(text: String) -> Res<ReceiptView> {
    let r = parse_fiscal_qr(&text).map_err(err)?;
    Ok(ReceiptView {
        date: r.date.dotted(),
        time: r.time.clone(),
        sum: rub_text(r.sum),
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
        builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    }
    builder
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let me = load_identity(&dir)?;
            let key = load_key(&dir)?;
            let db_path = dir.join("karman.db");
            let store = Store::open(&db_path, &key, &me.id)?;
            app.manage(AppState { store: Arc::new(Mutex::new(store)), me, data_dir: dir, db_path, server: Mutex::new(None) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            device_info,
            db_check,
            calc_check,
            ops_list,
            op_add_test,
            op_delete,
            sync_start,
            sync_status,
            sync_stop,
            sync_connect,
            receipt_parse
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить «Карман»");
}
