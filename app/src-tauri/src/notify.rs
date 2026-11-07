//! Напоминания о платежах и сроках долгов.
//!
//! - **Android**: уведомления по расписанию системы — приходят и при закрытом приложении,
//!   сохраняются после перезагрузки. Перепланируются, когда меняются платежи или долги.
//! - **Компьютер**: по настройке. «При открытии» (по умолчанию) — напоминает само приложение.
//!   «Уведомлением Windows» — «Карман» запускается вместе с Windows, работает в трее и раз
//!   в минуту проверяет, не пора ли напомнить.
//!
//! В тексте только название, без сумм: уведомление видно на заблокированном экране.

use crate::{err, AppState, Res};
use karman_core::book::Snapshot;
use karman_core::finance::{self, Reminder};
use karman_core::Date;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager, State};

static APP: OnceLock<AppHandle> = OnceLock::new();
/// Смещение местного времени от UTC в минутах; приходит с экрана при каждой сводке.
static TZ: AtomicI32 = AtomicI32::new(NO_TZ);
const NO_TZ: i32 = i32::MIN;
/// Отпечаток уже запланированных уведомлений: без изменений систему не трогаем.
static PLANNED: Mutex<Option<u64>> = Mutex::new(None);
/// Режим «уведомлением Windows»: закрытие окна прячет «Карман» в трей.
pub static TRAY: AtomicBool = AtomicBool::new(false);

const META_TZ: &str = "tz.offset";
#[cfg_attr(not(mobile), allow(dead_code))]
const META_IDS: &str = "remind.ids";
const META_SHOWN: &str = "remind.shown";
/// Напоминания на компьютере: open — при открытии, windows — уведомлением Windows.
/// Хранится на устройстве, а не в общих настройках: автозапуск у каждого компьютера свой.
pub const META_DESKTOP: &str = "remind.desktop";
/// Больше уведомлений вперёд не планируем (у Android предел на будильники приложения).
#[cfg_attr(not(mobile), allow(dead_code))]
const MAX_PLANNED: usize = 60;

pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    let state = app.state::<AppState>();
    let saved = state.store.lock().ok().and_then(|s| s.meta(META_TZ).ok().flatten()).and_then(|t| t.parse::<i32>().ok());
    if let Some(tz) = saved {
        TZ.store(tz, Ordering::Relaxed);
    }
    let mode = desktop_mode(&state);
    TRAY.store(cfg!(desktop) && mode == "windows", Ordering::Relaxed);
}

