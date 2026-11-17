//! ИИ в «Кармане»: запросы к Claude API с ключом пользователя.
//!
//! Ключ — только на этом устройстве: он зашифрован средствами системы (DPAPI на Windows,
//! Android Keystore на телефоне) и лежит в зашифрованной базе среди настроек устройства.
//! В обмен по Wi-Fi и в резервные копии ключ не попадает.
//!
//! В Anthropic уходит только то, что человек отправил сам: листы наряда или документа кнопкой
//! «Распознать», фраза для окна ввода, вопрос в «Спросить Claude» (и данные, которые Claude
//! запросит инструментами «Кармана»), данные поездки или месяца для проверки и сводки.
//! Распознанное ничего не меняет, пока человек не проверит и не сохранит.

use crate::claude::{run_tool, Caller};
use crate::{day, err, AppState, Res};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use karman_core::ai::{self, Doc, ExpenseDraft, Findings, OrderScan, PhraseDraft, TripDraft, Usage};
use karman_core::db::Store;
use karman_core::insights::{self, Insights};
use karman_core::overtime::Rules;
use karman_core::sync::EventBody;
use karman_core::{book, tools, Date};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

const META_KEY: &str = "ai.key";
const META_WORKSPACE: &str = "ai.workspace";
const META_USAGE: &str = "ai.usage";
const META_CHECKED: &str = "ai.checked";

/// Переключатели функций ИИ на этом устройстве: (функция, ключ настройки, включена ли по умолчанию).
/// Запись из чата по умолчанию выключена.
const FEATURES: [(&str, &str, bool); 6] = [
    ("orders", "ai.orders", true),
    ("docs", "ai.docs", true),
    ("phrase", "ai.phrase", true),
    ("chat", "ai.chat", true),
    ("write", "ai.write", false),
    ("review", "ai.review", true),
];

fn feature_on(st: &Store, name: &str) -> bool {
    let Some((_, key, default)) = FEATURES.iter().find(|f| f.0 == name) else { return false };
    match st.meta(key).ok().flatten().as_deref() {
        Some("1") => true,
        Some("0") => false,
        _ => *default,
    }
}

/// Разрешена ли запись для Claude в «Спросить Claude».
pub fn chat_write_on(st: &Store) -> bool {
    feature_on(st, "write")
}

fn need(st: &Store, name: &str, what: &str) -> Res<()> {
    if feature_on(st, name) {
        Ok(())
    } else {
        Err(format!("{what} выключено: Настройки → ИИ"))
    }
}

/// Листов в одном запросе: больше двадцати — у Claude API строже ограничения на размер снимков.
const MAX_FILES: usize = 20;
/// Всего файлов наряда в одном запросе, байт.
const MAX_BYTES: usize = 24_000_000;

/// Запросы и оценка стоимости за месяц.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct Month {
    n: u32,
    input: u64,
    output: u64,
    usd: f64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct UsageView {
    n: u32,
    usd: f64,
}

