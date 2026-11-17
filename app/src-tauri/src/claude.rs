//! Коннектор для приложения Claude на компьютере.
//!
//! Приложение Claude запускает `karman --mcp` (расширение `.mcpb`) и говорит с ним по протоколу MCP
//! через stdio. Посредник сам отвечает на `initialize` и `tools/list`, а вызовы инструментов передаёт
//! запущенному «Карману» по 127.0.0.1 — запросы из сети «Карман» не принимает. Ключ связи новый при
//! каждом запуске и лежит в папке данных; по сети он не ходит: стороны доказывают друг другу, что знают
//! его (MAC от одноразовых чисел). Программа, занявшая порт закрытого «Кармана», не получит ни ключа,
//! ни запроса. Если «Карман» закрыт, посредник его запускает (на Windows).
//! Посредник и приём запросов — общие из kit (`kit_core::link`); здесь — инструменты «Кармана».
//!
//! Чтение — когда подключение включено в настройках. Запись — только если она отдельно разрешена;
//! каждое действие Claude видно в «Кармане» и отменяется кнопкой «Вернуть».

// На телефоне коннектора нет: там остаются только команды экрана.
#![cfg_attr(not(desktop), allow(unused_imports, dead_code))]

use crate::{err, snap, AppState, Res};
use karman_core::book::Snapshot;
use karman_core::db::Store;
use karman_core::sync::EventBody;
use karman_core::{mcp, tools, Date};
use kit_core::link::{self, Connector};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

const META_ON: &str = "claude.on";
const META_WRITE: &str = "claude.write";
/// Действие Claude, которое можно отменить.
struct Action {
    id: u64,
    at: i64,
    tool: String,
    text: String,
    /// Что записал Claude — чтобы не отменить поверх более поздней правки.
    done: Vec<EventBody>,
    undo: Vec<EventBody>,
    undone: bool,
}

static ACTIONS: Mutex<Vec<Action>> = Mutex::new(Vec::new());

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Сегодня по местному времени.
fn local_today() -> Date {
    let tz = crate::notify::tz_minutes() as i64;
    Date::from_epoch_days(((unix_now() + tz * 60).div_euclid(86_400)) as i32)
}

fn meta_flag(st: &Store, key: &str) -> bool {
    st.meta(key).ok().flatten().is_some_and(|v| v == "1")
}

fn locked_text() -> String {
    "«Карман» заблокирован: окно «Кармана» открыто — войдите по PIN-коду (или Windows Hello) и повторите запрос.".into()
}

/// Кто вызывает инструмент: приложение Claude через коннектор или «Спросить Claude» в самом «Кармане».
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Connector,
    Chat,
}

/// Выполнить инструмент в запущенном «Кармане» по запросу посредника.
fn execute(app: &AppHandle, tool: &str, args: &Value) -> Result<Value, String> {
    run_tool(app, Caller::Connector, tool, args)
}

/// Инструмент «Кармана» для Claude: чтение — когда подключение (или чат) включено, запись — только
/// если она разрешена; каждое записанное действие можно отменить кнопкой «Вернуть».
pub fn run_tool(app: &AppHandle, caller: Caller, tool: &str, args: &Value) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let info = tools::find(tool).ok_or_else(|| format!("нет инструмента «{tool}»"))?;
    let store = match state.store() {
        Ok(st) => st,
        Err(e) if e == kit_tauri::guard::LOCKED => {
            #[cfg(desktop)]
            if caller == Caller::Connector {
                crate::notify::show_main(app);
            }
            return Err(locked_text());
        }
        Err(e) => return Err(e),
    };
    match caller {
        Caller::Connector => {
            if !meta_flag(&store, META_ON) {
                return Err("Подключение для Claude выключено: включите его в «Кармане» — Настройки → Claude.".into());
            }
            if info.write && !meta_flag(&store, META_WRITE) {
                return Err("Запись для Claude выключена: разрешите её в «Кармане» — Настройки → Claude → «Разрешить запись». Читать данные можно и так.".into());
            }
        }
        Caller::Chat => {
            if info.write && !crate::ai::chat_write_on(&store) {
                return Err("Запись для Claude в «Кармане» выключена: человек может разрешить её в Настройки → ИИ → «Claude может записывать». Читать данные можно и так.".into());
            }
        }
    }
    let today = local_today();
    if !info.write {
        return tools::read(&store, today, tool, args);
    }
    if tool == "overtime_report" {
        drop(store);
        let trip = args.get("trip").and_then(|v| v.as_str()).map(str::to_string);
        let month = args.get("month").and_then(|v| v.as_str()).map(str::to_string);
        if trip.is_none() && month.is_none() {
            return Err("укажите поездку (trip) или месяц (month)".into());
        }
        let done = crate::overtime::export_report(app, &state, trip, month)?;
        let text = format!("Claude собрал отчёт по переработкам: {}", done.path);
        let _ = app.emit("claude", json!({ "id": 0, "text": text }));
        return Ok(json!({ "folder": done.path, "work_order_files": done.files, "not_included": done.notes }));
    }
    let mut store = store;
    let change = tools::plan(&store, today, tool, args, &|cur, date| crate::fx::stored_rate(&store, cur, &date.to_string()))?;
    store.append_all(change.bodies.clone()).map_err(err)?;
    drop(store);
    let id = {
        let mut list = ACTIONS.lock().map_err(err)?;
        let id = list.last().map_or(1, |a| a.id + 1);
        list.push(Action { id, at: unix_now(), tool: tool.to_string(), text: change.text.clone(), done: change.bodies, undo: change.undo, undone: false });
        let extra = list.len().saturating_sub(50);
        list.drain(..extra);
        id
    };
    let _ = app.emit("claude", json!({ "id": id, "text": format!("Claude: {}", change.text) }));
    let mut result = change.result;
    if let Some(m) = result.as_object_mut() {
        m.insert("done".into(), json!(change.text));
        m.insert("undo".into(), json!("Человек может отменить это в «Кармане» кнопкой «Вернуть»"));
    }
    Ok(result)
}

