//! Облако: Яндекс Диск или свой сервер по логину и паролю (WebDAV); Яндекс Диск или Google Drive
//! по коду, если в сборке есть регистрация приложения. Пароль копий и обмен в фоне.
//!
//! Пароль приложения Яндекса (или пароль своего сервера) хранится только на этом устройстве:
//! в зашифрованной базе и ещё под защитой системы, как ключ API Claude. В копии и на другие
//! устройства он не уходит.
//!
//! Обмен идёт сам: вскоре после запуска, через минуту после изменений, каждые 15 минут
//! и при возвращении в «Карман», если с прошлого обмена прошло больше двух минут.
//! Настройка «Только по кнопке» оставляет одну кнопку «Обменяться сейчас».
//! Пока «Карман» заблокирован, обмен продолжается — как и автокопии; экран данных не показывает.
//!
//! Пароль облака — это пароль копий: на компьютере они совпадают, смена пароля копий
//! перешифровывает ключ облака. Телефон пароль копий не хранит — его вводят один раз при подключении.

use crate::{err, AppState, Res};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use karman_cloud::dav::{self, Dav};
use karman_cloud::{Auth, Client, DeviceCode, Endpoints, Poll, Provider, Session, Tokens};
use karman_core::backup::{check_password, BackupKey, Kdf};
use karman_core::cloud::{self as kc, Cloud, CloudKey, Dir, KeyFile, Report, Round, State as CloudState, Step};
use karman_core::db::Store;
use karman_core::error::{CloudFail, Error};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// На устройстве (не синхронизируется).
const META_PROVIDER: &str = "cloud.provider";
/// Вход по коду — токены; вход по паролю — пароль под защитой системы.
const META_TOKENS: &str = "cloud.tokens";
/// Адрес сервера WebDAV и логин.
const META_DAV: &str = "cloud.dav";
const META_KEY: &str = "cloud.key";
const META_STATE: &str = "cloud.state";
const META_EVERY: &str = "cloud.every";
const META_LAST: &str = "cloud.last";
const META_ERROR: &str = "cloud.error";
/// Ключ облака под новым паролем копий: уйдёт в облако при ближайшем обмене.
const META_KEYFILE: &str = "cloud.keyfile";
/// В облаке уже есть данные «Кармана»: «1» или «0».
const META_EXISTS: &str = "cloud.exists";

const EVERY: Duration = Duration::from_secs(15 * 60);
const RESUME_GAP: Duration = Duration::from_secs(2 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static MANUAL: AtomicBool = AtomicBool::new(false);
static RESUMED: AtomicBool = AtomicBool::new(false);
static PROGRESS: Mutex<Option<Step>> = Mutex::new(None);
/// Один обмен за раз; отключение ждёт, пока идущий обмен остановится.
static ROUND: Mutex<()> = Mutex::new(());

struct Wake {
    flag: Mutex<bool>,
    cv: Condvar,
}

static WAKE: Wake = Wake { flag: Mutex::new(false), cv: Condvar::new() };

fn wake() {
    if let Ok(mut f) = WAKE.flag.lock() {
        *f = true;
    }
    WAKE.cv.notify_all();
}

fn sleep_or_wake(d: Duration) {
    let Ok(g) = WAKE.flag.lock() else {
        std::thread::sleep(d);
        return;
    };
    if let Ok((mut g, _)) = WAKE.cv.wait_timeout_while(g, d, |woken| !*woken) {
        *g = false;
    }
}

/// Обменяться при первой возможности (кнопка, подключение, новый пароль).
pub fn sync_now() {
    MANUAL.store(true, Ordering::SeqCst);
    wake();
}

/// «Карман» снова на экране.
pub fn resumed() {
    RESUMED.store(true, Ordering::SeqCst);
    wake();
}

fn now() -> i64 {
    karman_cloud::now()
}

fn meta(st: &Store, k: &str) -> Option<String> {
    st.meta(k).ok().flatten().filter(|v| !v.is_empty())
}

fn clear(st: &Store, keys: &[&str]) {
    for k in keys {
        let _ = st.set_meta(k, "");
    }
}

// ——— Как подключено облако ———

/// Способ подключения.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    /// По коду (OAuth): Яндекс Диск («Приложения/Карман») или Google Drive.
    Code(Provider),
    /// Яндекс Диск по логину и паролю приложения (WebDAV).
    YaDisk,
    /// Другой сервер WebDAV: Nextcloud, NAS.
    WebDav,
}

