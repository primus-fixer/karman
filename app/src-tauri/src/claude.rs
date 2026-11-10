//! Коннектор для приложения Claude на компьютере.
//!
//! Приложение Claude запускает `karman --mcp` (расширение `.mcpb`) и говорит с ним по протоколу MCP
//! через stdio. Посредник сам отвечает на `initialize` и `tools/list`, а вызовы инструментов передаёт
//! запущенному «Карману» по 127.0.0.1 — запросы из сети «Карман» не принимает. Ключ связи новый при
//! каждом запуске и лежит в папке данных; по сети он не ходит: стороны доказывают друг другу, что знают
//! его (MAC от одноразовых чисел). Программа, занявшая порт закрытого «Кармана», не получит ни ключа,
//! ни запроса. Если «Карман» закрыт, посредник его запускает (на Windows).
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// Где запущенный «Карман» ждёт посредника: порт и ключ.
const LINK_FILE: &str = "claude.json";
const META_ON: &str = "claude.on";
const META_WRITE: &str = "claude.write";
/// Запрос посредника — одна строка JSON, не больше.
const MAX_REQUEST: u64 = 1 << 20;

#[derive(Serialize, Deserialize)]
struct Link {
    port: u16,
    token: String,
    pid: u32,
    app: String,
}

/// Запрос посредника — после того как «Карман» доказал, что знает ключ связи.
#[derive(Serialize, Deserialize)]
struct Request {
    /// Доказательство посредника: MAC от одноразового числа «Кармана».
    auth: String,
    tool: String,
    #[serde(default)]
    args: Value,
}

/// Одноразовое число для проверки: 32 случайных байта.
fn nonce() -> Option<String> {
    karman_core::pairing::random_key().ok().map(|k| k.iter().map(|b| format!("{b:02x}")).collect())
}

fn valid_nonce(n: &str) -> bool {
    n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Посредник из прежней версии «Кармана» присылает ключ открытым текстом.
const OUTDATED: &str = "«Карман» обновился — перезапустите приложение Claude, чтобы обновился и коннектор";

#[derive(Serialize, Deserialize, Default)]
struct Reply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ok: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    err: Option<String>,
}

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

