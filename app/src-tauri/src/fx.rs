//! Курсы ЦБ РФ на дату операции. С сайта ЦБ запрашивается только дата —
//! данные «Кармана» никуда не уходят.
//!
//! Если cbr.ru не открылся (нет связи или его сертификат не из списка доверенных),
//! те же курсы берутся с зеркала cbr-xml-daily.ru — туда тоже уходит только дата.

use crate::{err, AppState, Res};
use karman_core::book::{fx_rates, meta_cbr_on, CbrRates, META_CBR};
use karman_core::cbr::{parse_json, parse_xml};
use karman_core::Date;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const URL: &str = "https://www.cbr.ru/scripts/XML_daily.asp?date_req=";
const MIRROR: &str = "https://www.cbr-xml-daily.ru/archive/";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(12))).http_status_as_error(false).build().into()
}

/// Ответ сервера: код и тело не больше 2 МБ.
fn get(agent: &ureq::Agent, url: &str) -> Result<(u16, Vec<u8>), String> {
    let mut resp = agent.get(url).header("User-Agent", "Karman").call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().with_config().limit(2_000_000).read_to_vec().map_err(|e| e.to_string())?;
    Ok((status, body))
}

/// Курсы на дату (ГГГГ-ММ-ДД). Если на эту дату курсов нет (выходной), ЦБ отдаёт последние.
pub fn fetch(date: &str) -> Result<CbrRates, String> {
    fetch_from(&agent(), URL, MIRROR, date)
}

fn fetch_from(agent: &ureq::Agent, url: &str, mirror: &str, date: &str) -> Result<CbrRates, String> {
    let day = Date::parse(date).ok_or("неверная дата")?;
    let (y, m, d) = day.ymd();
    let official = match get(agent, &format!("{url}{d:02}/{m:02}/{y}")) {
        Ok((200, body)) => parse_xml(&body),
        Ok((code, _)) => Err(format!("ответ {code}")),
        Err(e) => Err(e),
    };
    let first = match official {
        Ok(c) => return Ok(c),
        Err(e) => e,
    };
    // Зеркало хранит курсы по датам, когда ЦБ их устанавливал: на выходные ищем назад.
    for back in 0..7 {
        let (y, m, d) = day.add_days(-back).ymd();
        match get(agent, &format!("{mirror}{y}/{m:02}/{d:02}/daily_json.js")) {
            Ok((200, body)) => return parse_json(&body),
            Ok((404, _)) => continue,
            Ok((code, _)) => return Err(format!("курсы ЦБ не загрузились: cbr.ru — {first}, зеркало — ответ {code}")),
            Err(e) => return Err(format!("курсы ЦБ не загрузились: cbr.ru — {first}, зеркало — {e}")),
        }
    }
    Err(format!("курсы ЦБ не загрузились: cbr.ru — {first}, на зеркале нет курсов за неделю до {date}"))
}

/// Курс без запроса в интернет: сохранённый курс ЦБ на дату, последние сохранённые курсы ЦБ
/// или курс из настроек. Для записей Claude — они не ждут сайт ЦБ.
pub fn stored_rate(st: &karman_core::db::Store, cur: &str, date: &str) -> Option<f64> {
    if cur == "RUB" {
        return Some(1.0);
    }
    let settings = st.settings().ok()?;
    if settings.get("fx.auto").map(|v| v != "0").unwrap_or(true) {
        for key in [meta_cbr_on(date), META_CBR.to_string()] {
            let saved = st.meta(&key).ok().flatten().and_then(|t| serde_json::from_str::<CbrRates>(&t).ok());
            if let Some(r) = saved.and_then(|c| c.rates.get(cur).copied()) {
                return Some(r);
            }
        }
    }
    fx_rates(&settings).get(cur).copied()
}

fn auto(state: &AppState) -> bool {
    state.store.lock().ok().and_then(|s| s.settings().ok()).and_then(|s| s.get("fx.auto").cloned()).map(|v| v != "0").unwrap_or(true)
}

/// Курс валюты на дату: из сохранённых курсов ЦБ, с сайта ЦБ или ручной из настроек.
#[derive(Serialize)]
pub struct FxQuote {
    rate: f64,
    /// cbr — курс ЦБ, manual — из настроек, none — курса нет.
    source: &'static str,
    /// Дата курса ЦБ.
    date: Option<String>,
    /// Сайт ЦБ не ответил — это последние сохранённые курсы, а не курсы на эту дату.
    fallback: bool,
}