impl Link {
    fn code(self) -> &'static str {
        match self {
            Link::Code(p) => p.code(),
            Link::YaDisk => "yadisk",
            Link::WebDav => "webdav",
        }
    }

    fn parse(s: &str) -> Option<Link> {
        match s {
            "yadisk" => Some(Link::YaDisk),
            "webdav" => Some(Link::WebDav),
            s => Provider::parse(s).map(Link::Code),
        }
    }

    fn title(self) -> &'static str {
        match self {
            Link::Code(p) => p.title(),
            Link::YaDisk => "Яндекс Диск",
            Link::WebDav => "Сервер WebDAV",
        }
    }

    /// Надпись на кнопке подключения.
    fn button(self) -> &'static str {
        match self {
            Link::Code(Provider::Yandex) => "Яндекс Диск по коду",
            Link::Code(Provider::Google) => "Google Drive",
            Link::YaDisk => "Яндекс Диск",
            Link::WebDav => "Другой сервер WebDAV",
        }
    }
}

/// Сервер WebDAV и логин (пароль хранится отдельно).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DavLogin {
    url: String,
    user: String,
}

fn link_of(st: &Store) -> Option<Link> {
    meta(st, META_PROVIDER).and_then(|p| Link::parse(&p))
}

fn dav_of(st: &Store) -> Option<DavLogin> {
    meta(st, META_DAV).and_then(|t| serde_json::from_str(&t).ok())
}

/// Пароль WebDAV из базы: он лежит там под защитой системы.
fn dav_password(st: &Store) -> Res<Option<String>> {
    let Some(text) = meta(st, META_TOKENS) else { return Ok(None) };
    let blob = B64.decode(text.trim()).map_err(err)?;
    let plain = karman_secure::unprotect(&blob).map_err(|e| format!("пароль облака не достать из хранилища системы: {e}"))?;
    String::from_utf8(plain).map(Some).map_err(err)
}

/// Адрес Яндекс Диска по WebDAV; в сквозных проверках — имитация.
fn yadisk_url() -> String {
    #[cfg(debug_assertions)]
    if let Some(base) = mock_base() {
        return format!("{base}/dav");
    }
    dav::YANDEX.into()
}

/// Подключение по входу, сохранённому на устройстве.
enum Conn {
    Code(Session),
    Dav(Dav),
}

impl Cloud for Conn {
    fn list(&self, dir: Dir) -> karman_core::Result<Vec<kc::Entry>> {
        match self {
            Conn::Code(s) => s.list(dir),
            Conn::Dav(d) => d.list(dir),
        }
    }

    fn get(&self, dir: Dir, name: &str, max: usize) -> karman_core::Result<Option<Vec<u8>>> {
        match self {
            Conn::Code(s) => s.get(dir, name, max),
            Conn::Dav(d) => d.get(dir, name, max),
        }
    }

    fn put(&self, dir: Dir, name: &str, data: &[u8]) -> karman_core::Result<()> {
        match self {
            Conn::Code(s) => s.put(dir, name, data),
            Conn::Dav(d) => d.put(dir, name, data),
        }
    }

    fn exists(&self, dir: Dir, name: &str) -> karman_core::Result<bool> {
        match self {
            Conn::Code(s) => s.exists(dir, name),
            Conn::Dav(d) => d.exists(dir, name),
        }
    }

    fn delete(&self, dir: Dir, name: &str) -> karman_core::Result<()> {
        match self {
            Conn::Code(s) => s.delete(dir, name),
            Conn::Dav(d) => d.delete(dir, name),
        }
    }
}

impl Conn {
    /// Вход по коду обновился за обмен — сохраняем.
    fn save_tokens(&self, st: &Store) {
        if let Conn::Code(s) = self {
            if s.tokens_changed() {
                let _ = st.set_meta(META_TOKENS, &serde_json::to_string(&s.tokens()).unwrap_or_default());
            }
        }
    }
}

/// Подключение по входу этого устройства.
fn open(st: &Store) -> Res<Conn> {
    let link = link_of(st).ok_or("Сначала войдите в облако")?;
    match link {
        Link::Code(p) => Ok(Conn::Code(Session::new(auth_for(p)?, tokens_of(st).ok_or("Сначала войдите в облако")?))),
        Link::YaDisk | Link::WebDav => {
            let login = dav_of(st).ok_or("Сначала войдите в облако")?;
            let password = dav_password(st)?.ok_or("Сначала войдите в облако")?;
            Ok(Conn::Dav(Dav::new(&login.url, &login.user, &password)))
        }
    }
}

// ——— Регистрация приложения и адреса ———

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
    if mock_base().is_some() {
        return Some(Client { id: "mock-client".into(), secret: "mock-secret".into() });
    }
    None
}

