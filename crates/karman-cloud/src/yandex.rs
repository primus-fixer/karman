//! Яндекс Диск, REST API. Приложение работает только в своей папке «Приложения/Карман»:
//! в API она называется `app:/` и создаётся Диском сама при первом обращении.

use crate::http::{enc, fail, no_access, no_space, server, Body, Resp};
use crate::Session;
use karman_core::cloud::{Dir, Entry};
use karman_core::error::{CloudFail, Result};
use serde::Deserialize;

pub(crate) const OAUTH: &str = "https://oauth.yandex.ru";
pub(crate) const API: &str = "https://cloud-api.yandex.net/v1/disk";
/// Ответы со списками и ссылками маленькие.
const SMALL: usize = 4 << 20;
const PAGE: usize = 1000;

fn folder(dir: Dir) -> String {
    format!("app:/{}", dir.folder())
}

fn path(dir: Dir, name: &str) -> String {
    match dir {
        Dir::Root => format!("app:/{name}"),
        d => format!("app:/{}/{name}", d.folder()),
    }
}

fn api(s: &Session, tail: &str) -> String {
    format!("{}{tail}", s.auth.ends.api)
}

/// Общие ответы с ошибкой.
fn trouble(r: &Resp) -> karman_core::Error {
    match r.status {
        403 => no_access(),
        413 | 507 => no_space(),
        code => server(code),
    }
}

#[derive(Deserialize)]
struct Href {
    href: String,
}

pub(crate) fn list(s: &Session, dir: Dir) -> Result<Vec<Entry>> {
    #[derive(Deserialize)]
    struct Item {
        name: String,
        #[serde(default)]
        size: u64,
        #[serde(rename = "type")]
        kind: String,
    }
    #[derive(Deserialize)]
    struct Embedded {
        #[serde(default)]
        items: Vec<Item>,
        #[serde(default)]
        total: usize,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(rename = "_embedded")]
        embedded: Option<Embedded>,
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    loop {
        let url = api(s, &format!("/resources?path={}&limit={PAGE}&offset={offset}&fields={}", enc(&folder(dir)), enc("_embedded.items.name,_embedded.items.size,_embedded.items.type,_embedded.total")));
        let r = s.call("GET", &url, &[], Body::Empty, SMALL)?;
        if r.status == 404 {
            return Ok(out);
        }
        if !r.ok() {
            return Err(trouble(&r));
        }
        let Some(page) = r.json::<Answer>()?.embedded else { return Ok(out) };
        let got = page.items.len();
        out.extend(page.items.into_iter().filter(|i| i.kind == "file").map(|i| Entry { name: i.name, size: i.size }));
        offset += got;
        if got == 0 || offset >= page.total {
            return Ok(out);
        }
    }
}

pub(crate) fn get(s: &Session, dir: Dir, name: &str, max: usize) -> Result<Option<Vec<u8>>> {
    let r = s.call("GET", &api(s, &format!("/resources/download?path={}", enc(&path(dir, name)))), &[], Body::Empty, SMALL)?;
    if r.status == 404 {
        return Ok(None);
    }
    if !r.ok() {
        return Err(trouble(&r));
    }
    let href = r.json::<Href>()?.href;
    // Ссылка на скачивание сама по себе даёт доступ: вход к ней не прикладываем.
    let file = s.auth.http.send("GET", &href, &[], Body::Empty, max)?;
    match file.status {
        200 => Ok(Some(file.body)),
        404 => Ok(None),
        code => Err(server(code)),
    }
}

fn mkdir(s: &Session, dir: Dir) -> Result<()> {
    if dir == Dir::Root || s.yandex_dirs.borrow().contains(&dir) {
        return Ok(());
    }
    let r = s.call("PUT", &api(s, &format!("/resources?path={}", enc(&folder(dir)))), &[], Body::Empty, SMALL)?;
    // 409 — папка уже есть.
    if !r.ok() && r.status != 409 {
        return Err(trouble(&r));
    }
    s.yandex_dirs.borrow_mut().push(dir);
    Ok(())
}

pub(crate) fn put(s: &Session, dir: Dir, name: &str, data: &[u8]) -> Result<()> {
    mkdir(s, dir)?;
    let url = api(s, &format!("/resources/upload?path={}&overwrite=true", enc(&path(dir, name))));
    let mut r = s.call("GET", &url, &[], Body::Empty, SMALL)?;
    if r.status == 409 && r.error_code() == "DiskPathDoesntExistsError" {
        // Папку удалили, пока «Карман» работал: создаём снова.
        s.yandex_dirs.borrow_mut().retain(|d| *d != dir);
        mkdir(s, dir)?;
        r = s.call("GET", &url, &[], Body::Empty, SMALL)?;
    }
    if !r.ok() {
        return Err(trouble(&r));
    }
    let href = r.json::<Href>()?.href;
    let up = s.auth.http.send("PUT", &href, &[], Body::Bytes(data), SMALL)?;
    match up.status {
        200..=202 => Ok(()),
        413 | 507 => Err(no_space()),
        code => Err(fail(CloudFail::Server, format!("Файл не загрузился в облако (ответ {code}) — обмен повторится позже"))),
    }
}

pub(crate) fn exists(s: &Session, dir: Dir, name: &str) -> Result<bool> {
    let r = s.call("GET", &api(s, &format!("/resources?path={}&fields=name", enc(&path(dir, name)))), &[], Body::Empty, SMALL)?;
    match r.status {
        200 => Ok(true),
        404 => Ok(false),
        _ => Err(trouble(&r)),
    }
}

pub(crate) fn delete(s: &Session, dir: Dir, name: &str) -> Result<()> {
    let r = s.call("DELETE", &api(s, &format!("/resources?path={}&permanently=true", enc(&path(dir, name)))), &[], Body::Empty, SMALL)?;
    match r.status {
        200..=299 | 404 => Ok(()),
        _ => Err(trouble(&r)),
    }
}
