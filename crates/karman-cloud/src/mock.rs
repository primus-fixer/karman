//! Имитация серверов Яндекса и Google: вход по коду, Яндекс Диск и Google Drive — в памяти;
//! и сервер WebDAV (`/dav/…`) с входом по логину и паролю, как Яндекс Диск по паролю приложения.
//! Для проверок и сквозных тестов; в выпускную сборку не входит.
//!
//! Управление (для сквозных тестов): `POST /mock/approve` — код «введён», `/mock/deny` — доступ
//! не дали, `/mock/expire` — доступ истёк (обновится по refresh-токену), `/mock/revoke` — вход отозван,
//! `/mock/full?on=1` — на диске нет места, `/mock/small?on=1` — списки по 2 файла, `GET /mock/files`;
//! WebDAV: `/mock/dav-password?value=…` — другой пароль (прежний пароль приложения удалили),
//! `/mock/dav-forbid?on=1` — сервер не пускает (403), `/mock/dav-remove?path=/Карман` — удалить папку.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone)]
struct GFile {
    name: String,
    parent: String,
    folder: bool,
    data: Vec<u8>,
    created: u64,
}

#[derive(Default)]
struct State {
    approve: bool,
    deny: bool,
    full: bool,
    small: bool,
    /// Код входа → сколько раз спросили.
    codes: BTreeMap<String, u32>,
    access: BTreeSet<String>,
    refresh: BTreeSet<String>,
    seq: u64,
    /// Яндекс Диск: «app:/e/имя» → содержимое; папки «app:/e».
    yandex: BTreeMap<String, Vec<u8>>,
    yandex_dirs: BTreeSet<String>,
    /// Ссылки на загрузку и скачивание → путь.
    links: BTreeMap<String, String>,
    google: BTreeMap<String, GFile>,
    /// Сеанс загрузки Google → (номер файла, если он уже есть; имя; папка).
    sessions: BTreeMap<String, (Option<String>, String, String)>,
    /// WebDAV: «/Карман/e/имя» → содержимое; папки «/Карман», «/Карман/e».
    dav: BTreeMap<String, Vec<u8>>,
    dav_dirs: BTreeSet<String>,
    /// Пароль WebDAV, если его сменили.
    dav_password: Option<String>,
    dav_forbid: bool,
    requests: usize,
}

/// Логин и пароль имитации WebDAV.
pub const DAV_USER: &str = "karman-test";
pub const DAV_PASSWORD: &str = "abcdefghijklmnop";

impl State {
    fn next(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("{prefix}{}", self.seq)
    }
}

/// Сервер-имитация на 127.0.0.1.
pub struct Mock {
    pub base: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn lock(st: &Mutex<State>) -> MutexGuard<'_, State> {
    st.lock().unwrap_or_else(|e| e.into_inner())
}

impl Mock {
    pub fn start() -> Mock {
        Mock::start_on(0).expect("порт для имитации облака")
    }