/// Для сквозных проверок в отладочной сборке: имитация облаков на этом компьютере.
#[cfg(debug_assertions)]
fn mock_base() -> Option<String> {
    std::env::var("KARMAN_CLOUD_MOCK").ok().filter(|b| b.starts_with("http://127.0.0.1:"))
}

fn endpoints(p: Provider) -> Endpoints {
    #[cfg(debug_assertions)]
    if let Some(base) = mock_base() {
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

// ——— Состояние для экрана ———

/// Вход по коду, который ждёт ввода.
pub struct Login {
    provider: Provider,
    code: DeviceCode,
    deadline: Instant,
    next: Instant,
    interval: u64,
}

#[derive(Serialize)]
pub struct ProviderView {
    code: &'static str,
    title: &'static str,
    /// password — логин и пароль; code — вход по коду в браузере.
    kind: &'static str,
}

/// Вход по паролю: сервер и логин (пароль экран не получает).
#[derive(Serialize)]
pub struct DavView {
    url: String,
    host: String,
    user: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Last {
    pub at: i64,
    pub received: usize,
    pub sent: usize,
    pub files_received: usize,
    pub files_sent: usize,
    pub files_waiting: usize,
    #[serde(default)]
    pub problems: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Problem {
    pub kind: CloudFail,
    pub text: String,
    pub at: i64,
}

#[derive(Serialize)]
pub struct LoginView {
    provider: &'static str,
    title: &'static str,
    user_code: String,
    url: String,
    seconds_left: u64,
}

#[derive(Serialize)]
pub struct Status {
    desktop: bool,
    /// Чем можно подключиться: Яндекс Диск и свой сервер — всегда; по коду — если есть регистрация.
    providers: Vec<ProviderView>,
    /// off — не подключено; signin — ждём, пока введут код; password — нужен пароль копий;
    /// relogin — вход устарел; ready — подключено.
    state: &'static str,
    provider: Option<&'static str>,
    title: Option<&'static str>,
    login: Option<LoginView>,
    dav: Option<DavView>,
    /// В облаке уже есть данные «Кармана»: пароль вводят, а не придумывают.
    cloud_exists: bool,
    /// На этом устройстве задан пароль копий.
    backup_password: bool,
    /// auto — обмен сам, manual — только по кнопке.
    every: String,
    running: bool,
    progress: Option<Step>,
    last: Option<Last>,
    error: Option<Problem>,
    /// Новый пароль копий ещё не ушёл в облако.
    key_pending: bool,
    /// Сообщение после действия.
    notice: Option<String>,
}

fn every(st: &Store) -> String {
    if meta(st, META_EVERY).as_deref() == Some("manual") {
        "manual".into()
    } else {
        "auto".into()
    }
}

fn tokens_of(st: &Store) -> Option<Tokens> {
    meta(st, META_TOKENS).and_then(|t| serde_json::from_str(&t).ok())
}

fn status_of(state: &AppState, st: &Store, notice: Option<String>) -> Status {
    let provider = link_of(st);
    let login = state.cloud_login.lock().ok().and_then(|l| {
        l.as_ref().map(|l| LoginView {
            provider: l.provider.code(),
            title: l.provider.title(),
            user_code: l.code.user_code.clone(),
            url: l.code.url.clone(),
            seconds_left: l.deadline.saturating_duration_since(Instant::now()).as_secs(),
        })
    });
    let (tokens, key) = (meta(st, META_TOKENS).is_some(), meta(st, META_KEY).is_some());
    let name = match (login.is_some(), provider.is_some(), tokens, key) {
        (true, ..) => "signin",
        (_, false, ..) => "off",
        (_, true, true, true) => "ready",
        (_, true, true, false) => "password",
        (_, true, false, true) => "relogin",
        (_, true, false, false) => "off",
    };
    let code = [Provider::Yandex, Provider::Google].into_iter().filter(|p| client(*p).is_some()).map(Link::Code);
    Status {
        desktop: cfg!(desktop),
        providers: [Link::YaDisk, Link::WebDav].into_iter().chain(code).map(|l| ProviderView { code: l.code(), title: l.button(), kind: if matches!(l, Link::Code(_)) { "code" } else { "password" } }).collect(),
        state: name,
        provider: provider.map(Link::code),
        title: provider.map(Link::title),
        login,
        dav: provider.filter(|l| !matches!(l, Link::Code(_))).and(dav_of(st)).map(|l| DavView { host: dav::host_of(&l.url).to_string(), url: l.url, user: l.user }),
        cloud_exists: meta(st, META_EXISTS).as_deref() == Some("1"),
        backup_password: meta(st, crate::backup::META_KEY).is_some(),
        every: every(st),
        running: RUNNING.load(Ordering::SeqCst),
        progress: PROGRESS.lock().ok().and_then(|p| *p),
        last: meta(st, META_LAST).and_then(|s| serde_json::from_str(&s).ok()),
        error: meta(st, META_ERROR).and_then(|s| serde_json::from_str(&s).ok()),
        key_pending: meta(st, META_KEYFILE).is_some(),
        notice,
    }
}

fn status(app: &AppHandle, notice: Option<String>) -> Res<Status> {
    let state = app.state::<AppState>();
    let st = state.store()?;
    Ok(status_of(&state, &st, notice))
}

// ——— Команды ———

#[tauri::command]
pub fn cloud_status(app: AppHandle) -> Res<Status> {
    status(&app, None)
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
        let state = app.state::<AppState>();
        *state.cloud_login.lock().map_err(err)? =
            Some(Login { provider: p, deadline: start + Duration::from_secs(code.expires_in), next: start + Duration::from_secs(code.interval), interval: code.interval, code });
        status(&app, None)
    })
    .await
    .map_err(err)?
}

/// Ещё раз открыть страницу, где вводят код.
#[tauri::command]
pub fn cloud_open_page(app: AppHandle) -> Res<()> {
    let state = app.state::<AppState>();
    state.unlocked()?;
    let url = state.cloud_login.lock().map_err(err)?.as_ref().map(|l| l.code.url.clone());
    if let Some(url) = url {
        open_page(&app, &url);
    }
    Ok(())
}

/// Страница Яндекс ID, где создают пароль приложения.
#[tauri::command]
pub fn cloud_app_passwords(app: AppHandle) -> Res<()> {
    app.state::<AppState>().unlocked()?;
    open_page(&app, "https://id.yandex.ru/security");
    Ok(())
}

/// Вход по логину и паролю: Яндекс Диск (пароль приложения) или свой сервер WebDAV.
/// Проверяет вход, создаёт папку «Карман» и узнаёт, есть ли в ней данные.
#[tauri::command]
pub async fn cloud_dav_login(app: AppHandle, kind: String, url: Option<String>, user: String, password: String) -> Res<Status> {
    app.state::<AppState>().unlocked()?;
    let link = match kind.as_str() {
        "yadisk" => Link::YaDisk,
        "webdav" => Link::WebDav,
        _ => return Err("Неизвестное облако".into()),
    };
    let mut bytes = password.into_bytes();
    tauri::async_runtime::spawn_blocking(move || {
        let r = std::str::from_utf8(&bytes).map_err(err).and_then(|p| dav_login(&app, link, url.as_deref(), &user, p));
        bytes.fill(0);
        std::hint::black_box(&bytes);
        r
    })
    .await
    .map_err(err)?
}

fn dav_login(app: &AppHandle, link: Link, url: Option<&str>, user: &str, password: &str) -> Res<Status> {
    let user = user.trim();
    // Пароль приложения Яндекса — 16 латинских букв; пробелы при вставке убираем.
    let password: String = if link == Link::YaDisk { password.chars().filter(|c| !c.is_whitespace()).collect() } else { password.to_string() };
    if user.is_empty() {
        return Err(if link == Link::YaDisk { "Введите логин Яндекса" } else { "Введите логин" }.into());
    }
    if password.is_empty() {
        return Err(if link == Link::YaDisk { "Введите пароль приложения" } else { "Введите пароль" }.into());
    }
    let url = match link {
        Link::YaDisk => yadisk_url(),
        _ => dav::server_url(url.unwrap_or(""), cfg!(debug_assertions))?,
    };
    let exists = Dav::new(&url, user, &password).connect().map_err(err)?;
    let blob = karman_secure::protect(password.as_bytes()).map_err(|e| format!("пароль не сохранить в хранилище системы: {e}"))?;
    let login = DavLogin { url, user: user.to_string() };
    let state = app.state::<AppState>();
    *state.cloud_login.lock().map_err(err)? = None;
    let st = state.store()?;
    // То же облако, что было (пароль сменили): обмен продолжается без пароля копий.
    let same = link_of(&st) == Some(link) && dav_of(&st).as_ref() == Some(&login) && meta(&st, META_KEY).is_some();
    if !same {
        clear(&st, &[META_KEY, META_STATE, META_LAST, META_KEYFILE]);
    }
    st.set_meta(META_PROVIDER, link.code()).map_err(err)?;
    st.set_meta(META_DAV, &serde_json::to_string(&login).map_err(err)?).map_err(err)?;
    st.set_meta(META_TOKENS, &B64.encode(blob)).map_err(err)?;
    st.set_meta(META_EXISTS, if exists || same { "1" } else { "0" }).map_err(err)?;
    st.set_meta(META_ERROR, "").map_err(err)?;
    let s = status_of(&state, &st, same.then(|| "Вход обновлён — обмен продолжается".to_string()));
    drop(st);
    if same {
        sync_now();
    }
    Ok(s)
}

#[tauri::command]
pub fn cloud_login_cancel(app: AppHandle) -> Res<Status> {
    *app.state::<AppState>().cloud_login.lock().map_err(err)? = None;
    status(&app, None)
}

/// Ввели ли код: экран спрашивает каждые пару секунд, пока виден код.
#[tauri::command]
pub async fn cloud_login_check(app: AppHandle) -> Res<Status> {
    app.state::<AppState>().unlocked()?;
    tauri::async_runtime::spawn_blocking(move || check_login(&app)).await.map_err(err)?
}

fn check_login(app: &AppHandle) -> Res<Status> {
    let state = app.state::<AppState>();
    let due = {
        let mut slot = state.cloud_login.lock().map_err(err)?;
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
                if let Some(l) = state.cloud_login.lock().map_err(err)?.as_mut() {
                    l.interval += 5;
                    l.next = Instant::now() + Duration::from_secs(l.interval);
                }
            }
            Ok(Poll::Done(tokens)) => {
                *state.cloud_login.lock().map_err(err)? = None;
                signed_in(app, p, auth, tokens)?;
            }
            Err(e) => {
                *state.cloud_login.lock().map_err(err)? = None;
                return Err(e.to_string());
            }
        }
    }
    status(app, None)
}