/// Записанное Claude ещё на месте — никто не правил после него.
fn still_current(st: &Store, done: &[EventBody]) -> Res<bool> {
    let ops = st.ops().map_err(err)?;
    let days = st.work_days().map_err(err)?;
    let orders = st.orders().map_err(err)?;
    let trips = st.trips().map_err(err)?;
    Ok(done.iter().all(|b| match b {
        EventBody::OpPut { op } => ops.iter().any(|o| o == op),
        EventBody::OpDelete { id } => !ops.iter().any(|o| &o.id == id),
        EventBody::DayPut { day } => days.iter().any(|d| d == day),
        EventBody::DayDelete { id } => !days.iter().any(|d| &d.id == id),
        EventBody::OrderPut { order } => orders.iter().any(|o| o == order),
        EventBody::TripPut { trip } => trips.iter().any(|t| t == trip),
        _ => false,
    }))
}

// ——— Связь с посредником `karman --mcp` (компьютер) ———

/// Коннектор «Кармана» для связи посредника с запущенным «Карманом». Идентификатор — имя папки данных
/// (`identifier` в `tauri.conf.json`); его, как и строку ключа связи, менять нельзя.
pub const CONNECTOR: Connector = Connector { server: mcp::SERVER, extension: mcp::EXTENSION, names: crate::names::APP, identifier: "io.github.primusfixer.karman" };

/// При запуске на компьютере — после того как база открыта: ждём посредника на 127.0.0.1 и записываем,
/// где нас искать.
#[cfg(desktop)]
pub fn start(app: AppHandle, data_dir: PathBuf) {
    if let Err(e) = link::listen(&CONNECTOR, &data_dir, env!("CARGO_PKG_VERSION"), move |tool, args| execute(&app, tool, args)) {
        eprintln!("коннектор для Claude: {e}");
    }
}

/// `karman --mcp`: MCP по stdio для приложения Claude. Код выхода — 0, когда приложение Claude закрыло поток.
#[cfg(desktop)]
pub fn relay() -> i32 {
    link::relay(&CONNECTOR, &mcp::tools(), env!("CARGO_PKG_VERSION"))
}

// ——— Команды экрана ———

#[derive(Serialize)]
pub struct ActionView {
    id: u64,
    at: i64,
    tool: String,
    text: String,
    undone: bool,
}

#[derive(Serialize)]
pub struct ClaudeStatus {
    /// Коннектор есть только на компьютере.
    desktop: bool,
    on: bool,
    write: bool,
    /// Чем приложение Claude запускает «Карман»: путь и ключ запуска.
    command: String,
    args: Vec<String>,
    /// Папка данных — для ручной настройки.
    dir: String,
    /// Последние действия Claude, новые сверху.
    actions: Vec<ActionView>,
}

fn status(state: &AppState) -> Res<ClaudeStatus> {
    let st = state.store()?;
    let actions = ACTIONS
        .lock()
        .map_err(err)?
        .iter()
        .rev()
        .map(|a| ActionView { id: a.id, at: a.at, tool: a.tool.clone(), text: a.text.clone(), undone: a.undone })
        .collect();
    Ok(ClaudeStatus {
        desktop: cfg!(desktop),
        on: meta_flag(&st, META_ON),
        write: meta_flag(&st, META_WRITE),
        command: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        args: vec!["--mcp".into()],
        dir: state.data_dir.display().to_string(),
        actions,
    })
}

#[tauri::command]
pub fn claude_status(state: State<AppState>) -> Res<ClaudeStatus> {
    status(&state)
}

#[tauri::command]
pub fn claude_set(state: State<AppState>, on: bool, write: bool) -> Res<ClaudeStatus> {
    if !cfg!(desktop) {
        return Err("Коннектор для Claude — на компьютере".into());
    }
    {
        let st = state.store()?;
        st.set_meta(META_ON, if on { "1" } else { "0" }).map_err(err)?;
        st.set_meta(META_WRITE, if on && write { "1" } else { "0" }).map_err(err)?;
    }
    status(&state)
}

