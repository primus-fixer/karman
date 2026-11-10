//! Резервные копии: пароль копии, папка, автокопии по расписанию и «Сделать копию» — на компьютере;
//! восстановление из файла — на компьютере и на телефоне.
//!
//! Телефон копии не делает: после синхронизации по Wi-Fi его записи попадают в копии компьютера,
//! а новый телефон получает всё обратно, когда его свяжут с компьютером.

use crate::{err, notify, snap, AppState, Res};
use karman_core::backup::{self, BackupKey, Checked, MadeBy, Manifest, Reader, Writer};
use karman_core::db::Store;
use karman_core::sync::{EventBody, VersionVector, APP_VERSION};
use karman_core::Date;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// На устройстве (не синхронизируется): ключ копий, папка, как часто, последняя копия, ошибка автокопии.
pub(crate) const META_KEY: &str = "backup.key";
const META_DIR: &str = "backup.dir";
const META_EVERY: &str = "backup.every";
const META_LOCAL: &str = "backup.local";
const META_ERROR: &str = "backup.error";
/// Общая настройка: когда и где сделана последняя копия — видна и на телефоне.
pub const SETTING_LAST: &str = "backup.last";

/// Файл копии при загрузке для восстановления — не больше 8 ГБ.
const MAX_UPLOAD: u64 = 8 << 30;
const CONFLICT: &str = "Копия сделана на этом же устройстве до того, как его база началась заново, и записи в ней расходятся с нынешними. Восстановите её на чистой установке «Кармана»: удалите данные приложения или поставьте его на другое устройство";

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Сбрасывает «копия делается», что бы ни случилось.
struct Running;
impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Last {
    /// Когда: секунды Unix.
    pub at: i64,
    pub path: String,
    pub auto: bool,
    pub bytes: u64,
    pub events: u64,
    pub blobs: u64,
}

#[derive(Serialize)]
pub struct Status {
    /// Копии делает это устройство (компьютер).
    pub desktop: bool,
    pub password: bool,
    pub dir: String,
    /// Папку выбрали (а не папка по умолчанию).
    pub chosen: bool,
    /// day, week или off.
    pub every: String,
    /// Сколько автокопий хранится.
    pub keep: usize,
    pub last: Option<Last>,
    /// Почему не получилась последняя автокопия.
    pub error: Option<String>,
    pub running: bool,
    /// Облако подключено: оно зашифровано тем же паролем.
    pub cloud: bool,
}

fn every(st: &Store) -> String {
    match st.meta(META_EVERY).ok().flatten().as_deref() {
        Some("week") => "week".into(),
        Some("off") => "off".into(),
        _ => "day".into(),
    }
}

fn keep(every: &str) -> usize {
    if every == "week" {
        8
    } else {
        14
    }
}

/// Папка по умолчанию: «Документы/Карман — копии».
fn default_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .document_dir()
        .or_else(|_| app.path().home_dir().map(|h| h.join("Documents")))
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Карман — копии")
}

fn dir_of(app: &AppHandle, st: &Store) -> (PathBuf, bool) {
    match st.meta(META_DIR).ok().flatten().filter(|d| !d.is_empty()) {
        Some(d) => (PathBuf::from(d), true),
        None => (default_dir(app), false),
    }
}

fn status_of(app: &AppHandle, st: &Store) -> Status {
    let (dir, chosen) = dir_of(app, st);
    let every = every(st);
    Status {
        desktop: cfg!(desktop),
        password: st.meta(META_KEY).ok().flatten().is_some(),
        dir: dir.display().to_string(),
        chosen,
        keep: keep(&every),
        every,
        last: st.meta(META_LOCAL).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()),
        error: st.meta(META_ERROR).ok().flatten().filter(|e| !e.is_empty()),
        running: RUNNING.load(Ordering::SeqCst),
        cloud: crate::cloud::key_meta(st).is_some(),
    }
}

#[tauri::command]
pub fn backup_status(app: AppHandle, state: State<AppState>) -> Res<Status> {
    let st = state.store()?;
    Ok(status_of(&app, &st))
}