/// Вход прошёл. Если это то же облако, что было (вход устарел), обмен продолжается без пароля.
fn signed_in(app: &AppHandle, p: Provider, auth: Auth, tokens: Tokens) -> Res<()> {
    let state = app.state::<AppState>();
    let same = {
        let st = state.store()?;
        link_of(&st) == Some(Link::Code(p)) && meta(&st, META_KEY).is_some()
    };
    // Есть ли в облаке данные «Кармана»: от этого зависит, вводить пароль или придумывать.
    let session = Session::new(auth, tokens);
    let exists = same || session.exists(Dir::Root, kc::KEY_FILE).map_err(err)?;
    let st = state.store()?;
    if !same {
        clear(&st, &[META_KEY, META_STATE, META_LAST, META_KEYFILE, META_DAV]);
    }
    st.set_meta(META_PROVIDER, p.code()).map_err(err)?;
    st.set_meta(META_TOKENS, &serde_json::to_string(&session.tokens()).map_err(err)?).map_err(err)?;
    st.set_meta(META_EXISTS, if exists { "1" } else { "0" }).map_err(err)?;
    st.set_meta(META_ERROR, "").map_err(err)?;
    drop(st);
    if same {
        sync_now();
    }
    Ok(())
}

/// Пароль копий: открывает ключ облака, которое уже есть, или создаёт новое.
#[tauri::command]
pub async fn cloud_password(app: AppHandle, password: String) -> Res<Status> {
    app.state::<AppState>().unlocked()?;
    let mut bytes = password.into_bytes();
    tauri::async_runtime::spawn_blocking(move || {
        let r = std::str::from_utf8(&bytes).map_err(err).and_then(|p| set_password(&app, p));
        bytes.fill(0);
        std::hint::black_box(&bytes);
        r
    })
    .await
    .map_err(err)?
}