/// Часовой пояс устройства (минуты от UTC). Сохраняется, чтобы напоминания работали и до открытия экрана.
pub fn set_tz(store: &karman_core::db::Store, minutes: i32) {
    if !(-14 * 60..=14 * 60).contains(&minutes) {
        return;
    }
    if TZ.swap(minutes, Ordering::Relaxed) != minutes {
        let _ = store.set_meta(META_TZ, &minutes.to_string());
        *PLANNED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Время напоминаний из настройки «remind.time» (по умолчанию 10:00).
pub fn remind_time(settings: &BTreeMap<String, String>) -> (u32, u32) {
    settings
        .get("remind.time")
        .and_then(|t| {
            let (h, m) = t.split_once(':')?;
            let (h, m) = (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?);
            (h < 24 && m < 60).then_some((h, m))
        })
        .unwrap_or((10, 0))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Unix-время напоминания в этот день по местному времени.
fn at(day: Date, (h, m): (u32, u32), tz: i32) -> i64 {
    let days = Date::from_epoch_days(0).days_until(day) as i64;
    days * 86_400 + (h * 3600 + m * 60) as i64 - tz as i64 * 60
}

/// Сегодняшняя дата по местному времени.
fn local_today(tz: i32) -> Date {
    Date::from_epoch_days(((unix_now() + tz as i64 * 60).div_euclid(86_400)) as i32)
}

/// Номер уведомления по ключу напоминания (FNV-1a), всегда положительный.
#[cfg_attr(not(mobile), allow(dead_code))]
fn key_id(key: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in key.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    (h & 0x7fff_ffff) as i32
}

fn signature(list: &[Reminder], hm: (u32, u32), tz: i32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |s: &str| {
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&format!("{}:{}:{tz}", hm.0, hm.1));
    for r in list {
        eat(&r.key);
        eat(&r.text);
    }
    h
}

/// После каждой сводки: если напоминания изменились — перепланировать уведомления телефона.
pub fn after_snapshot(s: &Snapshot) {
    if !cfg!(mobile) {
        return;
    }
    let tz = TZ.load(Ordering::Relaxed);
    if tz == NO_TZ {
        return;
    }
    let list = finance::reminders(&s.upcoming, s.today);
    let hm = remind_time(&s.settings);
    let sig = signature(&list, hm, tz);
    {
        let mut planned = PLANNED.lock().unwrap_or_else(|e| e.into_inner());
        if *planned == Some(sig) {
            return;
        }
        *planned = Some(sig);
    }
    let Some(app) = APP.get().cloned() else { return };
    // Вызовы в систему Android — не из потока окна.
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = schedule(&app, &list, hm, tz) {
            eprintln!("напоминания не запланированы: {e}");
            *PLANNED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    });
}

#[cfg(mobile)]
fn schedule(app: &AppHandle, list: &[Reminder], hm: (u32, u32), tz: i32) -> Res<()> {
    use tauri_plugin_notification::{NotificationExt, PermissionState, Schedule};
    let n = app.notification();
    if !matches!(n.permission_state(), Ok(PermissionState::Granted)) {
        return Ok(());
    }
    let state = app.state::<AppState>();
    let prev: Vec<i32> = state.store.lock().map_err(err)?.meta(META_IDS).map_err(err)?.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if !prev.is_empty() {
        let _ = n.cancel(prev);
    }
    let now = unix_now();
    let offset = time::UtcOffset::from_whole_seconds(tz * 60).unwrap_or(time::UtcOffset::UTC);
    let mut ids = Vec::new();
    for r in list {
        let t = at(r.day, hm, tz);
        if t <= now + 30 {
            continue;
        }
        if ids.len() >= MAX_PLANNED {
            break;
        }
        let Ok(date) = time::OffsetDateTime::from_unix_timestamp(t) else { continue };
        let id = key_id(&r.key);
        let res = n.builder().id(id).title("Карман").body(&r.text).schedule(Schedule::At { date: date.to_offset(offset), repeating: false, allow_while_idle: true }).show();
        match res {
            Ok(()) => ids.push(id),
            Err(e) => eprintln!("уведомление не запланировано: {e}"),
        }
    }
    let text = serde_json::to_string(&ids).map_err(err)?;
    let saved = state.store.lock().map_err(err)?.set_meta(META_IDS, &text).map_err(err);
    saved
}

#[cfg(not(mobile))]
fn schedule(_app: &AppHandle, _list: &[Reminder], _hm: (u32, u32), _tz: i32) -> Res<()> {
    Ok(())
}

fn desktop_mode(state: &AppState) -> String {
    state.store.lock().ok().and_then(|s| s.meta(META_DESKTOP).ok().flatten()).filter(|m| m == "windows").unwrap_or_else(|| "open".into())
}

/// Компьютер в режиме «уведомлением Windows»: раз в минуту проверяет, не пора ли напомнить.
/// Каждое напоминание показывается один раз, и только в свой день — вчерашние не догоняют.
pub fn start_desktop_loop(app: AppHandle) {
    if !cfg!(desktop) {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
        if !TRAY.load(Ordering::Relaxed) {
            continue;
        }
        if let Err(e) = desktop_check(&app) {
            eprintln!("напоминания: {e}");
        }
    });
}

fn desktop_check(app: &AppHandle) -> Res<()> {
    use tauri_plugin_notification::NotificationExt;
    let tz = TZ.load(Ordering::Relaxed);
    if tz == NO_TZ {
        return Ok(());
    }
    let today = local_today(tz);
    let state = app.state::<AppState>();
    let (due, hm, mut shown) = {
        // Напоминание без сумм можно показать и пока «Карман» заблокирован PIN-кодом.
        let st = state.store.lock().map_err(err)?;
        let s = karman_core::book::snapshot(&st, today).map_err(err)?;
        let shown: BTreeMap<String, String> = st.meta(META_SHOWN).map_err(err)?.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        (finance::reminders(&s.upcoming, today), remind_time(&s.settings), shown)
    };
    let now = unix_now();
    let mut changed = false;
    let ready: Vec<&Reminder> = due.iter().filter(|r| r.day == today && at(r.day, hm, tz) <= now && !shown.contains_key(&r.key)).collect();
    for r in ready {
        app.notification().builder().title("Карман").body(&r.text).show().map_err(err)?;
        shown.insert(r.key.clone(), r.day.to_string());
        changed = true;
    }
    // Старые отметки не копим.
    let week_ago = today.add_days(-7).to_string();
    let before = shown.len();
    shown.retain(|_, day| *day >= week_ago);
    if changed || shown.len() != before {
        let text = serde_json::to_string(&shown).map_err(err)?;
        state.store.lock().map_err(err)?.set_meta(META_SHOWN, &text).map_err(err)?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct RemindStatus {
    /// android, windows или other.
    platform: &'static str,
    /// Разрешение на уведомления: granted, denied, prompt; на компьютере — granted.
    permission: String,
    /// Напоминания на компьютере: open или windows.
    desktop: String,
}

fn platform() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else if cfg!(windows) {
        "windows"
    } else {
        "other"
    }
}

#[cfg(mobile)]
fn permission(app: &AppHandle, ask: bool) -> String {
    use tauri_plugin_notification::{NotificationExt, PermissionState};
    let n = app.notification();
    let st = if ask { n.request_permission() } else { n.permission_state() };
    match st {
        Ok(PermissionState::Granted) => "granted".into(),
        Ok(PermissionState::Denied) => "denied".into(),
        Ok(_) => "prompt".into(),
        Err(e) => {
            eprintln!("разрешение на уведомления: {e}");
            "prompt".into()
        }
    }
}

#[cfg(not(mobile))]
fn permission(_app: &AppHandle, _ask: bool) -> String {
    "granted".into()
}

/// Состояние напоминаний; `ask` — попросить разрешение на уведомления (Android).
#[tauri::command]
pub async fn remind_status(app: AppHandle, ask: bool) -> Res<RemindStatus> {
    drop(app.state::<AppState>().store()?);
    let a = app.clone();
    let perm = tauri::async_runtime::spawn_blocking(move || permission(&a, ask)).await.map_err(err)?;
    if perm == "granted" && ask {
        // Разрешили — следующая сводка запланирует уведомления заново.
        *PLANNED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    let state = app.state::<AppState>();
    Ok(RemindStatus { platform: platform(), permission: perm, desktop: desktop_mode(&state) })
}

/// Напоминания на компьютере: open — при открытии, windows — уведомлением Windows
/// (автозапуск вместе с Windows и значок в трее).
#[tauri::command]
pub fn remind_desktop(app: AppHandle, state: State<AppState>, mode: String) -> Res<String> {
    drop(state.store()?);
    let mode = if mode == "windows" { "windows" } else { "open" };
    if mode == "windows" && !cfg!(windows) {
        return Err("Уведомления Windows есть только на компьютере с Windows".into());
    }
    apply_desktop_mode(&app, mode == "windows")?;
    state.store.lock().map_err(err)?.set_meta(META_DESKTOP, mode).map_err(err)?;
    TRAY.store(mode == "windows", Ordering::Relaxed);
    Ok(mode.into())
}

/// Автозапуск и значок в трее для режима «уведомлением Windows».
#[cfg(windows)]
pub fn apply_desktop_mode(app: &AppHandle, on: bool) -> Res<()> {
    use tauri_plugin_autostart::ManagerExt;
    let auto = app.autolaunch();
    if on {
        auto.enable().map_err(|e| format!("не получилось включить автозапуск: {e}"))?;
        tray(app)?;
    } else {
        let _ = auto.disable();
        let _ = app.remove_tray_by_id("karman");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn apply_desktop_mode(_app: &AppHandle, _on: bool) -> Res<()> {
    Ok(())
}

/// Значок в трее: щелчок открывает «Карман», в меню — «Открыть» и «Выход».
#[cfg(windows)]
pub fn tray(app: &AppHandle) -> Res<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    if app.tray_by_id("karman").is_some() {
        return Ok(());
    }
    let open = MenuItem::with_id(app, "open", "Открыть «Карман»", true, None::<&str>).map_err(err)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>).map_err(err)?;
    let menu = Menu::with_items(app, &[&open, &quit]).map_err(err)?;
    let mut b = TrayIconBuilder::with_id("karman").tooltip("Карман").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.on_menu_event(|app, e| match e.id.as_ref() {
        "open" => show_main(app),
        "quit" => app.exit(0),
        _ => {}
    })
    .on_tray_icon_event(|tray, e| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            show_main(tray.app_handle());
        }
    })
    .build(app)
    .map_err(err)?;
    Ok(())
}

/// Показать окно «Кармана» (из трея или при повторном запуске).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_time_and_ids() {
        // 10:00 по Кызылорде (UTC+5) 10 ноября 2026 — это 05:00 UTC.
        let t = at(Date::parse("2026-11-10").unwrap(), (10, 0), 300);
        assert_eq!(t, 1_794_286_800);
        assert_eq!(time::OffsetDateTime::from_unix_timestamp(t).unwrap().hour(), 5);
        assert!(key_id("plan:p:2026-11-10:3") > 0);
        assert_ne!(key_id("plan:p:2026-11-10:3"), key_id("plan:p:2026-11-10:0"));
        let mut st = BTreeMap::new();
        assert_eq!(remind_time(&st), (10, 0));
        st.insert("remind.time".to_string(), "08:30".to_string());
        assert_eq!(remind_time(&st), (8, 30));
        st.insert("remind.time".to_string(), "25:00".to_string());
        assert_eq!(remind_time(&st), (10, 0));
    }
}