/// Пароль копии: из него считается ключ копий (около секунды) и хранится в базе.
/// Прежние копии открываются прежним паролем.
#[tauri::command]
pub async fn backup_password(app: AppHandle, password: String) -> Res<Status> {
    if !cfg!(desktop) {
        return Err("Копии делает «Карман» на компьютере".into());
    }
    // Облако зашифровано тем же паролем: его ключ перешифровывается вместе с паролем копий.
    let cloud_key = crate::cloud::key_meta(&*app.state::<AppState>().store()?);
    let mut bytes = password.into_bytes();
    let (key, cloud_file) = tauri::async_runtime::spawn_blocking(move || {
        let made = std::str::from_utf8(&bytes).map_err(err).and_then(|p| {
            let key = BackupKey::new(p).map_err(err)?;
            let cloud_file = cloud_key.as_deref().map(|ck| crate::cloud::rewrap(ck, p)).transpose()?;
            Ok((key, cloud_file))
        });
        bytes.fill(0);
        std::hint::black_box(&bytes);
        made
    })
    .await
    .map_err(err)??;
    let state = app.state::<AppState>();
    let st = state.store()?;
    st.set_meta(META_KEY, &key.to_meta()).map_err(err)?;
    if let Some(kf) = cloud_file {
        crate::cloud::save_keyfile(&st, &kf)?;
    }
    Ok(status_of(&app, &st))
}

#[tauri::command]
pub fn backup_every(app: AppHandle, state: State<AppState>, every: String) -> Res<Status> {
    if !["day", "week", "off"].contains(&every.as_str()) {
        return Err("Как часто: каждый день, раз в неделю или выключено".into());
    }
    let st = state.store()?;
    st.set_meta(META_EVERY, &every).map_err(err)?;
    Ok(status_of(&app, &st))
}

/// В папку можно писать: создаём её и пробуем записать и удалить маленький файл.
fn check_writable(dir: &Path) -> Res<()> {
    let bad = |e: std::io::Error| format!("В папку «{}» не записать: {e}", dir.display());
    std::fs::create_dir_all(dir).map_err(bad)?;
    let probe = dir.join(".karman-probe");
    std::fs::write(&probe, b"ok").map_err(bad)?;
    std::fs::remove_file(&probe).map_err(bad)
}

/// Папка копий: путь или `None` — папка по умолчанию.
#[tauri::command]
pub fn backup_dir(app: AppHandle, state: State<AppState>, path: Option<String>) -> Res<Status> {
    let st = state.store()?;
    match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => {
            let dir = PathBuf::from(p.trim());
            if !dir.is_absolute() {
                return Err("Нужен полный путь к папке".into());
            }
            check_writable(&dir)?;
            st.set_meta(META_DIR, &dir.display().to_string()).map_err(err)?;
        }
        None => st.set_meta(META_DIR, "").map_err(err)?,
    }
    Ok(status_of(&app, &st))
}

/// Папка копий в окне системы.
#[tauri::command]
pub async fn backup_pick_dir(app: AppHandle) -> Res<Status> {
    #[cfg(desktop)]
    {
        use tauri_plugin_dialog::DialogExt;
        let current = {
            let state = app.state::<AppState>();
            let st = state.store()?;
            dir_of(&app, &st).0
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let mut dialog = app.dialog().file().set_title("Папка для резервных копий").set_can_create_directories(true);
        if current.is_dir() {
            dialog = dialog.set_directory(&current);
        }
        dialog.pick_folder(move |p| {
            let _ = tx.send(p);
        });
        let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten()).await.map_err(err)?;
        if let Some(p) = picked {
            let dir = p.into_path().map_err(err)?;
            check_writable(&dir)?;
            let state = app.state::<AppState>();
            state.store()?.set_meta(META_DIR, &dir.display().to_string()).map_err(err)?;
        }
    }
    let state = app.state::<AppState>();
    let st = state.store()?;
    Ok(status_of(&app, &st))
}

/// Копия сейчас, в папку копий.
#[tauri::command]
pub async fn backup_now(app: AppHandle) -> Res<Last> {
    app.state::<AppState>().unlocked()?;
    tauri::async_runtime::spawn_blocking(move || make(&app, false)).await.map_err(err)?
}

/// «2026-11-07 1830» по местному времени.
fn stamp(at: i64) -> String {
    let local = at + notify::tz_minutes() as i64 * 60;
    let day = Date::from_epoch_days(local.div_euclid(86_400) as i32);
    let sec = local.rem_euclid(86_400);
    format!("{day} {:02}{:02}", sec / 3600, sec % 3600 / 60)
}