#[derive(Serialize)]
pub struct AiStatus {
    /// Ключ сохранён на этом устройстве.
    key: bool,
    /// «sk-ant-…a1B2»
    tail: String,
    workspace: String,
    /// Когда ключ последний раз проверен успешно (секунды Unix).
    checked: Option<i64>,
    /// Включённые функции: orders, docs, phrase, chat, write, review.
    features: BTreeMap<&'static str, bool>,
    /// Распознавание нарядов включено (для экрана нарядов).
    orders: bool,
    /// Модель для документов.
    model: &'static str,
    /// Модель для вопросов, проверки отчёта и сводки.
    chat_model: &'static str,
    month: UsageView,
    total: UsageView,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Ключ из базы: расшифровывается хранилищем системы.
fn stored_key(st: &Store) -> Res<Option<String>> {
    let Some(text) = st.meta(META_KEY).map_err(err)?.filter(|t| !t.is_empty()) else { return Ok(None) };
    let blob = B64.decode(text.as_bytes()).map_err(|_| "ключ API повреждён — введите его снова".to_string())?;
    let plain = crate::names::unprotect(&blob).map_err(|e| format!("ключ API не достать из хранилища системы: {e}"))?;
    String::from_utf8(plain).map(Some).map_err(|_| "ключ API повреждён — введите его снова".to_string())
}

fn usage_map(st: &Store) -> BTreeMap<String, Month> {
    st.meta(META_USAGE).ok().flatten().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn status(st: &Store, today: Option<&str>) -> Res<AiStatus> {
    let key = stored_key(st).ok().flatten();
    let months = usage_map(st);
    let month = today.and_then(|t| t.get(..7)).and_then(|m| months.get(m)).map(|m| UsageView { n: m.n, usd: m.usd }).unwrap_or_default();
    let total = months.values().fold(UsageView::default(), |a, m| UsageView { n: a.n + m.n, usd: a.usd + m.usd });
    Ok(AiStatus {
        key: key.is_some(),
        tail: key.as_deref().map(ai::key_tail).unwrap_or_default(),
        workspace: st.meta(META_WORKSPACE).map_err(err)?.unwrap_or_default(),
        checked: st.meta(META_CHECKED).map_err(err)?.and_then(|t| t.parse().ok()),
        features: FEATURES.iter().map(|f| (f.0, feature_on(st, f.0))).collect(),
        orders: feature_on(st, "orders"),
        model: ai::model_title(ai::MODEL_DOCS),
        chat_model: ai::model_title(ai::MODEL_CHAT),
        month,
        total,
    })
}

/// Учёт запроса — для счётчика в настройках.
fn count(st: &Store, today: &str, model: &str, u: Usage) -> f64 {
    let usd = ai::cost_usd(model, u);
    let mut months = usage_map(st);
    let m = months.entry(today.get(..7).unwrap_or("0000-00").to_string()).or_default();
    m.n += 1;
    m.input += u.input;
    m.output += u.output;
    m.usd += usd;
    if let Ok(text) = serde_json::to_string(&months) {
        let _ = st.set_meta(META_USAGE, &text);
    }
    usd
}

/// Адрес Claude API. В отладочной сборке его подменяет KARMAN_AI_URL — для проверок без сети.
fn api_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(u) = std::env::var("KARMAN_AI_URL") {
        if !u.is_empty() {
            return u;
        }
    }
    ai::API_URL.to_string()
}

fn net_error(e: ureq::Error, secs: u64) -> String {
    match e {
        ureq::Error::Timeout(_) => format!("Claude не ответил за {secs} секунд — попробуйте ещё раз"),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => "Нет связи с Claude API — проверьте интернет".into(),
        ureq::Error::Io(e) => format!("Нет связи с Claude API: {e}"),
        e => format!("Нет связи с Claude API: {e}"),
    }
}

/// Запрос к Claude API: ответ в JSON или понятная ошибка.
fn send(key: &str, workspace: &str, body: &Value, secs: u64) -> Res<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(secs))).http_status_as_error(false).build().into();
    let mut req = agent
        .post(&api_url())
        .header("Authorization", &format!("Bearer {key}"))
        .header("anthropic-version", ai::API_VERSION)
        .header("content-type", "application/json")
        .header("User-Agent", "Karman");
    if !workspace.is_empty() {
        req = req.header("anthropic-workspace-id", workspace);
    }
    let mut resp = req.send(body.to_string()).map_err(|e| net_error(e, secs))?;
    let code = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(20_000_000).read_to_string().map_err(|e| net_error(e, secs))?;
    if code != 200 {
        return Err(ai::api_error(code, &text));
    }
    serde_json::from_str(&text).map_err(|_| "Claude API ответил непонятно — попробуйте ещё раз".to_string())
}

/// Ключ и пространство — без блокировки базы на время запроса.
fn credentials(state: &AppState) -> Res<(String, String)> {
    let st = state.store()?;
    let key = stored_key(&st)?.ok_or("Сначала введите ключ API Claude: Настройки → ИИ")?;
    let ws = st.meta(META_WORKSPACE).map_err(err)?.unwrap_or_default();
    Ok((key, ws))
}

#[tauri::command]
pub fn ai_status(state: State<AppState>, today: String) -> Res<AiStatus> {
    status(&*state.store()?, Some(&today))
}

