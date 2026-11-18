//! Облака «Кармана»: Яндекс Диск, свой сервер WebDAV и Google Drive.
//!
//! Два способа входа:
//! - логин и пароль приложения (WebDAV, [`dav`]): Яндекс Диск с паролем приложения или свой сервер
//!   (Nextcloud, NAS). «Карман» работает в папке «Карман» в корне диска;
//! - по коду (OAuth 2.0, вход на устройствах с ограниченным вводом): «Карман» показывает код,
//!   человек вводит его на странице Яндекса или Google. Нужна регистрация приложения в сборке.
//!   Приложение видит только свою папку: на Яндекс Диске — «Приложения/Карман», на Google Drive —
//!   файлы, которые создало само приложение (drive.file).
//!
//! В облако уходят файлы, уже зашифрованные ядром (`karman_core::cloud`).

pub mod dav;
mod google;
mod http;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod yandex;

use http::{auth_expired, Body, Http, Resp};
use karman_core::cloud::{Cloud, Dir, Entry};
use kit_core::cloud::CloudFail;
use kit_core::Result;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};

pub use kit_cloud::now;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Yandex,
    Google,
}

impl Provider {
    pub fn title(self) -> &'static str {
        match self {
            Provider::Yandex => "Яндекс Диск",
            Provider::Google => "Google Drive",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Provider::Yandex => "yandex",
            Provider::Google => "google",
        }
    }

    pub fn parse(s: &str) -> Option<Provider> {
        match s {
            "yandex" => Some(Provider::Yandex),
            "google" => Some(Provider::Google),
            _ => None,
        }
    }
}

/// Регистрация «Кармана» у Яндекса или Google (из секретов сборки).
#[derive(Clone, Debug)]
pub struct Client {
    pub id: String,
    pub secret: String,
}

/// Адреса серверов: настоящие или имитация для проверок.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub oauth: String,
    pub api: String,
    pub upload: String,
}

impl Endpoints {
    pub fn real(p: Provider) -> Endpoints {
        match p {
            Provider::Yandex => Endpoints { oauth: yandex::OAUTH.into(), api: yandex::API.into(), upload: String::new() },
            Provider::Google => Endpoints { oauth: google::OAUTH.into(), api: google::API.into(), upload: google::UPLOAD.into() },
        }
    }

    /// Имитация серверов по адресу `base` (сквозные проверки).
    pub fn mock(p: Provider, base: &str) -> Endpoints {
        let base = base.trim_end_matches('/');
        match p {
            Provider::Yandex => Endpoints { oauth: format!("{base}/yandex/oauth"), api: format!("{base}/yandex/api/v1/disk"), upload: String::new() },
            Provider::Google => Endpoints { oauth: format!("{base}/google/oauth"), api: format!("{base}/google/api/drive/v3"), upload: format!("{base}/google/upload/drive/v3") },
        }
    }
}

/// Вход в облако. Хранится в базе устройства (она зашифрована).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access: String,
    #[serde(default)]
    pub refresh: String,
    /// Когда истекает доступ: секунды Unix.
    pub expires: i64,
}

/// Код для входа: его показывают человеку, а «Карман» ждёт, пока его введут.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceCode {
    pub user_code: String,
    /// Страница, где вводят код.
    pub url: String,
    pub device_code: String,
    /// Как часто спрашивать, введён ли код: секунды.
    pub interval: u64,
    pub expires_in: u64,
}

pub enum Poll {
    /// Код ещё не ввели.
    Pending,
    /// Спрашиваем слишком часто.
    SlowDown,
    Done(Tokens),
}

/// Вход по коду и обновление входа.
pub struct Auth {
    pub provider: Provider,
    client: Client,
    ends: Endpoints,
    http: Http,
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
}

impl TokenAnswer {
    fn tokens(self, old_refresh: &str) -> Tokens {
        // Google при обновлении новый refresh-токен не присылает — остаётся прежний.
        let refresh = if self.refresh_token.is_empty() { old_refresh.to_string() } else { self.refresh_token };
        let life = if self.expires_in > 0 { self.expires_in } else { 3600 };
        Tokens { access: self.access_token, refresh, expires: now() + life }
    }
}