/// Имя без повторов: «… (2).karman».
fn unique(dir: &Path, base: &str) -> PathBuf {
    let mut path = dir.join(format!("{base}.{}", backup::EXT));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).{}", backup::EXT));
        n += 1;
    }
    path
}

/// Делает копию: события под замком базы сразу, снимки — по одному, чтобы экраны не ждали.
/// Пишется во временный файл, читается обратно целиком и только потом получает своё имя.
fn make(app: &AppHandle, auto: bool) -> Res<Last> {
    if !cfg!(desktop) {
        return Err("Копии делает «Карман» на компьютере".into());
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Копия уже делается — подождите немного".into());
    }
    let _running = Running;
    let state = app.state::<AppState>();
    let at = now();
    let (key, dir, events, blobs, manifest) = {
        let st = state.store.lock().map_err(err)?;
        let key = st.meta(META_KEY).map_err(err)?.ok_or_else(|| "Сначала задайте пароль копии".to_string())?;
        let key = BackupKey::from_meta(&key).map_err(err)?;
        let events = st.events_after(&VersionVector::new()).map_err(err)?;
        let blobs = st.present_blobs().map_err(err)?;
        let by = MadeBy { app: APP_VERSION.into(), created: at, device: state.me.name.clone(), device_id: st.device().to_string() };
        let manifest = Manifest::new(&by, events.len() as u64, blobs.len() as u64, st.ops().map_err(err)?.len() as u64, st.trips().map_err(err)?.len() as u64);
        (key, dir_of(app, &st).0, events, blobs, manifest)
    };
    check_writable(&dir)?;
    let path = unique(&dir, &format!("Карман {} {}", if auto { "автокопия" } else { "копия" }, stamp(at)));
    let tmp = path.with_file_name(format!("{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("копия")));
    let written = (|| -> Res<backup::Made> {
        let file = File::create(&tmp).map_err(err)?;
        let mut w = Writer::new(BufWriter::with_capacity(1 << 20, file), &key, &manifest).map_err(err)?;
        w.events(&events).map_err(err)?;
        for id in &blobs {
            let blob = state.store.lock().map_err(err)?.blob(id).map_err(err)?;
            if let Some((mime, data)) = blob {
                w.blob(id, &mime, &data).map_err(err)?;
            }
        }
        let (made, out) = w.finish().map_err(err)?;
        out.into_inner().map_err(|e| err(e.error()))?.sync_all().map_err(err)?;
        // Проверка: копия читается целиком сохранённым ключом и сходится по числу записей.
        let r = Reader::open_with_key(BufReader::new(File::open(&tmp).map_err(err)?), &key).map_err(err)?;
        let checked = backup::verify(r, "", |_| Ok(false)).map_err(err)?;
        if (checked.events, checked.blobs) != (made.events, made.blobs) {
            return Err("копия не сошлась при проверке".into());
        }
        std::fs::rename(&tmp, &path).map_err(err)?;
        Ok(made)
    })();
    let made = match written {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Копия не сделана: {e}"));
        }
    };
    let last = Last { at, path: path.display().to_string(), auto, bytes: made.bytes, events: made.events, blobs: made.blobs };
    {
        let mut st = state.store.lock().map_err(err)?;
        st.set_meta(META_LOCAL, &serde_json::to_string(&last).map_err(err)?).map_err(err)?;
        st.set_meta(META_ERROR, "").map_err(err)?;
        // Когда и где была копия — общая настройка: её увидит и телефон.
        st.append(EventBody::SettingPut { key: SETTING_LAST.into(), value: format!("{at}|{}", state.me.name) }).map_err(err)?;
    }
    if auto {
        let every = state.store.lock().map(|st| every(&st)).unwrap_or_else(|_| "day".into());
        prune(&dir, keep(&every));
    }
    let _ = app.emit("backup", &last);
    Ok(last)
}

/// Оставляет последние автокопии. Трогает только свои файлы: по имени и по началу файла.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut autos: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with("Карман автокопия ") && name.ends_with(&format!(".{}", backup::EXT)) && backup::is_backup(p)
        })
        .collect();
    // Имена со временем «ГГГГ-ММ-ДД ЧЧММ» сортируются по времени.
    autos.sort();
    while autos.len() > keep {
        let _ = std::fs::remove_file(autos.remove(0));
    }
}