/// Новый ключ: шифруется хранилищем системы. Проверка — отдельной командой.
#[tauri::command]
pub fn ai_key_set(state: State<AppState>, today: String, key: String) -> Res<AiStatus> {
    let key = key.trim().to_string();
    ai::check_key(&key)?;
    let blob = crate::names::protect(key.as_bytes()).map_err(|e| format!("ключ не сохранить в хранилище системы: {e}"))?;
    let st = state.store()?;
    st.set_meta(META_KEY, &B64.encode(blob)).map_err(err)?;
    st.set_meta(META_CHECKED, "").map_err(err)?;
    status(&st, Some(&today))
}

#[tauri::command]
pub fn ai_key_delete(state: State<AppState>, today: String) -> Res<AiStatus> {
    let st = state.store()?;
    st.set_meta(META_KEY, "").map_err(err)?;
    st.set_meta(META_CHECKED, "").map_err(err)?;
    status(&st, Some(&today))
}

#[tauri::command]
pub fn ai_workspace_set(state: State<AppState>, today: String, workspace: String) -> Res<AiStatus> {
    let ws = workspace.trim().to_string();
    ai::check_workspace(&ws)?;
    let st = state.store()?;
    st.set_meta(META_WORKSPACE, &ws).map_err(err)?;
    status(&st, Some(&today))
}

/// Включить или выключить функцию: orders, docs, phrase, chat, write, review.
#[tauri::command]
pub fn ai_set(state: State<AppState>, today: String, feature: String, on: bool) -> Res<AiStatus> {
    let (_, key, _) = FEATURES.iter().find(|f| f.0 == feature).ok_or_else(|| format!("нет функции «{feature}»"))?;
    let st = state.store()?;
    st.set_meta(key, if on { "1" } else { "0" }).map_err(err)?;
    status(&st, Some(&today))
}

/// Проверка ключа: одно слово от младшей модели — доли цента.
#[tauri::command]
pub async fn ai_check(state: State<'_, AppState>, today: String) -> Res<AiStatus> {
    let (key, ws) = credentials(&state)?;
    let resp = tauri::async_runtime::spawn_blocking(move || send(&key, &ws, &ai::check_request(), 30)).await.map_err(err)??;
    let st = state.store()?;
    count(&st, &today, ai::MODEL_SHORT, ai::usage(&resp));
    st.set_meta(META_CHECKED, &now().to_string()).map_err(err)?;
    status(&st, Some(&today))
}

/// Распознанный наряд и сколько стоил запрос.
#[derive(Serialize)]
pub struct Scan {
    #[serde(flatten)]
    scan: OrderScan,
    usd: f64,
}

/// Листы наряда → Claude → поля наряда и дни для таблицы. Ничего не сохраняет.
#[tauri::command]
pub async fn ai_order_scan(state: State<'_, AppState>, today: String, trip: String, att: Vec<String>) -> Res<Scan> {
    day(&today)?;
    let (key, ws) = credentials(&state)?;
    let (files, t, who, rules) = {
        let st = state.store()?;
        need(&st, "orders", "Распознавание нарядов")?;
        let t = st.trips().map_err(err)?.into_iter().find(|t| t.id == trip).ok_or("Поездка не найдена")?;
        let settings = st.settings().map_err(err)?;
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut size = 0usize;
        for id in att.iter().take(MAX_FILES + 1) {
            let (mime, data) = st.blob(id).map_err(err)?.ok_or("Снимок наряда ещё не пришёл с другого устройства — дождитесь обмена")?;
            if !ai::doc_supported(&mime) {
                continue;
            }
            size += data.len();
            files.push((mime, data));
        }
        if files.is_empty() {
            return Err("Добавьте снимок или PDF наряда — распознавать нечего".into());
        }
        if files.len() > MAX_FILES {
            return Err(format!("Не больше {MAX_FILES} листов за раз"));
        }
        if size > MAX_BYTES {
            return Err("Файлы наряда слишком большие для одного запроса — уберите лишние листы".into());
        }
        (files, t, settings.get("ot.name").cloned().unwrap_or_default(), Rules::from_settings(&settings))
    };
    let body = {
        let docs: Vec<Doc> = files.iter().map(|(m, d)| Doc { mime: m, data: d }).collect();
        ai::order_request(&docs, t.start, t.end)
    };
    drop(files);
    let resp = tauri::async_runtime::spawn_blocking(move || send(&key, &ws, &body, 240)).await.map_err(err)??;
    // Запрос оплачен, даже если ответ не разобрался, — учитываем сразу.
    let usd = count(&*state.store()?, &today, ai::MODEL_DOCS, ai::usage(&resp));
    let raw = ai::parse_order(&ai::reply_text(&resp)?)?;
    let scan = ai::plan(&raw, &who, t.start, t.end, &rules);
    if scan.not_order {
        return Err("Claude не нашёл на снимках наряда-допуска".into());
    }
    Ok(Scan { scan, usd })
}