fn set_password(app: &AppHandle, password: &str) -> Res<Status> {
    check_password(password).map_err(err)?;
    let state = app.state::<AppState>();
    let (session, backup_meta) = {
        let st = state.store()?;
        (open(&st)?, meta(&st, crate::backup::META_KEY))
    };
    let existing = kc::read_key_file(&session).map_err(err)?;
    // На компьютере пароль облака и пароль копий — один и тот же.
    let mut set_backup = false;
    let mut notice = None;
    if cfg!(desktop) {
        match backup_meta.as_deref().map(BackupKey::from_meta).transpose().map_err(err)? {
            Some(bk) if bk.matches(password).map_err(err)? => {}
            Some(_) if existing.is_none() => {
                return Err("Это не пароль копий этого компьютера. Введите пароль, которым зашифрованы резервные копии, — сменить его можно в разделе «Резервная копия»".into());
            }
            Some(_) => {
                set_backup = true;
                notice = Some("Пароль копий на этом компьютере теперь тот же, что у облака. Новые копии делаются с ним, прежние открываются прежним паролем.".into());
            }
            None => {
                set_backup = true;
                notice = Some("Этот пароль теперь и пароль резервных копий на этом компьютере.".into());
            }
        }
    }
    let (key, _created) = kc::join(&session, password, Kdf::DEFAULT, now()).map_err(err)?;
    let backup_key = if set_backup { Some(BackupKey::new(password).map_err(err)?) } else { None };
    let st = state.store()?;
    st.set_meta(META_KEY, &key.to_meta()).map_err(err)?;
    clear(&st, &[META_STATE, META_ERROR, META_KEYFILE]);
    st.set_meta(META_EXISTS, "1").map_err(err)?;
    session.save_tokens(&st);
    if let Some(bk) = backup_key {
        st.set_meta(crate::backup::META_KEY, &bk.to_meta()).map_err(err)?;
    }
    let s = status_of(&state, &st, notice);
    drop(st);
    sync_now();
    Ok(s)
}