#[tauri::command]
pub async fn fx_rate(app: AppHandle, today: String, cur: String, date: String) -> Res<FxQuote> {
    if cur == "RUB" {
        return Ok(FxQuote { rate: 1.0, source: "cbr", date: None, fallback: false });
    }
    let state = app.state::<AppState>();
    // Без входа курсы не нужны и не отдаются.
    drop(state.store()?);
    if auto(&state) {
        let cached: Option<CbrRates> = state.store.lock().map_err(err)?.meta(&meta_cbr_on(&date)).map_err(err)?.and_then(|t| serde_json::from_str(&t).ok());
        let mut fallback = false;
        let rates = match cached {
            Some(c) => Some(c),
            None => {
                let d = date.clone();
                match tauri::async_runtime::spawn_blocking(move || fetch(&d)).await.map_err(err)? {
                    Ok(c) => {
                        remember(&app, &state, &today, &date, &c);
                        Some(c)
                    }
                    // Нет связи — последние сохранённые курсы ЦБ (их дата видна в подсказке).
                    Err(e) => {
                        eprintln!("{e}");
                        fallback = true;
                        state.store.lock().map_err(err)?.meta(META_CBR).map_err(err)?.and_then(|t| serde_json::from_str(&t).ok())
                    }
                }
            }
        };
        if let Some(c) = rates {
            if let Some(r) = c.rates.get(&cur) {
                return Ok(FxQuote { rate: *r, source: "cbr", date: Some(c.date), fallback });
            }
        }
    }
    let settings = state.store.lock().map_err(err)?.settings().map_err(err)?;
    Ok(match fx_rates(&settings).get(&cur) {
        Some(r) => FxQuote { rate: *r, source: "manual", date: None, fallback: false },
        None => FxQuote { rate: 0.0, source: "none", date: None, fallback: false },
    })
}

/// Сохраняет загруженные курсы. Курсы на сегодня и прошлые даты ЦБ устанавливает заранее, они уже
/// не меняются — их можно хранить; на будущую дату ЦБ отдаёт последние, если новых ещё нет, —
/// такие под этой датой не храним. Самые свежие курсы идут в пересчёт остатков на счетах в валюте.
fn remember(app: &AppHandle, state: &AppState, today: &str, date: &str, c: &CbrRates) {
    let Ok(text) = serde_json::to_string(c) else { return };
    let Ok(st) = state.store.lock() else { return };
    if date <= today || c.date == date {
        let _ = st.set_meta(&meta_cbr_on(date), &text);
    }
    let _ = st.set_meta(&meta_cbr_on(&c.date), &text);
    let latest = st.meta(META_CBR).ok().flatten().and_then(|t| serde_json::from_str::<CbrRates>(&t).ok());
    if c.date.as_str() <= today && latest.is_none_or(|l| l.date < c.date) {
        let _ = st.set_meta(META_CBR, &text);
        drop(st);
        let _ = app.emit("fx", ());
    }
}

/// При запуске: свежие курсы ЦБ для пересчёта остатков на счетах в валюте.
pub fn refresh_on_start(app: AppHandle, today: String) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if !auto(&state) {
            return;
        }
        let have = state.store.lock().ok().and_then(|s| s.meta(META_CBR).ok().flatten()).and_then(|t| serde_json::from_str::<CbrRates>(&t).ok());
        if have.is_some_and(|c| c.date >= today) {
            return;
        }
        let d = today.clone();
        match tauri::async_runtime::spawn_blocking(move || fetch(&d)).await {
            Ok(Ok(c)) => remember(&app, &state, &today, &today, &c),
            Ok(Err(e)) => eprintln!("{e}"),
            Err(e) => eprintln!("{e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Сайт ЦБ не открылся — курсы с зеркала, на выходные — за последний день, когда ЦБ их устанавливал.
    #[test]
    fn falls_back_to_mirror_and_steps_back_over_weekend() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for _ in 0..3 {
                let (s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                }
                let path = line.split(' ').nth(1).unwrap().to_string();
                let (code, body) = if path.starts_with("/cbr/") {
                    ("403 Forbidden", String::new())
                } else if path == "/m/2026/11/07/daily_json.js" {
                    ("200 OK", r#"{"Date":"2026-11-07T11:30:00+03:00","Valute":{"USD":{"Nominal":1,"Value":81.5}}}"#.to_string())
                } else {
                    ("404 Not Found", String::new())
                };
                let mut s = s;
                write!(s, "HTTP/1.1 {code}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                paths.push(path);
            }
            paths
        });
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(5))).http_status_as_error(false).proxy(None).build().into();
        let base = format!("http://127.0.0.1:{port}");
        // Воскресенье 8 ноября: на зеркале курсы за субботу 7-е.
        let c = fetch_from(&agent, &format!("{base}/cbr/?d="), &format!("{base}/m/"), "2026-11-08").unwrap();
        assert_eq!((c.date.as_str(), c.rates["USD"]), ("2026-11-07", 81.5));
        assert_eq!(server.join().unwrap(), vec!["/cbr/?d=08/11/2026", "/m/2026/11/08/daily_json.js", "/m/2026/11/07/daily_json.js"]);
    }
}
