//! Google Drive, API v3. С доступом drive.file приложение видит только файлы, которые создало само:
//! папку «Карман» в корне Диска и всё внутри неё.

use crate::http::{enc, fail, no_access, no_space, odd, server, Body, Resp};
use crate::Session;
use karman_core::cloud::{Dir, Entry};
use karman_core::error::{CloudFail, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

pub(crate) const OAUTH: &str = "https://oauth2.googleapis.com";
pub(crate) const API: &str = "https://www.googleapis.com/drive/v3";
pub(crate) const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3";
pub(crate) const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const ROOT: &str = "Карман";
const FOLDER: &str = "application/vnd.google-apps.folder";
const SMALL: usize = 4 << 20;

/// Номера папок и файлов, чтобы не искать их каждый раз.
#[derive(Default)]
pub(crate) struct Cache {
    folders: BTreeMap<Dir, String>,
    ids: BTreeMap<(Dir, String), Vec<String>>,
}

#[derive(Deserialize)]
struct File {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: Option<String>,
}

fn trouble(r: &Resp) -> karman_core::Error {
    match (r.status, r.error_code().as_str()) {
        (403, "storageQuotaExceeded") | (413, _) => no_space(),
        (403, "userRateLimitExceeded" | "rateLimitExceeded") | (429, _) => server(429),
        (403, _) => no_access(),
        (code, _) => server(code),
    }
}

/// Значение в запросе поиска Google Drive — в одинарных кавычках.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn search(s: &Session, q: &str) -> Result<Vec<File>> {
    #[derive(Deserialize)]
    struct Page {
        #[serde(default)]
        files: Vec<File>,
        #[serde(rename = "nextPageToken")]
        next: Option<String>,
    }
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut url = format!("{}/files?q={}&fields={}&pageSize=1000&spaces=drive&orderBy=createdTime", s.auth.ends.api, enc(q), enc("nextPageToken,files(id,name,size)"));
        if let Some(t) = &token {
            url.push_str(&format!("&pageToken={}", enc(t)));
        }
        let r = s.call("GET", &url, &[], Body::Empty, SMALL)?;
        if !r.ok() {
            return Err(trouble(&r));
        }
        let page: Page = r.json()?;
        out.extend(page.files);
        match page.next {
            Some(t) if !t.is_empty() => token = Some(t),
            _ => return Ok(out),
        }
    }
}

fn create_folder(s: &Session, name: &str, parent: &str) -> Result<String> {
    let meta = serde_json::json!({ "name": name, "mimeType": FOLDER, "parents": [parent] }).to_string();
    let r = s.call("POST", &format!("{}/files?fields=id", s.auth.ends.api), &[], Body::Json(meta.as_bytes()), SMALL)?;
    if !r.ok() {
        return Err(trouble(&r));
    }
    Ok(r.json::<File>()?.id)
}

/// Номер папки; `create` — создать, если её нет.
fn folder_id(s: &Session, dir: Dir, create: bool) -> Result<Option<String>> {
    if let Some(id) = s.google.borrow().folders.get(&dir) {
        return Ok(Some(id.clone()));
    }
    let (name, parent) = match dir {
        Dir::Root => (ROOT, "root".to_string()),
        d => match folder_id(s, Dir::Root, create)? {
            Some(root) => (d.folder(), root),
            None => return Ok(None),
        },
    };
    let q = format!("name={} and mimeType={} and {} in parents and trashed=false", quote(name), quote(FOLDER), quote(&parent));
    // Если папок с таким именем несколько, берём самую старую — её же выберут и другие устройства.
    let id = match search(s, &q)?.into_iter().next() {
        Some(f) => f.id,
        None if create => create_folder(s, name, &parent)?,
        None => return Ok(None),
    };
    s.google.borrow_mut().folders.insert(dir, id.clone());
    Ok(Some(id))
}

/// Номера файлов с этим именем (Google Drive допускает одинаковые имена).
fn find(s: &Session, dir: Dir, name: &str) -> Result<Vec<String>> {
    if let Some(ids) = s.google.borrow().ids.get(&(dir, name.to_string())) {
        return Ok(ids.clone());
    }
    let Some(parent) = folder_id(s, dir, false)? else { return Ok(Vec::new()) };
    let ids: Vec<String> = search(s, &format!("name={} and {} in parents and trashed=false", quote(name), quote(&parent)))?.into_iter().map(|f| f.id).collect();
    if !ids.is_empty() {
        s.google.borrow_mut().ids.insert((dir, name.to_string()), ids.clone());
    }
    Ok(ids)
}