#[tauri::command]
pub fn cloud_sync_now(app: AppHandle) -> Res<Status> {
    let state = app.state::<AppState>();
    let st = state.store()?;
    if meta(&st, META_KEY).is_none() || meta(&st, META_TOKENS).is_none() {
        return Err("Облако не подключено".into());
    }
    let s = status_of(&state, &st, None);
    drop(st);
    sync_now();
    Ok(s)
}

#[tauri::command]
pub fn cloud_every(app: AppHandle, every: String) -> Res<Status> {
    if !["auto", "manual"].contains(&every.as_str()) {
        return Err("Обмен: сам или только по кнопке".into());
    }
    {
        let state = app.state::<AppState>();
        state.store()?.set_meta(META_EVERY, &every).map_err(err)?;
    }
    status(&app, None)
}

/// Отключает облако на этом устройстве; `wipe` — ещё и удаляет данные «Кармана» из облака.
#[tauri::command]
pub async fn cloud_disconnect(app: AppHandle, wipe: bool) -> Res<Status> {
    app.state::<AppState>().unlocked()?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        CANCEL.store(true, Ordering::SeqCst);
        // Ждём, пока идущий обмен остановится (он проверяет остановку между запросами).
        let _round = ROUND.lock().unwrap_or_else(|e| e.into_inner());
        let (loaded, yadisk) = {
            let st = state.store()?;
            (open(&st), link_of(&st) == Some(Link::YaDisk))
        };
        let done = (|| -> Res<()> {
            let Ok(conn) = loaded else {
                return if wipe { Err("Чтобы удалить данные из облака, сначала войдите в него".into()) } else { Ok(()) };
            };
            if wipe {
                kc::wipe(&conn, &AtomicBool::new(false)).map_err(err)?;
            }
            match &conn {
                Conn::Code(s) => s.auth().revoke(&s.tokens()),
                // Пустые папки «Кармана» убираем; не вышло — не страшно.
                Conn::Dav(d) if wipe => {
                    let _ = d.tidy();
                }
                Conn::Dav(_) => {}
            }
            Ok(())
        })();
        CANCEL.store(false, Ordering::SeqCst);
        // Вход не отозвался (нет связи) — всё равно отключаемся; данные из облака не удалились — остаёмся.
        if wipe {
            done?;
        }
        let st = state.store()?;
        clear(&st, &[META_PROVIDER, META_TOKENS, META_DAV, META_KEY, META_STATE, META_LAST, META_ERROR, META_KEYFILE, META_EXISTS]);
        *state.cloud_login.lock().map_err(err)? = None;
        let mut note = if wipe { "Облако отключено, данные «Кармана» из него удалены." } else { "Облако отключено. Данные «Кармана» в нём остались — его можно подключить снова." }.to_string();
        if yadisk {
            note.push_str(" Пароль приложения больше не нужен «Карману» — его можно удалить в Яндекс ID.");
        }
        Ok(status_of(&state, &st, Some(note)))
    })
    .await
    .map_err(err)?
}

/// Ключ облака на этом устройстве, если облако подключено.
pub fn key_meta(st: &Store) -> Option<String> {
    meta(st, META_KEY)
}

/// Пароль копий сменили: тот же ключ облака под новым паролем (около секунды).
pub fn rewrap(key_meta: &str, password: &str) -> Res<KeyFile> {
    let key = CloudKey::from_meta(key_meta).map_err(err)?;
    key.key_file(password, Kdf::DEFAULT, now()).map_err(err)
}

/// Новый ключ облака уйдёт в облако при ближайшем обмене.
pub fn save_keyfile(st: &Store, kf: &KeyFile) -> Res<()> {
    st.set_meta(META_KEYFILE, &String::from_utf8_lossy(&kf.to_bytes())).map_err(err)?;
    sync_now();
    Ok(())
}

