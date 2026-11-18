//! Облако: Яндекс Диск или свой сервер по логину и паролю (WebDAV); Яндекс Диск или Google Drive
//! по коду, если в сборке есть регистрация приложения. Пароль копий и обмен в фоне.
//!
//! С 0.16 вход по логину и паролю, пароль копий, обмен в фоне и команды экрана — общие, из kit
//! (`kit_tauri::cloud`), те же, что у «Полки». Своё у «Кармана» — формат облака
//! (`karman_core::cloud::FORMAT`), база (пароль сервера или вход по коду — в `cloud.tokens`)
//! и вход по коду ([`ByCode`]): его команды — здесь.
//!
//! Пока «Карман» заблокирован, обмен продолжается — как и автокопии; экран данных не показывает.

use crate::{err, AppState, Res};
use karman_cloud::{Auth, Client, DeviceCode, Endpoints, Poll, Provider, Session, Tokens};
use karman_core::cloud::{Cloud, Dir, Entry, KeyFile, FORMAT};
use karman_core::db::Store;
use kit_tauri::cloud::{Conn, Extra, ProviderView, Status};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

kit_tauri::cloud_commands!(Store);

/// Облако в приложении — состояние для `app.manage`.
pub type Sky = kit_tauri::cloud::Sky<Store>;

/// Где что лежит у «Кармана». Менять нельзя: вход в облако на устройствах пропадёт.
pub fn settings() -> kit_tauri::cloud::Settings {
    kit_tauri::cloud::Settings { cloud: &FORMAT, backup: &karman_core::backup::BACKUP, keys: &crate::names::KEYS, secret: "cloud.tokens", backup_key: crate::backup::META_KEY }
}

/// Обмен в фоне.
pub fn start(app: AppHandle) {
    kit_tauri::cloud::start::<_, Store>(app);
}

/// Вернулись в «Карман»: обменяться, если давно не обменивались.
pub fn resumed() {
    kit_tauri::cloud::resumed();
}

/// Ключ облака на этом устройстве, если облако подключено.
pub fn key_meta(st: &Store) -> Option<String> {
    kit_tauri::cloud::key_meta(st)
}

/// Пароль копий сменили: тот же ключ облака под новым паролем (около секунды).
pub fn rewrap(key_meta: &str, password: &str) -> Res<KeyFile> {
    kit_tauri::cloud::rewrap(&FORMAT, key_meta, password)
}

/// Новый ключ облака уйдёт в облако при ближайшем обмене.
pub fn save_keyfile(st: &Store, kf: &KeyFile) -> Res<()> {
    kit_tauri::cloud::save_keyfile(st, kf)
}

// ——— Вход по коду ———

/// Вход по коду, который ждёт ввода.
struct Login {
    provider: Provider,
    code: DeviceCode,
    deadline: Instant,
    next: Instant,
    interval: u64,
}

static LOGIN: Mutex<Option<Login>> = Mutex::new(None);

fn login() -> MutexGuard<'static, Option<Login>> {
    LOGIN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Номер и секрет приложения у Яндекса или Google — из секретов сборки.
fn client(p: Provider) -> Option<Client> {
    let (id, secret) = match p {
        Provider::Yandex => (option_env!("KARMAN_YANDEX_CLIENT_ID"), option_env!("KARMAN_YANDEX_CLIENT_SECRET")),
        Provider::Google => (option_env!("KARMAN_GOOGLE_CLIENT_ID"), option_env!("KARMAN_GOOGLE_CLIENT_SECRET")),
    };
    let (id, secret) = (id.unwrap_or("").trim(), secret.unwrap_or("").trim());
    if !id.is_empty() && !secret.is_empty() {
        return Some(Client { id: id.into(), secret: secret.into() });
    }
    #[cfg(debug_assertions)]
    if kit_tauri::cloud::mock_base(karman_core::NAMES).is_some() {
        return Some(Client { id: "mock-client".into(), secret: "mock-secret".into() });
    }
    None
}

fn endpoints(p: Provider) -> Endpoints {
    #[cfg(debug_assertions)]
    if let Some(base) = kit_tauri::cloud::mock_base(karman_core::NAMES) {
        return Endpoints::mock(p, &base);
    }
    Endpoints::real(p)
}

fn auth_for(p: Provider) -> Res<Auth> {
    let c = client(p).ok_or_else(|| format!("{} не подключается в этой сборке «Кармана»: нет регистрации приложения", p.title()))?;
    Ok(Auth::new(p, c, endpoints(p)))
}

/// Страницу входа открываем, только если она у Яндекса или Google.
fn safe_page(url: &str) -> bool {
    const PAGES: [&str; 6] = ["https://oauth.yandex.ru/", "https://id.yandex.ru/", "https://ya.ru/", "https://yandex.ru/", "https://www.google.com/", "https://google.com/"];
    PAGES.iter().any(|p| url.starts_with(p)) && !url.chars().any(|c| c.is_whitespace() || c == '"')
}

fn open_page(app: &AppHandle, url: &str) {
    if !safe_page(url) {
        return;
    }
    use tauri_plugin_opener::OpenerExt;
    if let Err(e) = app.opener().open_url(url, None::<&str>) {
        eprintln!("страница входа не открылась: {e}");
    }
}

/// Облако по коду на один обмен: вход по коду обновляет ключи доступа — их сохраняем после обмена.
struct CodeConn(Session);