/// Файлы документа из базы: только снимки и PDF, не больше 20 листов и 24 МБ за раз.
fn load_files(st: &Store, att: &[String], what: &str) -> Res<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    let mut size = 0usize;
    for id in att.iter().take(MAX_FILES + 1) {
        let (mime, data) = st.blob(id).map_err(err)?.ok_or_else(|| format!("Снимок {what} ещё не пришёл с другого устройства — дождитесь обмена"))?;
        if !ai::doc_supported(&mime) {
            continue;
        }
        size += data.len();
        files.push((mime, data));
    }
    if files.is_empty() {
        return Err(format!("Добавьте снимок или PDF {what} — распознавать нечего"));
    }
    if files.len() > MAX_FILES {
        return Err(format!("Не больше {MAX_FILES} листов за раз"));
    }
    if size > MAX_BYTES {
        return Err(format!("Файлы {what} слишком большие для одного запроса — уберите лишние листы"));
    }
    Ok(files)
}

fn docs_of(files: &[(String, Vec<u8>)]) -> Vec<Doc<'_>> {
    files.iter().map(|(m, d)| Doc { mime: m, data: d }).collect()
}

/// Один запрос с ответом по схеме: учтён в счётчике, даже если ответ потом не разобрался.
async fn ask(state: &AppState, today: &str, model: &'static str, body: Value, secs: u64) -> Res<(String, f64)> {
    let (key, ws) = credentials(state)?;
    let resp = tauri::async_runtime::spawn_blocking(move || send(&key, &ws, &body, secs)).await.map_err(err)??;
    let usd = count(&*state.store()?, today, model, ai::usage(&resp));
    Ok((ai::reply_text(&resp)?, usd))
}

#[derive(Serialize)]
pub struct Phrase {
    #[serde(flatten)]
    draft: PhraseDraft,
    usd: f64,
}

/// Фраза → черновик окна ввода (Claude Haiku 5.5). Ничего не записывает: сохраняет человек.
#[tauri::command]
pub async fn ai_phrase(state: State<'_, AppState>, today: String, text: String) -> Res<Phrase> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Напишите, что записать: например «такси 640 до объекта, чек есть»".into());
    }
    if text.chars().count() > 500 {
        return Err("Слишком длинная фраза — одна операция за раз".into());
    }
    let d = day(&today)?;
    let (body, snap) = {
        let st = state.store()?;
        need(&st, "phrase", "Ввод фразой")?;
        let snap = book::snapshot(&st, d).map_err(err)?;
        let mut ctx = tools::read(&st, d, "categories", &json!({}))?;
        let trips: Vec<_> = snap.trips.iter().map(|t| t.trip.clone()).collect();
        if let Some(t) = tools::active_trip(&trips, d) {
            ctx["active_trip"] = json!(t.id);
        }
        (ai::phrase_request(&text, d, &ctx), snap)
    };
    let (reply, usd) = ask(&state, &today, ai::MODEL_SHORT, body, 60).await?;
    Ok(Phrase { draft: ai::phrase_draft(&ai::parse_phrase(&reply)?, &snap), usd })
}

#[derive(Serialize)]
pub struct ExpenseScan {
    #[serde(flatten)]
    draft: ExpenseDraft,
    usd: f64,
}

/// Билет, счёт гостиницы или чек → сумма, дата, статья и комментарий (Claude Opus 5.5).
#[tauri::command]
pub async fn ai_doc_scan(state: State<'_, AppState>, today: String, att: Vec<String>, trip: Option<String>) -> Res<ExpenseScan> {
    let d = day(&today)?;
    let body = {
        let st = state.store()?;
        need(&st, "docs", "Распознавание документов")?;
        let files = load_files(&st, &att, "документа")?;
        let dates = trip.and_then(|id| st.trips().ok()?.into_iter().find(|t| t.id == id)).map(|t| (t.start, t.end));
        ai::expense_request(&docs_of(&files), d, dates)
    };
    let (reply, usd) = ask(&state, &today, ai::MODEL_DOCS, body, 240).await?;
    let draft = ai::expense_draft(&ai::parse_expense(&reply)?);
    if draft.not_document {
        return Err("Claude не нашёл на снимке билета, счёта или чека".into());
    }
    Ok(ExpenseScan { draft, usd })
}