/// Ключ без тайминговых подсказок: сравнение всегда до конца.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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
        Err(e) if e == crate::guard::LOCKED => {
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

// ——— Приём запросов посредника (компьютер) ———

/// «Карман»: посредник прислал своё одноразовое число — отвечаем доказательством и своим числом;
/// дальше принимаем запрос, только если посредник доказал, что знает ключ. `None` — связь оборвалась.
fn accept(stream: &std::net::TcpStream, token: &str) -> Option<Result<Request, String>> {
    let mut reader = BufReader::new(stream.take(MAX_REQUEST));
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let hello: Value = match serde_json::from_str(&line) {
        Ok(v) => v,
        Err(_) => return Some(Err("неверный запрос".into())),
    };
    let Some(theirs) = hello["hello"].as_str().filter(|n| valid_nonce(n)) else {
        return Some(Err(if hello.get("token").is_some() { OUTDATED } else { "неверный запрос" }.into()));
    };
    let Some(mine) = nonce() else { return Some(Err("нет случайных чисел".into())) };
    let mut out = stream;
    writeln!(out, "{}", json!({ "proof": mcp::link_proof(token, "karman", theirs), "nonce": mine })).ok()?;
    line.clear();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    Some(match serde_json::from_str::<Request>(&line) {
        Ok(r) if same(&r.auth, &mcp::link_proof(token, "bridge", &mine)) => Ok(r),
        Ok(_) => Err("неверный ключ".into()),
        Err(_) => Err("неверный запрос".into()),
    })
}

#[cfg(desktop)]
fn serve(app: &AppHandle, stream: std::net::TcpStream, token: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let reply = match accept(&stream, token) {
        None => return,
        Some(Ok(r)) => match execute(app, &r.tool, &r.args) {
            Ok(v) => Reply { ok: Some(v), err: None },
            Err(e) => Reply { ok: None, err: Some(e) },
        },
        Some(Err(e)) => Reply { ok: None, err: Some(e) },
    };
    let mut out = &stream;
    let _ = writeln!(out, "{}", serde_json::to_string(&reply).unwrap_or_default());
}

/// При запуске на компьютере: ждём посредника на 127.0.0.1 и записываем, где нас искать.
#[cfg(desktop)]
pub fn start(app: AppHandle, data_dir: PathBuf) {
    std::thread::spawn(move || {
        let listener = match std::net::TcpListener::bind(("127.0.0.1", 0)) {
            Ok(l) => l,
            Err(e) => return eprintln!("коннектор для Claude: {e}"),
        };
        let Ok(port) = listener.local_addr().map(|a| a.port()) else { return };
        let Ok(key) = karman_core::pairing::random_key() else { return };
        let token: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let link = Link { port, token: token.clone(), pid: std::process::id(), app: env!("CARGO_PKG_VERSION").into() };
        if let Err(e) = write_link(&data_dir, &link) {
            return eprintln!("коннектор для Claude: {e}");
        }
        for stream in listener.incoming().flatten() {
            let (app, token) = (app.clone(), token.clone());
            std::thread::spawn(move || serve(&app, stream, &token));
        }
    });
}

#[cfg(desktop)]
fn write_link(dir: &Path, link: &Link) -> std::io::Result<()> {
    let path = dir.join(LINK_FILE);
    let tmp = dir.join(format!("{LINK_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(link)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)
}

// ——— Посредник: `karman --mcp` ———

/// Папка данных «Кармана» — та же, что у запущенного приложения.
#[cfg(desktop)]
fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("KARMAN_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.unwrap_or_default().join("io.github.primusfixer.karman")
}

#[cfg(desktop)]
enum CallError {
    /// «Карман» не запущен (или ещё не успел открыться).
    NotRunning,
    Failed(String),
}

#[cfg(desktop)]
fn call_app(dir: &Path, tool: &str, args: &Value) -> Result<Value, CallError> {
    let link: Link = std::fs::read(dir.join(LINK_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(CallError::NotRunning)?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], link.port));
    let stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| CallError::NotRunning)?;
    // Отчёт с нарядами собирается дольше обычного запроса.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    talk(&stream, &link.token, tool, args)
}

/// Посредник: сначала «Карман» доказывает, что знает ключ связи, потом — запрос с доказательством
/// посредника. Если на порту не «Карман» (порт занят другой программой), запрос не уходит.
#[cfg(desktop)]
fn talk(stream: &std::net::TcpStream, token: &str, tool: &str, args: &Value) -> Result<Value, CallError> {
    let mine = nonce().ok_or_else(|| CallError::Failed("нет случайных чисел".into()))?;
    let mut out = stream;
    writeln!(out, "{}", json!({ "hello": mine })).map_err(|_| CallError::NotRunning)?;
    let mut reader = BufReader::new(stream.take(64 << 20));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|_| CallError::NotRunning)?;
    let challenge: Value = serde_json::from_str(&line).map_err(|_| CallError::NotRunning)?;
    if let Some(e) = challenge["err"].as_str() {
        // «Карман» прежней версии не знает о проверке.
        return Err(CallError::Failed(format!("«Карман» не принял запрос ({e}) — перезапустите «Карман», чтобы он обновился")));
    }
    let proof = challenge["proof"].as_str().unwrap_or_default();
    let Some(theirs) = challenge["nonce"].as_str().filter(|n| valid_nonce(n)) else { return Err(CallError::NotRunning) };
    if proof.is_empty() || !same(proof, &mcp::link_proof(token, "karman", &mine)) {
        // На порту не «Карман»: ни ключа, ни запроса ему не отдаём.
        return Err(CallError::NotRunning);
    }
    let req = Request { auth: mcp::link_proof(token, "bridge", theirs), tool: tool.to_string(), args: args.clone() };
    writeln!(out, "{}", serde_json::to_string(&req).map_err(|e| CallError::Failed(e.to_string()))?).map_err(|_| CallError::NotRunning)?;
    line.clear();
    reader.read_line(&mut line).map_err(|e| CallError::Failed(format!("«Карман» не ответил: {e}")))?;
    let reply: Reply = serde_json::from_str(&line).map_err(|_| CallError::Failed("«Карман» ответил непонятно — обновите его".into()))?;
    match (reply.ok, reply.err) {
        (Some(v), _) => Ok(v),
        (None, Some(e)) => Err(CallError::Failed(e)),
        _ => Err(CallError::Failed("пустой ответ «Кармана»".into())),
    }
}