impl Cloud for CodeConn {
    fn list(&self, dir: Dir) -> kit_core::Result<Vec<Entry>> {
        self.0.list(dir)
    }
    fn get(&self, dir: Dir, name: &str, max: usize) -> kit_core::Result<Option<Vec<u8>>> {
        self.0.get(dir, name, max)
    }
    fn put(&self, dir: Dir, name: &str, data: &[u8]) -> kit_core::Result<()> {
        self.0.put(dir, name, data)
    }
    fn exists(&self, dir: Dir, name: &str) -> kit_core::Result<bool> {
        self.0.exists(dir, name)
    }
    fn delete(&self, dir: Dir, name: &str) -> kit_core::Result<()> {
        self.0.delete(dir, name)
    }
}

impl Conn for CodeConn {
    fn renewed(&self) -> Option<String> {
        self.0.tokens_changed().then(|| serde_json::to_string(&self.0.tokens()).unwrap_or_default())
    }
}

/// Вход по коду: Яндекс Диск («Приложения/Карман») или Google Drive — для общего облака из kit.
pub struct ByCode;

impl Extra for ByCode {
    fn providers(&self) -> Vec<ProviderView> {
        [Provider::Yandex, Provider::Google]
            .into_iter()
            .filter(|p| client(*p).is_some())
            .map(|p| ProviderView { code: p.code(), title: if p == Provider::Yandex { "Яндекс Диск по коду" } else { "Google Drive" }, kind: "code" })
            .collect()
    }

    fn title(&self, link: &str) -> Option<&'static str> {
        Provider::parse(link).map(Provider::title)
    }

    fn open(&self, link: &str, secret: &str) -> Option<Res<Box<dyn Conn>>> {
        let p = Provider::parse(link)?;
        Some((|| {
            let tokens: Tokens = serde_json::from_str(secret).map_err(|_| "Сначала войдите в облако".to_string())?;
            Ok(Box::new(CodeConn(Session::new(auth_for(p)?, tokens))) as Box<dyn Conn>)
        })())
    }

    fn login(&self) -> Option<serde_json::Value> {
        login().as_ref().map(|l| {
            serde_json::json!({
                "provider": l.provider.code(),
                "title": l.provider.title(),
                "user_code": l.code.user_code,
                "url": l.code.url,
                "seconds_left": l.deadline.saturating_duration_since(Instant::now()).as_secs(),
            })
        })
    }
}

fn status(app: &AppHandle) -> Res<Status> {
    kit_tauri::cloud::status::<_, Store>(app, None)
}

/// Начинает вход по коду и открывает страницу Яндекса или Google.
#[tauri::command]
pub async fn cloud_login(app: AppHandle, provider: String) -> Res<Status> {
    let p = Provider::parse(&provider).ok_or("Неизвестное облако")?;
    let (device_id, device_name) = {
        let state = app.state::<AppState>();
        state.unlocked()?;
        (format!("karman-{}", state.me.id), state.me.name.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        let auth = auth_for(p)?;
        let code = auth.start(&device_id, &device_name).map_err(err)?;
        let start = Instant::now();
        open_page(&app, &code.url);
        *login() = Some(Login { provider: p, deadline: start + Duration::from_secs(code.expires_in), next: start + Duration::from_secs(code.interval), interval: code.interval, code });
        status(&app)
    })
    .await
    .map_err(err)?
}

/// Ещё раз открыть страницу, где вводят код.
#[tauri::command]
pub fn cloud_open_page(app: AppHandle) -> Res<()> {
    app.state::<AppState>().unlocked()?;
    let url = login().as_ref().map(|l| l.code.url.clone());
    if let Some(url) = url {
        open_page(&app, &url);
    }
    Ok(())
}

#[tauri::command]
pub fn cloud_login_cancel(app: AppHandle) -> Res<Status> {
    *login() = None;
    status(&app)
}

/// Ввели ли код: экран спрашивает каждые пару секунд, пока виден код.
#[tauri::command]
pub async fn cloud_login_check(app: AppHandle) -> Res<Status> {
    app.state::<AppState>().unlocked()?;
    tauri::async_runtime::spawn_blocking(move || check_login(&app)).await.map_err(err)?
}

fn check_login(app: &AppHandle) -> Res<Status> {
    let due = {
        let mut slot = login();
        match slot.as_mut() {
            None => None,
            Some(l) if Instant::now() >= l.deadline => {
                *slot = None;
                return Err("Код устарел — начните вход заново".into());
            }
            Some(l) if Instant::now() < l.next => None,
            Some(l) => {
                l.next = Instant::now() + Duration::from_secs(l.interval);
                Some((l.provider, l.code.clone()))
            }
        }
    };
    if let Some((p, code)) = due {
        let auth = auth_for(p)?;
        match auth.poll(&code) {
            Ok(Poll::Pending) => {}
            Ok(Poll::SlowDown) => {
                if let Some(l) = login().as_mut() {
                    l.interval += 5;
                    l.next = Instant::now() + Duration::from_secs(l.interval);
                }
            }
            Ok(Poll::Done(tokens)) => {
                *login() = None;
                // Есть ли в облаке данные «Кармана»: от этого зависит, вводить пароль или придумывать.
                let session = Session::new(auth, tokens);
                let secret = serde_json::to_string(&session.tokens()).map_err(err)?;
                kit_tauri::cloud::connected::<_, Store>(app, p.code(), &secret, || session.exists(Dir::Root, FORMAT.key_file).map_err(err))?;
            }
            Err(e) => {
                *login() = None;
                return Err(e.to_string());
            }
        }
    }
    status(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_yandex_and_google_pages_open() {
        assert!(safe_page("https://oauth.yandex.ru/device"));
        assert!(safe_page("https://www.google.com/device"));
        assert!(!safe_page("http://oauth.yandex.ru/device"));
        assert!(!safe_page("https://oauth.yandex.ru.evil.example/"));
        assert!(!safe_page("https://evil.example/?https://oauth.yandex.ru/"));
        assert!(!safe_page("file:///etc/passwd"));
    }
}