#[derive(Serialize)]
pub struct TripScan {
    #[serde(flatten)]
    draft: TripDraft,
    usd: f64,
}

/// Приказ или заявка на командировку → поля новой поездки (Claude Opus 5.5).
#[tauri::command]
pub async fn ai_trip_scan(state: State<'_, AppState>, today: String, att: Vec<String>) -> Res<TripScan> {
    let d = day(&today)?;
    let body = {
        let st = state.store()?;
        need(&st, "docs", "Распознавание документов")?;
        let files = load_files(&st, &att, "документа")?;
        ai::trip_request(&docs_of(&files), d)
    };
    let (reply, usd) = ask(&state, &today, ai::MODEL_DOCS, body, 240).await?;
    let draft = ai::trip_draft(&ai::parse_trip_doc(&reply)?);
    if draft.not_document {
        return Err("Claude не нашёл на снимке приказа или заявки на командировку".into());
    }
    Ok(TripScan { draft, usd })
}

// ——— «Спросить Claude» ———

#[derive(Clone, Serialize)]
pub struct ChatItem {
    /// user — человек, assistant — Claude.
    role: &'static str,
    text: String,
    /// Что Claude записал за этот ответ — каждое отменяется «Вернуть».
    actions: Vec<String>,
    error: bool,
    /// Удаления, которые ждут кнопки человека.
    confirm: Vec<Confirm>,
}

/// Удаление, которое предложил Claude: без кнопки человека не выполняется.
#[derive(Clone, Serialize)]
pub struct Confirm {
    id: u64,
    /// «Удалить операцию: Такси до объекта, 640 ₽».
    text: String,
    /// «Удалить» или «Очистить».
    button: &'static str,
    /// wait — ждёт; done — сделано; kept — человек не разрешил; failed — не получилось.
    state: &'static str,
}

struct Pending {
    id: u64,
    tool: String,
    args: Value,
    text: String,
}

/// Разговор живёт в памяти, пока открыт «Карман»: история сообщений для Claude API как есть
/// (с блоками рассуждения) и то, что видно на экране.
struct Chat {
    system: String,
    messages: Vec<Value>,
    items: Vec<ChatItem>,
    usd: f64,
    /// Удаления, которые ждут кнопки человека.
    pending: Vec<Pending>,
    next: u64,
    /// Что решил человек после ответа Claude — уйдёт вместе со следующим вопросом.
    notes: Vec<String>,
}

const fn empty_chat() -> Chat {
    Chat { system: String::new(), messages: Vec::new(), items: Vec::new(), usd: 0.0, pending: Vec::new(), next: 0, notes: Vec::new() }
}

static CHAT: Mutex<Chat> = Mutex::new(empty_chat());

/// Удаление записей Claude в «Спросить Claude» ждёт кнопки человека (выбор в 0.13.1): что удалится
/// и надпись кнопки. `None` — не удаление (или инструмент сам ответит ошибкой).
fn deletion(st: &Store, today: Date, tool: &str, args: &Value) -> Option<(String, &'static str)> {
    let change = tools::plan(st, today, tool, args, &|cur, date| crate::fx::stored_rate(st, cur, &date.to_string())).ok()?;
    let op = change.bodies.iter().any(|b| matches!(b, EventBody::OpDelete { .. }));
    let day = change.bodies.iter().any(|b| matches!(b, EventBody::DayDelete { .. }));
    if !op && !day {
        return None;
    }
    let text = if let Some(rest) = change.text.strip_prefix("Удалено: ") {
        format!("Удалить операцию: {rest}")
    } else if let Some(rest) = change.text.strip_prefix("Очищен день ") {
        format!("Очистить день {rest}")
    } else {
        change.text
    };
    Some((text, if op { "Удалить" } else { "Очистить" }))
}

/// Шагов с инструментами на один вопрос — не больше.
const MAX_STEPS: usize = 8;