/// Запустить «Карман» отдельно от приложения Claude (чтобы он не закрылся вместе с ним).
#[cfg(desktop)]
fn launch() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut cmd = std::process::Command::new(&exe);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP);
        cmd.spawn().is_ok() || {
            // Если выйти из задания приложения Claude нельзя — обычный запуск.
            let mut plain = std::process::Command::new(&exe);
            plain.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            plain.creation_flags(CREATE_NEW_PROCESS_GROUP);
            plain.spawn().is_ok()
        }
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
        false
    }
}

#[cfg(desktop)]
fn forward(dir: &Path, tool: &str, args: &Value) -> Result<Value, String> {
    match call_app(dir, tool, args) {
        Ok(v) => return Ok(v),
        Err(CallError::Failed(e)) => return Err(e),
        Err(CallError::NotRunning) => {}
    }
    // Запуск — только на Windows: там второй запуск лишь открывает уже работающий «Карман».
    if !cfg!(windows) || !launch() {
        return Err("«Карман» не запущен: откройте его на этом компьютере и повторите запрос.".into());
    }
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(40) {
        std::thread::sleep(Duration::from_millis(400));
        match call_app(dir, tool, args) {
            Ok(v) => return Ok(v),
            Err(CallError::Failed(e)) => return Err(e),
            Err(CallError::NotRunning) => {}
        }
    }
    Err("«Карман» не открылся за 40 секунд: откройте его вручную и повторите запрос.".into())
}