/// Пора делать автокопию: задан пароль, есть данные, прошло больше суток (или недели) с прошлой копии.
fn due(st: &Store) -> Res<bool> {
    let hours = match every(st).as_str() {
        "day" => 20,
        "week" => 6 * 24 + 12,
        _ => return Ok(false),
    };
    if st.meta(META_KEY).map_err(err)?.is_none() {
        return Ok(false);
    }
    if st.ops().map_err(err)?.is_empty() && st.trips().map_err(err)?.is_empty() {
        return Ok(false);
    }
    let last = st.meta(META_LOCAL).map_err(err)?.and_then(|s| serde_json::from_str::<Last>(&s).ok()).map(|l| l.at).unwrap_or(0);
    Ok(now() - last >= hours * 3600)
}

/// Первая проверка — через полминуты после запуска, чтобы не мешать открытию.
fn first_delay() -> Duration {
    // Для сквозных проверок в отладочной сборке — быстрее.
    #[cfg(debug_assertions)]
    if let Some(s) = std::env::var("KARMAN_BACKUP_DELAY_SECS").ok().and_then(|v| v.parse().ok()) {
        return Duration::from_secs(s);
    }
    Duration::from_secs(30)
}

/// Автокопии на компьютере: вскоре после запуска и потом каждые четверть часа проверяем, не пора ли.
pub fn start_auto(app: AppHandle) {
    if !cfg!(desktop) {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(first_delay());
        loop {
            let state = app.state::<AppState>();
            let is_due = state.store.lock().ok().map(|st| due(&st).unwrap_or(false)).unwrap_or(false);
            if is_due {
                if let Err(e) = make(&app, true) {
                    if let Ok(st) = state.store.lock() {
                        let _ = st.set_meta(META_ERROR, &e);
                    }
                    let _ = app.emit("backup", ());
                }
            }
            std::thread::sleep(Duration::from_secs(15 * 60));
        }
    });
}

// ——— Восстановление ———

fn uploads(state: &AppState) -> PathBuf {
    state.data_dir.join("restore")
}

/// Загрузки, брошенные на полпути, убираем при запуске.
pub fn clean_uploads(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.join("restore"));
}

fn upload_path(state: &AppState, id: &str) -> Res<PathBuf> {
    if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("неверный номер загрузки".into());
    }
    Ok(uploads(state).join(format!("{id}.part")))
}

#[derive(Serialize)]
pub struct Upload {
    pub id: String,
    pub size: u64,
}

/// Файл копии приходит с экрана кусками (на телефоне его иначе не прочитать):
/// заголовки `x-upload` (номер или `new`) и `x-offset`.
#[tauri::command]
pub fn backup_upload(state: State<AppState>, request: tauri::ipc::Request<'_>) -> Res<Upload> {
    state.unlocked()?;
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err("ожидались байты файла".into());
    };
    let head = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string).ok_or_else(|| format!("нет заголовка {name}"));
    let offset: u64 = head("x-offset")?.parse().map_err(|_| "неверное смещение".to_string())?;
    let given = head("x-upload")?;
    let id = if given == "new" { karman_core::book::new_id().map_err(err)? } else { given };
    let path = upload_path(&state, &id)?;
    std::fs::create_dir_all(uploads(&state)).map_err(err)?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if offset != size {
        return Err("файл копии пришёл не по порядку — выберите его снова".into());
    }
    if size + data.len() as u64 > MAX_UPLOAD {
        let _ = std::fs::remove_file(&path);
        return Err("Файл слишком большой для копии «Кармана»".into());
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(err)?;
    f.write_all(data).map_err(err)?;
    Ok(Upload { id, size: size + data.len() as u64 })
}

/// Проверка копии целиком; события этого же устройства сверяются с базой.
fn check_file(state: &AppState, path: &Path, password: &str) -> Res<Checked> {
    let device = state.store.lock().map_err(err)?.device().to_string();
    let r = Reader::open(BufReader::new(File::open(path).map_err(err)?), password).map_err(err)?;
    let checked = backup::verify(r, &device, |ev| {
        let st = state.store.lock().map_err(|_| karman_core::error::Error::Backup("база занята".into()))?;
        backup::conflicts_with(&st, ev)
    })
    .map_err(err)?;
    if checked.conflicts > 0 {
        return Err(CONFLICT.into());
    }
    Ok(checked)
}