    pub fn start_on(port: u16) -> std::io::Result<Mock> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let base = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        listener.set_nonblocking(true)?;
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp, b) = (state.clone(), stop.clone(), base.clone());
        let thread = std::thread::spawn(move || {
            while !sp.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((s, _)) => {
                        let (st, b) = (st.clone(), b.clone());
                        std::thread::spawn(move || {
                            let _ = s.set_nonblocking(false);
                            let _ = serve(s, &st, &b);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
        Ok(Mock { base, state, stop, thread: Some(thread) })
    }

    /// Коды входа «вводятся» сами (со второго вопроса).
    pub fn approve(&self, yes: bool) {
        lock(&self.state).approve = yes;
    }

    pub fn deny(&self) {
        lock(&self.state).deny = true;
    }

    /// Доступ истёк — обновится по refresh-токену.
    pub fn expire(&self) {
        lock(&self.state).access.clear();
    }

    /// Вход отозван совсем.
    pub fn revoke(&self) {
        let mut st = lock(&self.state);
        st.access.clear();
        st.refresh.clear();
    }

    pub fn full(&self, yes: bool) {
        lock(&self.state).full = yes;
    }

    /// Списки по два файла — проверка постраничного чтения.
    pub fn small_pages(&self, yes: bool) {
        lock(&self.state).small = yes;
    }

    /// Все файлы: «yandex:app:/e/имя» или «google:Карман/e/имя», размер.
    pub fn files(&self) -> Vec<(String, usize)> {
        files_of(&lock(&self.state))
    }

    pub fn requests(&self) -> usize {
        lock(&self.state).requests
    }

    /// Адрес сервера WebDAV.
    pub fn dav_url(&self) -> String {
        format!("{}/dav", self.base)
    }

    /// Другой пароль WebDAV: прежний пароль приложения удалили. `None` — снова прежний.
    pub fn dav_password(&self, p: Option<&str>) {
        lock(&self.state).dav_password = p.map(str::to_string);
    }

    /// Сервер WebDAV не пускает (403).
    pub fn dav_forbid(&self, yes: bool) {
        lock(&self.state).dav_forbid = yes;
    }

    /// Папки WebDAV.
    pub fn dav_dirs(&self) -> Vec<String> {
        lock(&self.state).dav_dirs.iter().cloned().collect()
    }

    /// Удаляет папку или файл WebDAV, как будто это сделал человек в облаке.
    pub fn dav_remove(&self, path: &str) {
        dav_remove(&mut lock(&self.state), path);
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn files_of(st: &State) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = st.yandex.iter().map(|(p, d)| (format!("yandex:{p}"), d.len())).collect();
    out.extend(st.dav.iter().map(|(p, d)| (format!("dav:{p}"), d.len())));
    for f in st.google.values().filter(|f| !f.folder) {
        let mut path = vec![f.name.clone()];
        let mut parent = f.parent.clone();
        while let Some(p) = st.google.get(&parent) {
            path.push(p.name.clone());
            parent = p.parent.clone();
        }
        path.reverse();
        out.push((format!("google:{}", path.join("/")), f.data.len()));
    }
    out.sort();
    out
}

struct Req {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Req {
    fn q(&self, k: &str) -> &str {
        self.query.get(k).map(String::as_str).unwrap_or("")
    }

    fn form(&self) -> BTreeMap<String, String> {
        parse_pairs(&String::from_utf8_lossy(&self.body))
    }

    fn header(&self, k: &str) -> &str {
        self.headers.get(k).map(String::as_str).unwrap_or("")
    }
}

struct Out {
    status: u16,
    body: Vec<u8>,
    location: Option<String>,
}

fn reply(status: u16, v: Value) -> Out {
    Out { status, body: v.to_string().into_bytes(), location: None }
}

fn raw(status: u16, body: Vec<u8>) -> Out {
    Out { status, body, location: None }
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_pairs(s: &str) -> BTreeMap<String, String> {
    s.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').map(|(k, v)| (decode(k), decode(v))).unwrap_or_else(|| (decode(p), String::new()))).collect()
}

fn read_req(stream: &mut TcpStream) -> std::io::Result<Option<Req>> {
    let mut r = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    if headers.get("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let mut body = Vec::new();
    if headers.get("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            r.read_line(&mut size)?;
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            let mut chunk = vec![0u8; n + 2];
            r.read_exact(&mut chunk)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
    } else if let Some(n) = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        body = vec![0u8; n];
        r.read_exact(&mut body)?;
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_pairs(q)),
        None => (target, BTreeMap::new()),
    };
    Ok(Some(Req { method, path, query, headers, body }))
}

fn serve(mut stream: TcpStream, st: &Mutex<State>, base: &str) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let Some(req) = read_req(&mut stream)? else { return Ok(()) };
    let out = {
        let mut st = lock(st);
        st.requests += 1;
        route(&req, &mut st, base)
    };
    let reason = match out.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        428 => "Precondition Required",
        507 => "Insufficient Storage",
        _ => "Other",
    };
    let mut head = format!("HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n", out.status, out.body.len());
    if let Some(l) = &out.location {
        head.push_str(&format!("Location: {l}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&out.body)?;
    stream.flush()
}

fn authorized(req: &Req, st: &State, scheme: &str) -> bool {
    req.header("authorization").strip_prefix(scheme).is_some_and(|t| st.access.contains(t.trim()))
}

fn route(req: &Req, st: &mut State, base: &str) -> Out {
    let p = req.path.as_str();
    match (req.method.as_str(), p) {
        (_, "/mock/approve") => {
            st.approve = true;
            reply(200, json!({}))
        }
        (_, "/mock/deny") => {
            st.deny = true;
            reply(200, json!({}))
        }
        (_, "/mock/expire") => {
            st.access.clear();
            reply(200, json!({}))
        }
        (_, "/mock/revoke") => {
            st.access.clear();
            st.refresh.clear();
            reply(200, json!({}))
        }
        (_, "/mock/full") => {
            st.full = req.q("on") == "1";
            reply(200, json!({}))
        }
        (_, "/mock/small") => {
            st.small = req.q("on") == "1";
            reply(200, json!({}))
        }
        (_, "/mock/dav-password") => {
            st.dav_password = Some(req.q("value").to_string()).filter(|v| !v.is_empty());
            reply(200, json!({}))
        }
        (_, "/mock/dav-forbid") => {
            st.dav_forbid = req.q("on") == "1";
            reply(200, json!({}))
        }
        (_, "/mock/dav-remove") => {
            dav_remove(st, req.q("path"));
            reply(200, json!({}))
        }
        (_, "/mock/files") => reply(200, json!(files_of(st).into_iter().map(|(n, s)| json!({"name": n, "size": s})).collect::<Vec<_>>())),
        ("POST", "/yandex/oauth/device/code" | "/google/oauth/device/code") => device_code(req, st, base, p.starts_with("/yandex")),
        ("POST", "/yandex/oauth/token" | "/google/oauth/token") => token(req, st, p.starts_with("/yandex")),
        ("POST", "/yandex/oauth/revoke_token") => {
            let f = req.form();
            st.access.remove(f.get("access_token").map(String::as_str).unwrap_or(""));
            reply(200, json!({"status": "ok"}))
        }
        ("POST", "/google/oauth/revoke") => {
            let f = req.form();
            let t = f.get("token").cloned().unwrap_or_default();
            st.refresh.remove(&t);
            st.access.remove(&t);
            reply(200, json!({}))
        }
        _ if p == "/dav" || p.starts_with("/dav/") => dav(req, st),
        _ if p.starts_with("/yandex/") => yandex(req, st, base),
        _ if p.starts_with("/google/") => google(req, st, base),
        _ => reply(404, json!({"error": "not found"})),
    }
}

fn device_code(req: &Req, st: &mut State, base: &str, ya: bool) -> Out {
    let f = req.form();
    if f.get("client_id").is_none_or(|c| c.is_empty()) {
        return reply(400, json!({"error": "invalid_client"}));
    }
    if !ya && f.get("scope").map(String::as_str) != Some("https://www.googleapis.com/auth/drive.file") {
        return reply(400, json!({"error": "invalid_scope"}));
    }
    if ya && f.get("device_id").is_none_or(|d| d.len() < 6 || d.len() > 50) {
        return reply(400, json!({"error": "invalid_request"}));
    }
    let code = st.next(if ya { "yd-" } else { "gd-" });
    st.codes.insert(code.clone(), 0);
    let user = if ya { "1234567" } else { "GQVQ-JKEC" };
    reply(200, json!({"device_code": code, "user_code": user, "verification_url": format!("{base}/device"), "interval": 1, "expires_in": 300}))
}

fn token(req: &Req, st: &mut State, ya: bool) -> Out {
    let f = req.form();
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    if get("client_id").is_empty() || get("client_secret").is_empty() {
        return reply(400, json!({"error": "invalid_client"}));
    }
    let grant = get("grant_type");
    let device_grant = if ya { "device_code" } else { "urn:ietf:params:oauth:grant-type:device_code" };
    if grant == device_grant {
        let code = if ya { get("code") } else { get("device_code") };
        let (approve, deny) = (st.approve, st.deny);
        let Some(polls) = st.codes.get_mut(&code) else { return reply(400, json!({"error": if ya { "invalid_grant" } else { "expired_token" }})) };
        *polls += 1;
        if deny {
            return reply(403, json!({"error": "access_denied"}));
        }
        if !approve || *polls < 2 {
            return reply(if ya { 400 } else { 428 }, json!({"error": "authorization_pending"}));
        }
        st.codes.remove(&code);
        let (access, refresh) = (st.next("acc-"), st.next("ref-"));
        st.access.insert(access.clone());
        st.refresh.insert(refresh.clone());
        return reply(200, json!({"access_token": access, "refresh_token": refresh, "expires_in": if ya { 31_536_000 } else { 3599 }, "token_type": "bearer"}));
    }
    if grant == "refresh_token" {
        let refresh = get("refresh_token");
        if !st.refresh.contains(&refresh) {
            return reply(400, json!({"error": "invalid_grant"}));
        }
        let access = st.next("acc-");
        st.access.insert(access.clone());
        // Яндекс присылает refresh-токен снова, Google — нет.
        return if ya {
            reply(200, json!({"access_token": access, "refresh_token": refresh, "expires_in": 31_536_000}))
        } else {
            reply(200, json!({"access_token": access, "expires_in": 3599}))
        };
    }
    reply(400, json!({"error": "unsupported_grant_type"}))
}

// ——— Яндекс Диск ———

fn ya_parent(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("app:", _)) => "app:/".into(),
        Some((parent, _)) => parent.to_string(),
        None => "app:/".into(),
    }
}

fn ya_dir_exists(st: &State, dir: &str) -> bool {
    dir == "app:/" || st.yandex_dirs.contains(dir)
}

fn yandex(req: &Req, st: &mut State, base: &str) -> Out {
    let p = req.path.as_str();
    // Ссылки загрузки и скачивания входа не требуют.
    if let Some(id) = p.strip_prefix("/yandex/up/") {
        let Some(path) = st.links.remove(id) else { return reply(404, json!({})) };
        if st.full {
            return reply(507, json!({"error": "DiskInsufficientStorageError"}));
        }
        st.yandex.insert(path, req.body.clone());
        return raw(201, Vec::new());
    }
    if let Some(id) = p.strip_prefix("/yandex/dl/") {
        return Out { status: 302, body: Vec::new(), location: Some(format!("{base}/yandex/file/{id}")) };
    }
    if let Some(id) = p.strip_prefix("/yandex/file/") {
        return match st.links.get(id).and_then(|path| st.yandex.get(path)) {
            Some(d) => raw(200, d.clone()),
            None => reply(404, json!({})),
        };
    }
    if !authorized(req, st, "OAuth ") {
        return reply(401, json!({"error": "UnauthorizedError", "message": "Не авторизован."}));
    }
    let path = req.q("path").trim_end_matches('/').to_string();
    let path = if path == "app:" { "app:/".to_string() } else { path };
    if !path.starts_with("app:/") {
        return reply(403, json!({"error": "DiskForbiddenError"}));
    }
    match (req.method.as_str(), p) {
        ("GET", "/yandex/api/v1/disk/resources") => {
            if ya_dir_exists(st, &path) {
                let prefix = if path == "app:/" { "app:/".to_string() } else { format!("{path}/") };
                let mut items: Vec<Value> = st.yandex_dirs.iter().filter(|d| ya_parent(d) == path).map(|d| json!({"name": d.trim_start_matches(&prefix), "type": "dir"})).collect();
                items.extend(st.yandex.iter().filter(|(f, _)| ya_parent(f) == path).map(|(f, d)| json!({"name": f.trim_start_matches(&prefix), "size": d.len(), "type": "file"})));
                let total = items.len();
                let mut limit: usize = req.q("limit").parse().unwrap_or(20);
                if st.small {
                    limit = limit.min(2);
                }
                let offset: usize = req.q("offset").parse().unwrap_or(0);
                let page: Vec<Value> = items.into_iter().skip(offset).take(limit).collect();
                return reply(200, json!({"type": "dir", "_embedded": {"items": page, "total": total, "limit": limit, "offset": offset}}));
            }
            match st.yandex.get(&path) {
                Some(d) => reply(200, json!({"name": path.rsplit('/').next(), "size": d.len(), "type": "file"})),
                None => reply(404, json!({"error": "DiskNotFoundError"})),
            }
        }
        ("PUT", "/yandex/api/v1/disk/resources") => {
            if ya_dir_exists(st, &path) {
                return reply(409, json!({"error": "DiskPathPointsToExistentDirectoryError"}));
            }
            if !ya_dir_exists(st, &ya_parent(&path)) {
                return reply(409, json!({"error": "DiskPathDoesntExistsError"}));
            }
            st.yandex_dirs.insert(path);
            reply(201, json!({}))
        }
        ("DELETE", "/yandex/api/v1/disk/resources") => {
            if st.yandex.remove(&path).is_some() {
                return raw(204, Vec::new());
            }
            if st.yandex_dirs.remove(&path) {
                let prefix = format!("{path}/");
                st.yandex.retain(|f, _| !f.starts_with(&prefix));
                return raw(204, Vec::new());
            }
            reply(404, json!({"error": "DiskNotFoundError"}))
        }
        ("GET", "/yandex/api/v1/disk/resources/upload") => {
            if !ya_dir_exists(st, &ya_parent(&path)) {
                return reply(409, json!({"error": "DiskPathDoesntExistsError"}));
            }
            if st.yandex.contains_key(&path) && req.q("overwrite") != "true" {
                return reply(409, json!({"error": "DiskResourceAlreadyExistsError"}));
            }
            if st.full {
                return reply(507, json!({"error": "DiskInsufficientStorageError"}));
            }
            let id = st.next("up-");
            st.links.insert(id.clone(), path);
            reply(200, json!({"href": format!("{base}/yandex/up/{id}"), "method": "PUT", "templated": false}))
        }
        ("GET", "/yandex/api/v1/disk/resources/download") => {
            if !st.yandex.contains_key(&path) {
                return reply(404, json!({"error": "DiskNotFoundError"}));
            }
            let id = st.next("dl-");
            st.links.insert(id.clone(), path);
            reply(200, json!({"href": format!("{base}/yandex/dl/{id}"), "method": "GET", "templated": false}))
        }
        _ => reply(404, json!({"error": "NotFound"})),
    }
}

// ——— Google Drive ———

/// Значение в кавычках сразу после `at`.
fn quoted(s: &str, at: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut i = at;
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    if b.get(i) != Some(&b'\'') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[i + 1..].char_indices();
    while let Some((j, c)) = chars.next() {
        match c {
            '\\' => {
                if let Some((_, n)) = chars.next() {
                    out.push(n);
                }
            }
            '\'' => return Some((out, i + 1 + j + 1)),
            c => out.push(c),
        }
    }
    None
}

#[derive(Default)]
struct Query {
    name: Option<String>,
    mime: Option<String>,
    not_mime: Option<String>,
    parent: Option<String>,
}

/// Разбор запросов поиска, которые делает «Карман».
fn parse_q(q: &str) -> Query {
    let mut out = Query::default();
    let mut i = 0;
    while i < q.len() {
        let rest = &q[i..];
        if rest.starts_with("name=") {
            if let Some((v, end)) = quoted(q, i + 5) {
                out.name = Some(v);
                i = end;
                continue;
            }
        } else if rest.starts_with("mimeType != ") {
            if let Some((v, end)) = quoted(q, i + 12) {
                out.not_mime = Some(v);
                i = end;
                continue;
            }
        } else if rest.starts_with("mimeType=") {
            if let Some((v, end)) = quoted(q, i + 9) {
                out.mime = Some(v);
                i = end;
                continue;
            }
        } else if rest.starts_with('\'') {
            if let Some((v, end)) = quoted(q, i) {
                if q[end..].trim_start().starts_with("in parents") {
                    out.parent = Some(v);
                }
                i = end;
                continue;
            }
        }
        i += rest.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    out
}

const G_FOLDER: &str = "application/vnd.google-apps.folder";

fn google(req: &Req, st: &mut State, base: &str) -> Out {
    if !authorized(req, st, "Bearer ") {
        return reply(401, json!({"error": {"code": 401, "message": "Invalid Credentials", "errors": [{"reason": "authError"}]}}));
    }
    let p = req.path.as_str();
    let created = st.seq;
    match (req.method.as_str(), p) {
        ("GET", "/google/api/drive/v3/files") => {
            let q = parse_q(req.q("q"));
            let mut found: Vec<(&String, &GFile)> = st
                .google
                .iter()
                .filter(|(_, f)| q.name.as_ref().is_none_or(|n| &f.name == n))
                .filter(|(_, f)| q.parent.as_ref().is_none_or(|p| &f.parent == p))
                .filter(|(_, f)| q.mime.as_ref().is_none_or(|m| (m == G_FOLDER) == f.folder))
                .filter(|(_, f)| q.not_mime.as_ref().is_none_or(|m| (m == G_FOLDER) != f.folder))
                .collect();
            found.sort_by_key(|(_, f)| f.created);
            let size = if st.small { 2 } else { 1000 };
            let offset: usize = req.q("pageToken").parse().unwrap_or(0);
            let page: Vec<Value> = found.iter().skip(offset).take(size).map(|(id, f)| json!({"id": id, "name": f.name, "size": f.data.len().to_string()})).collect();
            let mut v = json!({"files": page});
            if offset + size < found.len() {
                v["nextPageToken"] = json!((offset + size).to_string());
            }
            reply(200, v)
        }
        ("POST", "/google/api/drive/v3/files") => {
            let meta: Value = serde_json::from_slice(&req.body).unwrap_or_default();
            let id = st.next("g");
            let parent = meta["parents"][0].as_str().unwrap_or("root").to_string();
            let folder = meta["mimeType"].as_str() == Some(G_FOLDER);
            st.google.insert(id.clone(), GFile { name: meta["name"].as_str().unwrap_or("").to_string(), parent, folder, data: Vec::new(), created });
            reply(200, json!({"id": id}))
        }
        ("POST", "/google/upload/drive/v3/files") | ("PATCH", _) if req.q("uploadType") == "resumable" => {
            if st.full {
                return reply(403, json!({"error": {"code": 403, "errors": [{"reason": "storageQuotaExceeded"}]}}));
            }
            let sid = st.next("s");
            if let Some(id) = p.strip_prefix("/google/upload/drive/v3/files/") {
                let Some(f) = st.google.get(id) else { return reply(404, json!({})) };
                let (name, parent) = (f.name.clone(), f.parent.clone());
                st.sessions.insert(sid.clone(), (Some(id.to_string()), name, parent));
            } else {
                let meta: Value = serde_json::from_slice(&req.body).unwrap_or_default();
                let parent = meta["parents"][0].as_str().unwrap_or("root").to_string();
                st.sessions.insert(sid.clone(), (None, meta["name"].as_str().unwrap_or("").to_string(), parent));
            }
            Out { status: 200, body: Vec::new(), location: Some(format!("{base}/google/upload/session/{sid}")) }
        }
        ("PUT", _) if p.starts_with("/google/upload/session/") => {
            let sid = p.trim_start_matches("/google/upload/session/");
            let Some((existing, name, parent)) = st.sessions.remove(sid) else { return reply(404, json!({})) };
            if st.full {
                return reply(403, json!({"error": {"code": 403, "errors": [{"reason": "storageQuotaExceeded"}]}}));
            }
            let id = existing.unwrap_or_else(|| st.next("g"));
            let created = st.google.get(&id).map(|f| f.created).unwrap_or(created);
            st.google.insert(id.clone(), GFile { name: name.clone(), parent, folder: false, data: req.body.clone(), created });
            reply(200, json!({"id": id, "name": name}))
        }
        ("GET", _) if p.starts_with("/google/api/drive/v3/files/") => {
            let id = p.trim_start_matches("/google/api/drive/v3/files/");
            match st.google.get(id) {
                Some(f) if req.q("alt") == "media" => raw(200, f.data.clone()),
                Some(f) => reply(200, json!({"id": id, "name": f.name})),
                None => reply(404, json!({"error": {"code": 404, "errors": [{"reason": "notFound"}]}})),
            }
        }
        ("DELETE", _) if p.starts_with("/google/api/drive/v3/files/") => {
            let id = p.trim_start_matches("/google/api/drive/v3/files/").to_string();
            if st.google.remove(&id).is_none() {
                return reply(404, json!({"error": {"code": 404, "errors": [{"reason": "notFound"}]}}));
            }
            // Вместе с папкой — всё внутри.
            let mut gone = vec![id];
            while let Some(parent) = gone.pop() {
                let kids: Vec<String> = st.google.iter().filter(|(_, f)| f.parent == parent).map(|(k, _)| k.clone()).collect();
                for k in kids {
                    st.google.remove(&k);
                    gone.push(k);
                }
            }
            raw(204, Vec::new())
        }
        _ => reply(404, json!({"error": {"code": 404}})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_drive_queries_and_form_values() {
        let q = parse_q("name='Карман' and mimeType='application/vnd.google-apps.folder' and 'root' in parents and trashed=false");
        assert_eq!((q.name.as_deref(), q.mime.as_deref(), q.parent.as_deref()), (Some("Карман"), Some(G_FOLDER), Some("root")));
        let q = parse_q("'g12' in parents and trashed=false and mimeType != 'application/vnd.google-apps.folder'");
        assert_eq!((q.name, q.parent.as_deref(), q.not_mime.as_deref()), (None, Some("g12"), Some(G_FOLDER)));
        let q = parse_q(r"name='it\'s' and 'g1' in parents");
        assert_eq!(q.name.as_deref(), Some("it's"));
        assert_eq!(decode("app%3A%2Fe+x%D0%9A"), "app:/e xК");
        assert_eq!(decode("100%"), "100%");
    }
}

// ——— WebDAV ———

/// Путь из адреса: «%D0%9A…» раскодируется, «+» остаётся плюсом.
fn dav_path(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let p = String::from_utf8_lossy(&out).trim_end_matches('/').to_string();
    if p.is_empty() {
        "/".into()
    } else {
        p
    }
}

fn dav_parent(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((parent, _)) => parent.to_string(),
    }
}

fn dav_is_dir(st: &State, path: &str) -> bool {
    path == "/" || st.dav_dirs.contains(path)
}

fn dav_remove(st: &mut State, path: &str) {
    let path = dav_path(path);
    let inside = format!("{path}/");
    st.dav.retain(|p, _| p != &path && !p.starts_with(&inside));
    st.dav_dirs.retain(|p| p != &path && !p.starts_with(&inside));
}

/// Адрес в ответе — как у Яндекс Диска: путь, закодированный по частям.
fn dav_href(path: &str, dir: bool) -> String {
    let mut out: String = path.split('/').filter(|s| !s.is_empty()).map(|s| format!("/{}", crate::http::enc(s))).collect();
    if dir {
        out.push('/');
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn dav_entry(path: &str, size: Option<usize>) -> String {
    let props = match size {
        None => "<d:resourcetype><d:collection/></d:resourcetype>".to_string(),
        Some(n) => format!("<d:resourcetype/><d:getcontentlength>{n}</d:getcontentlength>"),
    };
    format!("<d:response><d:href>{}</d:href><d:propstat><d:status>HTTP/1.1 200 OK</d:status><d:prop>{props}</d:prop></d:propstat></d:response>", dav_href(path, size.is_none()))
}

fn dav(req: &Req, st: &mut State) -> Out {
    let want = format!("{DAV_USER}:{}", st.dav_password.as_deref().unwrap_or(DAV_PASSWORD));
    let given = req.header("authorization").strip_prefix("Basic ").and_then(|b| B64.decode(b.trim()).ok());
    if given.as_deref() != Some(want.as_bytes()) {
        return raw(401, b"Unauthorized".to_vec());
    }
    if st.dav_forbid {
        return raw(403, b"Forbidden".to_vec());
    }
    let path = dav_path(&req.path["/dav".len()..]);
    let parent = dav_parent(&path);
    match req.method.as_str() {
        "PROPFIND" => {
            let mut body = String::from(r#"<?xml version='1.0' encoding='UTF-8'?><d:multistatus xmlns:d="DAV:">"#);
            if dav_is_dir(st, &path) {
                body.push_str(&dav_entry(&path, None));
                if req.header("depth") != "0" {
                    let inside = if path == "/" { "/".to_string() } else { format!("{path}/") };
                    let child = |p: &str| p.strip_prefix(&inside).is_some_and(|rest| !rest.is_empty() && !rest.contains('/'));
                    for d in st.dav_dirs.iter().filter(|d| child(d)) {
                        body.push_str(&dav_entry(d, None));
                    }
                    for (p, data) in st.dav.iter().filter(|(p, _)| child(p)) {
                        body.push_str(&dav_entry(p, Some(data.len())));
                    }
                }
            } else if let Some(data) = st.dav.get(&path) {
                body.push_str(&dav_entry(&path, Some(data.len())));
            } else {
                return raw(404, b"Not Found".to_vec());
            }
            body.push_str("</d:multistatus>");
            raw(207, body.into_bytes())
        }
        "MKCOL" => {
            if dav_is_dir(st, &path) || st.dav.contains_key(&path) {
                raw(405, b"Method Not Allowed".to_vec())
            } else if !dav_is_dir(st, &parent) {
                raw(409, b"Conflict".to_vec())
            } else {
                st.dav_dirs.insert(path);
                raw(201, Vec::new())
            }
        }
        "PUT" => {
            if !dav_is_dir(st, &parent) {
                raw(409, b"Conflict".to_vec())
            } else if dav_is_dir(st, &path) {
                raw(405, b"Method Not Allowed".to_vec())
            } else if st.full {
                raw(507, b"Insufficient Storage".to_vec())
            } else {
                let fresh = st.dav.insert(path, req.body.clone()).is_none();
                raw(if fresh { 201 } else { 204 }, Vec::new())
            }
        }
        "GET" => match st.dav.get(&path) {
            Some(d) => raw(200, d.clone()),
            None => raw(404, b"Not Found".to_vec()),
        },
        "DELETE" => {
            if dav_is_dir(st, &path) && path != "/" || st.dav.contains_key(&path) {
                dav_remove(st, &path);
                raw(204, Vec::new())
            } else {
                raw(404, b"Not Found".to_vec())
            }
        }
        _ => raw(405, b"Method Not Allowed".to_vec()),
    }
}
