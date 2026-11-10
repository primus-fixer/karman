//! ИИ в «Кармане»: запросы к Claude API с ключом пользователя.
//!
//! Ключ — только на этом устройстве: он зашифрован средствами системы (DPAPI на Windows,
//! Android Keystore на телефоне) и лежит в зашифрованной базе среди настроек устройства.
//! В обмен по Wi-Fi и в резервные копии ключ не попадает.
//!
//! В Anthropic уходит только то, что человек отправил кнопкой «Распознать»: листы наряда
//! и даты поездки. Распознанное ничего не меняет, пока человек не проверит и не сохранит.

use crate::{day, err, AppState, Res};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use karman_core::ai::{self, Doc, OrderScan, Usage};
use karman_core::db::Store;
use karman_core::overtime::Rules;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;
use tauri::State;

const META_KEY: &str = "ai.key";
const META_WORKSPACE: &str = "ai.workspace";
const META_ORDERS: &str = "ai.orders";
const META_USAGE: &str = "ai.usage";
const META_CHECKED: &str = "ai.checked";

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
    /// Распознавание нарядов включено.
    orders: bool,
    /// Модель для документов.
    model: &'static str,
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
    let plain = karman_secure::unprotect(&blob).map_err(|e| format!("ключ API не достать из хранилища системы: {e}"))?;
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
        orders: st.meta(META_ORDERS).map_err(err)?.as_deref() != Some("0"),
        model: ai::model_title(ai::MODEL_DOCS),
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
    let blob = karman_secure::protect(key.as_bytes()).map_err(|e| format!("ключ не сохранить в хранилище системы: {e}"))?;
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

#[tauri::command]
pub fn ai_set(state: State<AppState>, today: String, orders: bool) -> Res<AiStatus> {
    let st = state.store()?;
    st.set_meta(META_ORDERS, if orders { "1" } else { "0" }).map_err(err)?;
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
        if st.meta(META_ORDERS).map_err(err)?.as_deref() == Some("0") {
            return Err("Распознавание нарядов выключено: Настройки → ИИ".into());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_is_counted_by_month() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&dir.path().join("t.db"), &[7u8; 32], "dev").unwrap();
        let u = Usage { input: 18_000, output: 2_500 };
        let usd = count(&st, "2026-11-10", ai::MODEL_DOCS, u);
        count(&st, "2026-11-11", ai::MODEL_SHORT, Usage { input: 20, output: 5 });
        count(&st, "2026-10-31", ai::MODEL_DOCS, u);
        let s = status(&st, Some("2026-11-20")).unwrap();
        assert!((usd - 0.122).abs() < 1e-9);
        assert_eq!((s.month.n, s.total.n), (2, 3));
        assert!((s.total.usd - (0.244 + 0.0000045)).abs() < 1e-9);
        assert!(!s.key && s.orders && s.checked.is_none());
    }

    #[test]
    fn key_is_stored_protected_and_shown_by_tail() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&dir.path().join("t.db"), &[7u8; 32], "dev").unwrap();
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzA1b2";
        st.set_meta(META_KEY, &B64.encode(karman_secure::protect(key.as_bytes()).unwrap())).unwrap();
        assert_eq!(stored_key(&st).unwrap().as_deref(), Some(key));
        let s = status(&st, None).unwrap();
        assert!(s.key && s.tail == "sk-ant-…A1b2");
        st.set_meta(META_KEY, "").unwrap();
        assert_eq!(stored_key(&st).unwrap(), None);
    }
}