fn forget(s: &Session, dir: Dir, name: &str) {
    s.google.borrow_mut().ids.remove(&(dir, name.to_string()));
}

pub(crate) fn list(s: &Session, dir: Dir) -> Result<Vec<Entry>> {
    let Some(parent) = folder_id(s, dir, false)? else { return Ok(Vec::new()) };
    let files = search(s, &format!("{} in parents and trashed=false and mimeType != {}", quote(&parent), quote(FOLDER)))?;
    let mut cache = s.google.borrow_mut();
    cache.ids.retain(|(d, _), _| *d != dir);
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        cache.ids.entry((dir, f.name.clone())).or_default().push(f.id);
        out.push(Entry { size: f.size.and_then(|v| v.parse().ok()).unwrap_or(0), name: f.name });
    }
    Ok(out)
}

pub(crate) fn get(s: &Session, dir: Dir, name: &str, max: usize) -> Result<Option<Vec<u8>>> {
    let Some(id) = find(s, dir, name)?.into_iter().next() else { return Ok(None) };
    let r = s.call("GET", &format!("{}/files/{}?alt=media", s.auth.ends.api, enc(&id)), &[], Body::Empty, max)?;
    match r.status {
        200 => Ok(Some(r.body)),
        404 => {
            forget(s, dir, name);
            Ok(None)
        }
        _ => Err(trouble(&r)),
    }
}

/// Загрузка с возобновлением: сначала адрес сеанса, потом содержимое. Подходит для файлов любого размера.
fn upload(s: &Session, method: &str, url: &str, meta: &str, data: &[u8]) -> Result<String> {
    let len = data.len().to_string();
    let r = s.call(method, url, &[("X-Upload-Content-Type", "application/octet-stream"), ("X-Upload-Content-Length", &len)], Body::Json(meta.as_bytes()), SMALL)?;
    if !r.ok() {
        return Err(trouble(&r));
    }
    let session = r.location.clone().ok_or_else(odd)?;
    let up = s.call("PUT", &session, &[], Body::Bytes(data), SMALL)?;
    if !up.ok() {
        return Err(match up.status {
            403 | 413 | 429 => trouble(&up),
            code => fail(CloudFail::Server, format!("Файл не загрузился в облако (ответ {code}) — обмен повторится позже")),
        });
    }
    Ok(up.json::<File>()?.id)
}

pub(crate) fn put(s: &Session, dir: Dir, name: &str, data: &[u8]) -> Result<()> {
    let parent = folder_id(s, dir, true)?.ok_or_else(odd)?;
    let ids = find(s, dir, name)?;
    let id = match ids.first() {
        Some(id) => upload(s, "PATCH", &format!("{}/files/{}?uploadType=resumable&fields=id", s.auth.ends.upload, enc(id)), "{}", data)?,
        None => {
            let meta = serde_json::json!({ "name": name, "parents": [parent] }).to_string();
            upload(s, "POST", &format!("{}/files?uploadType=resumable&fields=id", s.auth.ends.upload), &meta, data)?
        }
    };
    // Лишние файлы с тем же именем (два устройства загрузили одновременно) убираем.
    for extra in ids.iter().skip(1) {
        let _ = s.call("DELETE", &format!("{}/files/{}", s.auth.ends.api, enc(extra)), &[], Body::Empty, SMALL);
    }
    s.google.borrow_mut().ids.insert((dir, name.to_string()), vec![id]);
    Ok(())
}

pub(crate) fn exists(s: &Session, dir: Dir, name: &str) -> Result<bool> {
    forget(s, dir, name);
    Ok(!find(s, dir, name)?.is_empty())
}

pub(crate) fn delete(s: &Session, dir: Dir, name: &str) -> Result<()> {
    for id in find(s, dir, name)? {
        let r = s.call("DELETE", &format!("{}/files/{}", s.auth.ends.api, enc(&id)), &[], Body::Empty, SMALL)?;
        if !r.ok() && r.status != 404 {
            return Err(trouble(&r));
        }
    }
    forget(s, dir, name);
    Ok(())
}