// ——— Обмен в фоне ———

struct Loaded {
    conn: Res<Conn>,
    key: CloudKey,
    state: CloudState,
    keyfile: Option<String>,
}

fn load(st: &Store) -> Option<Loaded> {
    if !ready(st) {
        return None;
    }
    Some(Loaded {
        conn: open(st),
        key: CloudKey::from_meta(&meta(st, META_KEY)?).ok()?,
        state: meta(st, META_STATE).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        keyfile: meta(st, META_KEYFILE),
    })
}

/// Событие для экрана: обмен идёт или закончился; сколько пришло нового.
#[derive(Clone, Serialize)]
struct CloudEvent {
    running: bool,
    received: usize,
    step: Option<Step>,
}

fn emit(app: &AppHandle, ev: CloudEvent) {
    let _ = app.emit("cloud", ev);
}

struct RoundDone;

impl Drop for RoundDone {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
        if let Ok(mut p) = PROGRESS.lock() {
            *p = None;
        }
    }
}

/// Один обмен. Возвращает, что пошло не так.
fn run_round(app: &AppHandle) -> Option<CloudFail> {
    let Ok(_round) = ROUND.try_lock() else { return None };
    let state = app.state::<AppState>();
    let c = state.store.lock().ok().and_then(|st| load(&st))?;
    RUNNING.store(true, Ordering::SeqCst);
    let _done = RoundDone;
    emit(app, CloudEvent { running: true, received: 0, step: None });
    let session = match c.conn {
        Ok(s) => s,
        Err(e) => return fail(app, &state, CloudFail::Auth, e),
    };
    let mut cstate = c.state;
    let mut last_emit = Instant::now();
    let mut result: karman_core::Result<Report> = Ok(Report::default());
    if let Some(kf_text) = &c.keyfile {
        match KeyFile::parse(kf_text.as_bytes()).and_then(|kf| kc::put_key_file(&session, &c.key, &kf)) {
            Ok(()) => {
                if let Ok(st) = state.store.lock() {
                    // Пока шёл обмен, пароль могли сменить ещё раз — тогда ждёт уже новый ключ.
                    if meta(&st, META_KEYFILE).as_deref() == Some(kf_text) {
                        clear(&st, &[META_KEYFILE]);
                    }
                }
            }
            Err(e) => result = Err(e),
        }
    }
    if result.is_ok() {
        let mut progress = |step: Step| {
            if let Ok(mut p) = PROGRESS.lock() {
                *p = Some(step);
            }
            if last_emit.elapsed() >= Duration::from_millis(400) {
                last_emit = Instant::now();
                emit(app, CloudEvent { running: true, received: 0, step: Some(step) });
            }
        };
        result = kc::sync(&Round { cloud: &session, store: &state.store, key: &c.key, now: now(), cancel: &CANCEL }, &mut cstate, &mut progress);
    }
    let Ok(st) = state.store.lock() else { return None };
    let _ = st.set_meta(META_STATE, &serde_json::to_string(&cstate).unwrap_or_default());
    session.save_tokens(&st);
    match result {
        Ok(r) => {
            let last = Last { at: now(), received: r.received, sent: r.sent, files_received: r.files_received, files_sent: r.files_sent, files_waiting: r.files_waiting, problems: r.problems };
            let _ = st.set_meta(META_LAST, &serde_json::to_string(&last).unwrap_or_default());
            clear(&st, &[META_ERROR]);
            drop(st);
            emit(app, CloudEvent { running: false, received: r.received + r.files_received, step: None });
            None
        }
        Err(e) => {
            drop(st);
            let (kind, text) = match e {
                Error::Cloud(k, t) => (k, t),
                Error::CloudPassword => (CloudFail::Replaced, e.to_string()),
                other => (CloudFail::Server, format!("Обмен с облаком не получился: {other}")),
            };
            if kind == CloudFail::Cancelled {
                emit(app, CloudEvent { running: false, received: 0, step: None });
                return None;
            }
            fail(app, &state, kind, text)
        }
    }
}