impl Auth {
    pub fn new(provider: Provider, client: Client, ends: Endpoints) -> Auth {
        Auth { provider, client, ends, http: http::http() }
    }

    fn form(&self, path: &str, form: &[(&str, &str)]) -> Result<Resp> {
        self.http.send("POST", &format!("{}{path}", self.ends.oauth), &[("Accept", "application/json")], Body::Form(form), 1 << 20)
    }

    /// Код для входа. `device_id` и `device_name` Яндекс показывает в списке устройств с доступом.
    pub fn start(&self, device_id: &str, device_name: &str) -> Result<DeviceCode> {
        let r = match self.provider {
            Provider::Yandex => self.form("/device/code", &[("client_id", &self.client.id), ("device_id", device_id), ("device_name", device_name)])?,
            Provider::Google => self.form("/device/code", &[("client_id", &self.client.id), ("scope", google::SCOPE)])?,
        };
        if !r.ok() {
            return Err(http::fail(CloudFail::Auth, format!("{} не начал вход ({}). Проверьте регистрацию приложения", self.provider.title(), r.error_code())));
        }
        #[derive(Deserialize)]
        struct Answer {
            device_code: String,
            user_code: String,
            verification_url: String,
            #[serde(default)]
            interval: u64,
            #[serde(default)]
            expires_in: u64,
        }
        let a: Answer = r.json()?;
        Ok(DeviceCode { user_code: a.user_code, url: a.verification_url, device_code: a.device_code, interval: a.interval.clamp(1, 30), expires_in: if a.expires_in == 0 { 300 } else { a.expires_in } })
    }

    /// Ввели ли код.
    pub fn poll(&self, code: &DeviceCode) -> Result<Poll> {
        let r = match self.provider {
            Provider::Yandex => self.form("/token", &[("grant_type", "device_code"), ("code", &code.device_code), ("client_id", &self.client.id), ("client_secret", &self.client.secret)])?,
            Provider::Google => self.form(
                "/token",
                &[("client_id", &self.client.id), ("client_secret", &self.client.secret), ("device_code", &code.device_code), ("grant_type", "urn:ietf:params:oauth:grant-type:device_code")],
            )?,
        };
        if r.ok() {
            return Ok(Poll::Done(r.json::<TokenAnswer>()?.tokens("")));
        }
        match r.error_code().as_str() {
            "authorization_pending" => Ok(Poll::Pending),
            "slow_down" => Ok(Poll::SlowDown),
            "access_denied" => Err(http::fail(CloudFail::Auth, "Доступ к облаку не разрешили. Начните вход заново, если передумаете")),
            "expired_token" | "invalid_grant" => Err(http::fail(CloudFail::Auth, "Код устарел — начните вход заново")),
            code => Err(http::fail(CloudFail::Auth, format!("{} не пустил ({code}). Проверьте регистрацию приложения", self.provider.title()))),
        }
    }

    /// Новый доступ по refresh-токену.
    pub fn refresh(&self, t: &Tokens) -> Result<Tokens> {
        if t.refresh.is_empty() {
            return Err(auth_expired());
        }
        let r = match self.provider {
            Provider::Yandex => self.form("/token", &[("grant_type", "refresh_token"), ("refresh_token", &t.refresh), ("client_id", &self.client.id), ("client_secret", &self.client.secret)])?,
            Provider::Google => self.form("/token", &[("client_id", &self.client.id), ("client_secret", &self.client.secret), ("refresh_token", &t.refresh), ("grant_type", "refresh_token")])?,
        };
        if r.ok() {
            return Ok(r.json::<TokenAnswer>()?.tokens(&t.refresh));
        }
        if r.status >= 500 {
            return Err(http::server(r.status));
        }
        Err(auth_expired())
    }