#[derive(Serialize)]
pub struct Info {
    pub created: i64,
    pub device: String,
    pub app: String,
    pub ops: u64,
    pub trips: u64,
    pub events: u64,
    pub blobs: u64,
    pub bytes: u64,
}

/// Что в копии: проверяет пароль и файл целиком, базу не меняет.
#[tauri::command]
pub async fn backup_inspect(app: AppHandle, id: String, password: String) -> Res<Info> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.unlocked()?;
        let path = upload_path(&state, &id)?;
        let bytes = std::fs::metadata(&path).map_err(|_| "Файл копии не загружен — выберите его снова".to_string())?.len();
        let c = check_file(&state, &path, &password)?;
        let m = c.manifest;
        Ok(Info { created: m.created, device: m.device, app: m.app, ops: m.ops, trips: m.trips, events: c.events, blobs: c.blobs, bytes })
    })
    .await
    .map_err(err)?
}

#[derive(Serialize)]
pub struct Restored {
    pub events: u64,
    pub blobs: u64,
    pub ops: u64,
    pub trips: u64,
    pub snapshot: karman_core::book::Snapshot,
}

/// Добавляет копию к данным: сначала проверяет файл целиком, потом применяет —
/// как синхронизация: повторы пропускаются, более позднее изменение побеждает.
#[tauri::command]
pub async fn backup_restore(app: AppHandle, id: String, password: String, today: String) -> Res<Restored> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.unlocked()?;
        let path = upload_path(&state, &id)?;
        check_file(&state, &path, &password)?;
        let r = Reader::open(BufReader::new(File::open(&path).map_err(err)?), &password).map_err(err)?;
        let mut st = state.store()?;
        let (ops0, trips0) = (st.ops().map_err(err)?.len(), st.trips().map_err(err)?.len());
        let added = backup::restore_into(r, &mut st).map_err(err)?;
        karman_core::book::ensure_defaults(&mut st).map_err(err)?;
        let (ops1, trips1) = (st.ops().map_err(err)?.len(), st.trips().map_err(err)?.len());
        let snapshot = snap(&st, &today)?;
        drop(st);
        let _ = std::fs::remove_file(&path);
        Ok(Restored { events: added.events, blobs: added.blobs, ops: ops1.saturating_sub(ops0) as u64, trips: trips1.saturating_sub(trips0) as u64, snapshot })
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn backup_discard(state: State<AppState>, id: String) -> Res<()> {
    state.unlocked()?;
    let path = upload_path(&state, &id)?;
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_latest_autocopies_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let real = |name: &str| std::fs::write(dir.path().join(name), [backup::MAGIC.as_slice(), b"rest"].concat()).unwrap();
        for d in 1..=5 {
            real(&format!("Карман автокопия 2026-11-0{d} 1000.karman"));
        }
        real("Карман копия 2026-11-01 0900.karman");
        // Похожее имя, но не копия «Кармана» — не трогаем.
        std::fs::write(dir.path().join("Карман автокопия 2026-10-01 1000.karman"), b"my notes").unwrap();
        std::fs::write(dir.path().join("отчёт.xlsx"), b"PK").unwrap();
        prune(dir.path(), 3);
        let mut left: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(
            left,
            [
                "Карман автокопия 2026-10-01 1000.karman",
                "Карман автокопия 2026-11-03 1000.karman",
                "Карман автокопия 2026-11-04 1000.karman",
                "Карман автокопия 2026-11-05 1000.karman",
                "Карман копия 2026-11-01 0900.karman",
                "отчёт.xlsx",
            ]
        );
    }

    #[test]
    fn stamps_and_unique_names() {
        assert_eq!(stamp(1_762_540_200).len(), "2026-11-07 1830".len());
        let dir = tempfile::tempdir().unwrap();
        let a = unique(dir.path(), "Карман копия 2026-11-07 1830");
        std::fs::write(&a, b"x").unwrap();
        let b = unique(dir.path(), "Карман копия 2026-11-07 1830");
        assert!(b.to_string_lossy().ends_with("Карман копия 2026-11-07 1830 (2).karman"));
    }
}