#[derive(Serialize)]
pub struct ChatView {
    items: Vec<ChatItem>,
    /// Сколько стоил разговор — оценка.
    usd: f64,
    /// Разрешена ли запись.
    write: bool,
}

fn chat_view(chat: &Chat, write: bool) -> ChatView {
    ChatView { items: chat.items.clone(), usd: chat.usd, write }
}

fn chat_turn(app: &AppHandle, today: &str, text: &str) -> Res<ChatView> {
    let state = app.state::<AppState>();
    let d = day(today)?;
    let (key, ws) = {
        let st = state.store()?;
        need(&st, "chat", "«Спросить Claude»")?;
        let key = stored_key(&st)?.ok_or("Сначала введите ключ API Claude: Настройки → ИИ")?;
        (key, st.meta(META_WORKSPACE).map_err(err)?.unwrap_or_default())
    };
    let mut guard = CHAT.lock().map_err(err)?;
    let chat = &mut *guard;
    if chat.messages.is_empty() {
        chat.system = ai::chat_system(d);
    }
    let start = chat.messages.len();
    // Что человек решил по удалениям после прошлого ответа — Claude узнаёт об этом с вопросом.
    let content = if chat.notes.is_empty() { text.to_string() } else { format!("{}\n\n{text}", std::mem::take(&mut chat.notes).join("\n")) };
    chat.messages.push(json!({ "role": "user", "content": content }));
    chat.items.push(ChatItem { role: "user", text: text.to_string(), actions: Vec::new(), error: false, confirm: Vec::new() });
    let mut actions = Vec::new();
    let mut confirm = Vec::new();
    let mut outcome: Res<String> = Err(format!("Claude не уложился в {MAX_STEPS} шагов — спросите проще или по частям"));
    for _ in 0..MAX_STEPS {
        let body = ai::chat_request(&chat.system, &chat.messages);
        let resp = match send(&key, &ws, &body, 120) {
            Ok(r) => r,
            Err(e) => {
                outcome = Err(e);
                break;
            }
        };
        chat.usd += count(&*state.store()?, today, ai::MODEL_CHAT, ai::usage(&resp));
        let calls = ai::tool_calls(&resp);
        chat.messages.push(json!({ "role": "assistant", "content": resp["content"].clone() }));
        if calls.is_empty() {
            outcome = match resp["stop_reason"].as_str() {
                Some("refusal") => Err("Claude отказался отвечать на этот вопрос".into()),
                _ => Ok(ai::answer_text(&resp)),
            };
            break;
        }
        let mut results: Vec<Value> = Vec::new();
        for (id, name, input) in &calls {
            // Удалять Claude может только по кнопке человека: пока — предложение под ответом.
            let ask = if tools::is_write(name) {
                let st = state.store()?;
                if chat_write_on(&st) {
                    deletion(&st, d, name, input)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some((what, button)) = ask {
                chat.next += 1;
                let pid = chat.next;
                chat.pending.push(Pending { id: pid, tool: name.clone(), args: input.clone(), text: what.clone() });
                confirm.push(Confirm { id: pid, text: what.clone(), button, state: "wait" });
                let note = format!("{what} — ждёт подтверждения: под твоим ответом у человека кнопка «{button}». Скажи коротко, что нужно нажать; повторно не удаляй.");
                results.push(json!({ "type": "tool_result", "tool_use_id": id, "content": json!({ "pending": note }).to_string() }));
                continue;
            }
            results.push(match run_tool(app, Caller::Chat, name, input) {
                Ok(v) => {
                    if tools::is_write(name) {
                        if let Some(t) = v["done"].as_str() {
                            actions.push(t.to_string());
                        }
                    }
                    json!({ "type": "tool_result", "tool_use_id": id, "content": v.to_string() })
                }
                Err(e) => json!({ "type": "tool_result", "tool_use_id": id, "content": e, "is_error": true }),
            });
        }
        chat.messages.push(json!({ "role": "user", "content": results }));
    }
    match outcome {
        Ok(answer) => {
            let answer = if answer.is_empty() { "Готово.".to_string() } else { answer };
            chat.items.push(ChatItem { role: "assistant", text: answer, actions, error: false, confirm });
        }
        Err(e) => {
            // Разговор для Claude возвращается к началу вопроса; записанное остаётся — с «Вернуть».
            chat.messages.truncate(start);
            chat.items.push(ChatItem { role: "assistant", text: e, actions, error: true, confirm });
        }
    }
    let write = chat_write_on(&*state.store()?);
    Ok(chat_view(chat, write))
}

/// Вопрос Claude (Sonnet 5.5) о деньгах и поездках. Данные Claude получает только через
/// инструменты «Кармана» — те же, что у коннектора; запись — если разрешена.
#[tauri::command]
pub async fn ai_chat_send(app: AppHandle, today: String, text: String) -> Res<ChatView> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Напишите вопрос".into());
    }
    if text.chars().count() > 2000 {
        return Err("Слишком длинный вопрос".into());
    }
    tauri::async_runtime::spawn_blocking(move || chat_turn(&app, &today, &text)).await.map_err(err)?
}