/// Отменить действие Claude, если после него это никто не менял.
#[tauri::command]
pub fn claude_undo(state: State<AppState>, today: String, id: u64) -> Res<Snapshot> {
    let mut st = state.store()?;
    let mut list = ACTIONS.lock().map_err(err)?;
    let a = list.iter_mut().find(|a| a.id == id).ok_or("Это действие уже не найти — поправьте вручную")?;
    if a.undone {
        return Err("Уже отменено".into());
    }
    if !still_current(&st, &a.done)? {
        return Err("После Claude это уже меняли — отменить нельзя, поправьте вручную".into());
    }
    st.append_all(a.undo.clone()).map_err(err)?;
    a.undone = true;
    drop(list);
    snap(&st, &today)
}

/// Расширение для приложения Claude: `manifest.json` и значок в одном файле `.mcpb`.
pub fn bundle(command: &str, dir: &str) -> Res<Vec<u8>> {
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let manifest = mcp::manifest(env!("CARGO_PKG_VERSION"), command, dir, platform);
    mcp::bundle(&manifest, include_bytes!("../icons/icon.png"))
}

/// Сохранить расширение в «Загрузки».
#[tauri::command]
pub fn claude_bundle(app: AppHandle, state: State<AppState>) -> Res<String> {
    if !cfg!(desktop) {
        return Err("Коннектор для Claude — на компьютере".into());
    }
    let s = status(&state)?;
    let data = bundle(&s.command, &s.dir)?;
    crate::save_download(&app, "Карман для Claude", "mcpb", &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Содержимое архива проверяет karman-core (mcp.rs); здесь — что «Карман» собирает его со своим значком.
    #[test]
    fn bundle_is_a_zip_with_a_valid_manifest() {
        let data = bundle(r"C:\Users\Пётр\AppData\Local\Karman\karman.exe", r"C:\Users\Пётр\AppData\Roaming\io.github.primusfixer.karman").unwrap();
        assert_eq!(&data[..4], b"PK\x03\x04");
        let text = String::from_utf8_lossy(&data);
        assert!(text.contains("manifest.json") && text.contains("icon.png"));
        assert!(data.windows(8).any(|w| w == b"\x89PNG\r\n\x1a\n"), "значок лежит в архиве как есть");
        // Для проверки инструментом mcpb: KARMAN_BUNDLE_OUT=путь cargo test.
        if let Ok(p) = std::env::var("KARMAN_BUNDLE_OUT") {
            std::fs::write(p, &data).unwrap();
        }
    }

    /// Строки связи — те же, что у «Кармана» до kit: посредник прежней версии найдёт папку данных
    /// и узнает запущенный «Карман».
    #[test]
    fn connector_is_the_same_as_before_kit() {
        assert_eq!(CONNECTOR.server.name, "karman");
        assert_eq!(CONNECTOR.server.link_personal, b"karman-link");
        assert_eq!(CONNECTOR.extension.dir_env, "KARMAN_DIR");
        assert_eq!(CONNECTOR.identifier, "io.github.primusfixer.karman");
        assert_eq!(link::LINK_FILE, "claude.json");
        assert!(!CONNECTOR.names.feminine, "«Карман» не запущен, откройте его");
    }

    /// Посредник находит «Карман» по файлу связи и получает ответ инструмента из базы.
    #[cfg(desktop)]
    #[test]
    fn relay_reads_accounts_through_the_link_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = Store::open(&dir.path().join("t.db"), &[5; 32], "dev1").unwrap();
        karman_core::book::ensure_defaults(&mut st).unwrap();
        let st = Mutex::new(st);
        link::listen(&CONNECTOR, dir.path(), "test", move |tool, args| tools::read(&st.lock().unwrap(), Date::parse("2026-11-15").unwrap(), tool, args)).unwrap();
        let v = link::forward(&CONNECTOR, dir.path(), "accounts", &json!({})).unwrap();
        assert!(v.to_string().contains("Наличные"), "{v}");
        // Файла связи нет — «Карман» не запущен.
        assert_eq!(link::forward(&CONNECTOR, &dir.path().join("нет"), "summary", &json!({})), Err("«Карман» не запущен: откройте его на этом компьютере и повторите запрос.".into()));
    }

    #[cfg(desktop)]
    #[test]
    fn relay_finds_the_data_folder() {
        std::env::set_var("KARMAN_DIR", "/tmp/karman-test-dir");
        assert_eq!(link::data_dir(&CONNECTOR), PathBuf::from("/tmp/karman-test-dir"));
        std::env::remove_var("KARMAN_DIR");
        assert!(link::data_dir(&CONNECTOR).ends_with("io.github.primusfixer.karman"));
    }
}