    /// Отзывает вход (при отключении облака). Не получилось — не страшно: вход можно отозвать в аккаунте.
    pub fn revoke(&self, t: &Tokens) {
        let _ = match self.provider {
            Provider::Yandex => self.form("/revoke_token", &[("access_token", &t.access), ("client_id", &self.client.id), ("client_secret", &self.client.secret)]),
            Provider::Google => self.form("/revoke", &[("token", if t.refresh.is_empty() { &t.access } else { &t.refresh })]),
        };
    }
}

/// Подключение к облаку для одного обмена. Сам обновляет вход, когда тот истекает.
pub struct Session {
    auth: Auth,
    tokens: RefCell<Tokens>,
    changed: Cell<bool>,
    google: RefCell<google::Cache>,
    yandex_dirs: RefCell<Vec<Dir>>,
}

impl Session {
    pub fn new(auth: Auth, tokens: Tokens) -> Session {
        Session { auth, tokens: RefCell::new(tokens), changed: Cell::new(false), google: RefCell::default(), yandex_dirs: RefCell::default() }
    }

    pub fn provider(&self) -> Provider {
        self.auth.provider
    }

    /// Вход после обмена: если его обновили, сохраните.
    pub fn tokens(&self) -> Tokens {
        self.tokens.borrow().clone()
    }

    pub fn tokens_changed(&self) -> bool {
        self.changed.get()
    }

    pub fn auth(&self) -> &Auth {
        &self.auth
    }

    fn renew(&self) -> Result<()> {
        let fresh = self.auth.refresh(&self.tokens.borrow())?;
        *self.tokens.borrow_mut() = fresh;
        self.changed.set(true);
        Ok(())
    }

    fn header(&self) -> String {
        let t = self.tokens.borrow();
        match self.auth.provider {
            Provider::Yandex => format!("OAuth {}", t.access),
            Provider::Google => format!("Bearer {}", t.access),
        }
    }

    /// Запрос с входом: истекает — обновляем заранее; сервер не принял — обновляем и пробуем ещё раз.
    fn call(&self, method: &str, url: &str, extra: &[(&str, &str)], body: Body, max: usize) -> Result<Resp> {
        if self.tokens.borrow().expires - now() < 120 {
            self.renew()?;
        }
        for attempt in 0..2 {
            let header = self.header();
            // Без Accept: скачивание файла (alt=media) отдаёт сам файл, а не JSON.
            let mut headers: Vec<(&str, &str)> = vec![("Authorization", &header)];
            headers.extend_from_slice(extra);
            let r = self.auth.http.send(method, url, &headers, body, max)?;
            if r.status != 401 {
                return Ok(r);
            }
            if attempt == 0 {
                self.renew()?;
            }
        }
        Err(auth_expired())
    }
}

impl Cloud for Session {
    fn list(&self, dir: Dir) -> Result<Vec<Entry>> {
        match self.auth.provider {
            Provider::Yandex => yandex::list(self, dir),
            Provider::Google => google::list(self, dir),
        }
    }

    fn get(&self, dir: Dir, name: &str, max: usize) -> Result<Option<Vec<u8>>> {
        match self.auth.provider {
            Provider::Yandex => yandex::get(self, dir, name, max),
            Provider::Google => google::get(self, dir, name, max),
        }
    }

    fn put(&self, dir: Dir, name: &str, data: &[u8]) -> Result<()> {
        match self.auth.provider {
            Provider::Yandex => yandex::put(self, dir, name, data),
            Provider::Google => google::put(self, dir, name, data),
        }
    }

    fn exists(&self, dir: Dir, name: &str) -> Result<bool> {
        match self.auth.provider {
            Provider::Yandex => yandex::exists(self, dir, name),
            Provider::Google => google::exists(self, dir, name),
        }
    }

    fn delete(&self, dir: Dir, name: &str) -> Result<()> {
        match self.auth.provider {
            Provider::Yandex => yandex::delete(self, dir, name),
            Provider::Google => google::delete(self, dir, name),
        }
    }
}

#[cfg(test)]
mod tests;