#[tauri::command]
pub fn ai_chat_view(state: State<AppState>) -> Res<ChatView> {
    let write = chat_write_on(&*state.store()?);
    Ok(chat_view(&*CHAT.lock().map_err(err)?, write))
}

/// Новый разговор: прежний забывается.
#[tauri::command]
pub fn ai_chat_reset(state: State<AppState>) -> Res<ChatView> {
    let write = chat_write_on(&*state.store()?);
    let mut chat = CHAT.lock().map_err(err)?;
    *chat = empty_chat();
    Ok(chat_view(&chat, write))
}

/// Кнопка под ответом Claude: «Удалить» (`yes`) или «Оставить». Удаление — как запись Claude:
/// с «Вернуть»; если запись из чата успели выключить, не выполняется.
#[tauri::command]
pub async fn ai_chat_confirm(app: AppHandle, id: u64, yes: bool) -> Res<ChatView> {
    app.state::<AppState>().unlocked()?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut guard = CHAT.lock().map_err(err)?;
        let chat = &mut *guard;
        let pos = chat.pending.iter().position(|p| p.id == id).ok_or("Это действие уже не ждёт ответа")?;
        let p = chat.pending.remove(pos);
        let (mark, note) = if !yes {
            ("kept", format!("Человек не разрешил: {}.", p.text))
        } else {
            match run_tool(&app, Caller::Chat, &p.tool, &p.args) {
                Ok(_) => ("done", format!("Человек нажал «Удалить»: {} — сделано.", p.text)),
                Err(e) => ("failed", format!("Человек разрешил «{}», но не получилось: {e}", p.text)),
            }
        };
        for c in chat.items.iter_mut().flat_map(|i| i.confirm.iter_mut()).filter(|c| c.id == id) {
            c.state = mark;
        }
        chat.notes.push(note);
        let write = chat_write_on(&*state.store()?);
        Ok(chat_view(chat, write))
    })
    .await
    .map_err(err)?
}

// ——— Проверка отчёта и сводка месяца ———

fn rub_text(k: i64) -> String {
    karman_core::money::rub_text(k)
}

/// Что «Карман» находит сам: расходы без документов, превышения по статьям, долг, снимков нет.
fn trip_checks(tv: &book::TripView, snap: &book::Snapshot) -> Vec<String> {
    use karman_core::trip::{DocKind, OpKind};
    let mut out = Vec::new();
    for o in snap.ops.iter().filter(|o| o.trip.as_deref() == Some(tv.trip.id.as_str()) && o.kind == OpKind::Work) {
        match o.doc {
            None => out.push(format!("Без документа: {}, {} ({}) — в отчёт не войдёт, идёт из суточных", o.title, rub_text(o.amount), o.date.dotted())),
            Some(DocKind::Photo) | Some(DocKind::File) if o.att.is_empty() => out.push(format!("Чек отмечен, но снимка нет: {}, {} ({})", o.title, rub_text(o.amount), o.date.dotted())),
            _ => {}
        }
    }
    for r in tv.rows.iter().filter(|r| r.over > 0) {
        out.push(format!("Статья {} {}: расход {} больше заявки {} на {}", r.code, r.name, rub_text(r.fact), rub_text(r.plan), rub_text(r.over)));
    }
    if tv.s.debt > 0 {
        out.push(format!("Долг перед компанией: {}", rub_text(tv.s.debt)));
    }
    out.truncate(30);
    out
}