/// Запоминает, что пошло не так, и что для этого нужно сделать.
fn fail(app: &AppHandle, state: &AppState, kind: CloudFail, text: String) -> Option<CloudFail> {
    if let Ok(st) = state.store.lock() {
        match kind {
            // Вход устарел: войти снова — ключ облака остаётся, пароль не понадобится.
            CloudFail::Auth => clear(&st, &[META_TOKENS]),
            // Данных в облаке нет или облако создано заново: нужен пароль копий.
            CloudFail::Gone => {
                clear(&st, &[META_KEY, META_STATE, META_KEYFILE]);
                let _ = st.set_meta(META_EXISTS, "0");
            }
            CloudFail::Replaced => {
                clear(&st, &[META_KEY, META_STATE, META_KEYFILE]);
                let _ = st.set_meta(META_EXISTS, "1");
            }
            _ => {}
        }
        let p = Problem { kind, text, at: now() };
        let _ = st.set_meta(META_ERROR, &serde_json::to_string(&p).unwrap_or_default());
    }
    emit(app, CloudEvent { running: false, received: 0, step: None });
    Some(kind)
}

fn ready(st: &Store) -> bool {
    link_of(st).is_some() && meta(st, META_TOKENS).is_some() && meta(st, META_KEY).is_some()
}

/// Для сквозных проверок в отладочной сборке паузы короче.
fn secs(name: &str, default: u64) -> Duration {
    #[cfg(debug_assertions)]
    if let Some(s) = std::env::var(name).ok().and_then(|v| v.parse().ok()) {
        return Duration::from_secs(s);
    }
    let _ = name;
    Duration::from_secs(default)
}

/// Обмен в фоне: проверяем каждые полминуты, не пора ли.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let tick = secs("KARMAN_CLOUD_TICK_SECS", 30);
        let change_wait = secs("KARMAN_CLOUD_CHANGE_SECS", 60);
        // Первый обмен — вскоре после запуска, чтобы не мешать открытию.
        sleep_or_wake(secs("KARMAN_CLOUD_DELAY_SECS", 10));
        let mut seen: Option<u64> = None;
        let mut changed_at: Option<Instant> = None;
        let mut last_round: Option<Instant> = None;
        let mut retry_at: Option<Instant> = None;
        let mut first = true;
        loop {
            if !first {
                sleep_or_wake(tick);
            }
            first = false;
            let state = app.state::<AppState>();
            let Some((is_ready, auto, events)) = state.store.lock().ok().map(|st| (ready(&st), every(&st) == "auto", st.event_count().unwrap_or(0))) else { continue };
            let manual = MANUAL.swap(false, Ordering::SeqCst);
            let back = RESUMED.swap(false, Ordering::SeqCst);
            if !is_ready {
                last_round = None;
                retry_at = None;
                continue;
            }
            if seen.is_some_and(|s| s != events) && changed_at.is_none() {
                changed_at = Some(Instant::now());
            }
            seen.get_or_insert(events);
            let since = last_round.map(|t| t.elapsed());
            let can_retry = retry_at.is_none_or(|t| Instant::now() >= t);
            let due = manual
                || (auto && can_retry && (since.is_none_or(|d| d >= EVERY) || changed_at.is_some_and(|t| t.elapsed() >= change_wait) || (back && since.is_none_or(|d| d >= RESUME_GAP))));
            if !due {
                continue;
            }
            let failed = run_round(&app);
            last_round = Some(Instant::now());
            changed_at = None;
            seen = state.store.lock().ok().and_then(|st| st.event_count().ok()).or(Some(events));
            retry_at = match failed {
                Some(CloudFail::Offline | CloudFail::Server | CloudFail::Space | CloudFail::Format) => Some(Instant::now() + RETRY),
                Some(CloudFail::Conflict | CloudFail::Newer) => Some(Instant::now() + Duration::from_secs(6 * 3600)),
                _ => None,
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_yandex_and_google_pages_open() {
        assert!(safe_page("https://oauth.yandex.ru/device"));
        assert!(safe_page("https://id.yandex.ru/security"));
        assert!(safe_page("https://www.google.com/device"));
        assert!(!safe_page("http://oauth.yandex.ru/device"));
        assert!(!safe_page("https://oauth.yandex.ru.evil.example/device"));
        assert!(!safe_page("https://evil.example/?https://www.google.com/"));
        assert!(!safe_page("https://www.google.com/device\" onclick"));
        assert!(!safe_page("file:///etc/passwd"));
    }

    #[test]
    fn links_by_code_and_by_password() {
        for l in [Link::Code(Provider::Yandex), Link::Code(Provider::Google), Link::YaDisk, Link::WebDav] {
            assert_eq!(Link::parse(l.code()), Some(l));
        }
        assert_eq!(Link::parse("dropbox"), None);
        assert_eq!(Link::YaDisk.title(), "Яндекс Диск");
        assert_ne!(Link::YaDisk.button(), Link::Code(Provider::Yandex).button(), "две кнопки Яндекса различимы");
    }
}