/// `karman --mcp`: MCP по stdio для приложения Claude. Код выхода — 0, когда приложение Claude закрыло поток.
#[cfg(desktop)]
pub fn relay() -> i32 {
    let dir = data_dir();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return 1 };
        if line.trim().is_empty() {
            continue;
        }
        let reply = mcp::handle(&line, env!("CARGO_PKG_VERSION"), &mut |tool, args| forward(&dir, tool, args));
        if let Some(r) = reply {
            if writeln!(out, "{r}").and_then(|_| out.flush()).is_err() {
                return 1;
            }
        }
    }
    0
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
    let entry = Path::new(command).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "karman".into());
    let manifest = json!({
        "manifest_version": "0.3",
        "name": "karman",
        "display_name": "Карман",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Личные деньги и командировки из «Кармана»: сводка, операции, поездки, переработки. Запись — если она разрешена в «Кармане».",
        "long_description": "Расширение запускает «Карман» на этом компьютере в режиме коннектора. Данные не уходят в интернет, кроме того, что Claude прочитает по вашей просьбе. Включается в «Кармане»: Настройки → Claude.",
        "author": { "name": "Карман" },
        "icon": "icon.png",
        "server": {
            "type": "binary",
            "entry_point": entry,
            "mcp_config": { "command": command, "args": ["--mcp"], "env": { "KARMAN_DIR": dir } }
        },
        "tools": tools::list().iter().map(|t| json!({ "name": t.name, "description": t.title })).collect::<Vec<_>>(),
        "compatibility": { "platforms": [platform] },
        "keywords": ["finance", "business trips", "overtime"],
        "license": "Proprietary"
    });
    let files = vec![
        ("manifest.json".to_string(), serde_json::to_vec_pretty(&manifest).map_err(err)?),
        ("icon.png".to_string(), include_bytes!("../icons/icon.png").to_vec()),
    ];
    karman_core::zip::build(&files).map_err(err)
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

    #[test]
    fn bundle_is_a_zip_with_a_valid_manifest() {
        let data = bundle(r"C:\Users\Пётр\AppData\Local\Karman\karman.exe", r"C:\Users\Пётр\AppData\Roaming\io.github.primusfixer.karman").unwrap();
        assert_eq!(&data[..4], b"PK\x03\x04");
        let text = String::from_utf8_lossy(&data);
        assert!(text.contains("manifest.json") && text.contains("icon.png"));
        assert!(text.contains(r#""--mcp""#) && text.contains("karman.exe") && text.contains("KARMAN_DIR"));
        // Для проверки инструментом mcpb: KARMAN_BUNDLE_OUT=путь cargo test.
        if let Ok(p) = std::env::var("KARMAN_BUNDLE_OUT") {
            std::fs::write(p, &data).unwrap();
        }
    }

    #[test]
    fn token_compare() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd") && !same("abc", "abcd"));
    }

    /// «Карман» на 127.0.0.1: принимает одно подключение и отвечает эхом запроса.
    fn fake_karman(token: &'static str) -> (u16, std::thread::JoinHandle<Option<Result<Request, String>>>) {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (stream, _) = l.accept().unwrap();
            let got = accept(&stream, token);
            let reply = match &got {
                Some(Ok(r)) => Reply { ok: Some(json!({ "tool": r.tool, "args": r.args })), err: None },
                Some(Err(e)) => Reply { ok: None, err: Some(e.clone()) },
                None => return got,
            };
            let mut out = &stream;
            writeln!(out, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
            got
        });
        (port, h)
    }

    fn connect(port: u16) -> std::net::TcpStream {
        let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s
    }

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[cfg(desktop)]
    #[test]
    fn bridge_and_karman_prove_the_key_without_sending_it() {
        let (port, h) = fake_karman(KEY);
        let v = talk(&connect(port), KEY, "summary", &json!({ "x": 1 })).ok().unwrap();
        assert_eq!(v, json!({ "tool": "summary", "args": { "x": 1 } }));
        assert!(matches!(h.join().unwrap(), Some(Ok(_))));
        // Посредник с чужим ключом: «Карман» не выполняет запрос.
        let (port, h) = fake_karman(KEY);
        let other = "f".repeat(64);
        let r = talk(&connect(port), &other, "summary", &json!({}));
        // Доказательство «Кармана» не сходится с чужим ключом — посредник сам ничего не отправил.
        assert!(matches!(r, Err(CallError::NotRunning)));
        assert!(h.join().unwrap().is_none(), "запрос не ушёл");
    }

    #[cfg(desktop)]
    #[test]
    fn a_program_on_the_port_gets_neither_key_nor_request() {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (stream, _) = l.accept().unwrap();
            let mut seen = String::new();
            let mut reader = BufReader::new(&stream);
            reader.read_line(&mut seen).unwrap();
            // Самозванец отвечает как умеет — ключа он не знает.
            let mut out = &stream;
            writeln!(out, "{}", json!({ "proof": "00".repeat(32), "nonce": "11".repeat(32) })).unwrap();
            let mut rest = String::new();
            let _ = reader.read_line(&mut rest);
            seen + &rest
        });
        let r = talk(&connect(port), KEY, "add_operation", &json!({ "title": "Такси до объекта" }));
        assert!(matches!(r, Err(CallError::NotRunning)));
        let seen = h.join().unwrap();
        assert!(seen.contains("hello") && !seen.contains(KEY) && !seen.contains("Такси") && !seen.contains("auth"), "{seen}");
    }

    #[test]
    fn old_bridge_is_told_to_restart_claude() {
        let (port, h) = fake_karman(KEY);
        let s = connect(port);
        let mut out = &s;
        writeln!(out, "{}", json!({ "token": KEY, "tool": "summary", "args": {} })).unwrap();
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).unwrap();
        assert!(line.contains("перезапустите приложение Claude"), "{line}");
        assert!(matches!(h.join().unwrap(), Some(Err(e)) if e == OUTDATED));
    }
}