#[derive(Serialize)]
pub struct Review {
    #[serde(flatten)]
    findings: Findings,
    /// Что нашёл сам «Карман».
    checks: Vec<String>,
    usd: f64,
}

/// Проверка авансового отчёта поездки перед сдачей (Claude Sonnet 5.5).
#[tauri::command]
pub async fn ai_trip_check(state: State<'_, AppState>, today: String, trip: String) -> Res<Review> {
    let d = day(&today)?;
    let (body, checks) = {
        let st = state.store()?;
        need(&st, "review", "Проверка отчёта и сводка")?;
        let data = tools::read(&st, d, "trip", &json!({ "id": trip }))?;
        let snap = book::snapshot(&st, d).map_err(err)?;
        let tv = snap.trips.iter().find(|t| t.trip.id == trip).ok_or("Поездка не найдена")?;
        let checks = trip_checks(tv, &snap);
        (ai::trip_check_request(&data, &checks), checks)
    };
    let (reply, usd) = ask(&state, &today, ai::MODEL_CHAT, body, 120).await?;
    Ok(Review { findings: ai::parse_findings(&reply)?, checks, usd })
}

/// Сводка месяца без ИИ: траты против обычного, подписки, крупные покупки, прогноз.
#[tauri::command]
pub fn month_insights(state: State<AppState>, today: String) -> Res<Insights> {
    let st = state.store()?;
    let snap = book::snapshot(&st, day(&today)?).map_err(err)?;
    Ok(insights::insights(&snap))
}

#[derive(Serialize)]
pub struct MonthSummary {
    #[serde(flatten)]
    findings: Findings,
    usd: f64,
}

/// Сводка месяца словами: Claude (Sonnet 5.5) пересказывает цифры, посчитанные «Карманом».
#[tauri::command]
pub async fn ai_month_summary(state: State<'_, AppState>, today: String) -> Res<MonthSummary> {
    let d = day(&today)?;
    let body = {
        let st = state.store()?;
        need(&st, "review", "Проверка отчёта и сводка")?;
        let snap = book::snapshot(&st, d).map_err(err)?;
        let i = insights::insights(&snap);
        ai::month_request(&insights::facts(&i, &snap))
    };
    let (reply, usd) = ask(&state, &today, ai::MODEL_CHAT, body, 120).await?;
    Ok(MonthSummary { findings: ai::parse_findings(&reply)?, usd })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_is_counted_by_month() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&dir.path().join("t.db"), &[7u8; 32], "dev").unwrap();
        let u = Usage { input: 18_000, output: 2_500, ..Usage::default() };
        let usd = count(&st, "2026-11-10", ai::MODEL_DOCS, u);
        count(&st, "2026-11-11", ai::MODEL_SHORT, Usage { input: 20, output: 5, ..Usage::default() });
        count(&st, "2026-10-31", ai::MODEL_DOCS, u);
        let s = status(&st, Some("2026-11-20")).unwrap();
        assert!((usd - 0.122).abs() < 1e-9);
        assert_eq!((s.month.n, s.total.n), (2, 3));
        assert!((s.total.usd - (0.244 + 0.0000045)).abs() < 1e-9);
        assert!(!s.key && s.orders && s.checked.is_none());
        assert_eq!(s.features.get("write"), Some(&false), "запись из чата по умолчанию выключена");
        assert!(s.features.iter().filter(|(k, _)| **k != "write").all(|(_, v)| *v));
        st.set_meta("ai.write", "1").unwrap();
        st.set_meta("ai.docs", "0").unwrap();
        assert!(chat_write_on(&st) && !feature_on(&st, "docs") && need(&st, "docs", "Документы").unwrap_err().contains("выключено"));
    }

    #[test]
    fn key_is_stored_protected_and_shown_by_tail() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&dir.path().join("t.db"), &[7u8; 32], "dev").unwrap();
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzA1b2";
        st.set_meta(META_KEY, &B64.encode(crate::names::protect(key.as_bytes()).unwrap())).unwrap();
        assert_eq!(stored_key(&st).unwrap().as_deref(), Some(key));
        let s = status(&st, None).unwrap();
        assert!(s.key && s.tail == "sk-ant-…A1b2");
        st.set_meta(META_KEY, "").unwrap();
        assert_eq!(stored_key(&st).unwrap(), None);
    }
}
