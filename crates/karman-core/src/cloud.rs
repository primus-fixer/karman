//! Облако: Яндекс Диск, свой сервер WebDAV или Google Drive — общий ящик для устройств одного человека.
//!
//! С 0.15.9 формат облака и обмен с ним — общие, из kit (`kit_core::cloud`), те же, что у «Полки».
//! Здесь — то, чем «Карман» отличается ([`FORMAT`]): файл ключа `karman.key`, метка файлов `KRMC`,
//! расширения `.kev` и `.kbl`, снимки и PDF чеков. Менять их нельзя: устройства перестанут читать
//! то, что уже лежит в облаке.
//!
//! - `karman.key` — ключ облака, зашифрованный Argon2id от пароля копий.
//! - `e/<устройство>-<с>-<по>.kev` — события одного устройства, сжатые и зашифрованные.
//! - `b/<имя>.kbl` — снимок или PDF чека; имя — MAC от отпечатка файла.

use crate::backup::Kdf;
use crate::db::{self, Store};
use crate::error::{Error, Result};
use crate::sync::Event;
use kit_core::cloud::{self as kit, CloudStore};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

pub use kit::{Cloud, Dir, Entry, EventFileName, KeyFile, MemCloud, State, COMPACT_AFTER, FILE_EVENTS, MAX_EVENTS_FILE, MAX_KEY_FILE};

/// Файл ключа облака в папке «Кармана».
pub const KEY_FILE: &str = "karman.key";
/// Предел файла снимка: снимок до 15 МБ и немного на заголовок.
pub const MAX_BLOB_FILE: usize = db::MAX_BLOB + 1024;

fn receipt_ok(mime: &str, data: &[u8]) -> bool {
    db::check_blob(mime, data).is_ok()
}

/// Облако «Кармана». У «Полки» — `polka.key`, `PLKC`, `.pev`, `.pbl`, обложки и файлы книг.
pub static FORMAT: kit::Format = kit::Format {
    names: crate::NAMES,
    key_file: KEY_FILE,
    magic: *b"KRMC",
    events_ext: ".kev",
    max_seq: u64::MAX,
    blobs: kit::Blobs { ext: ".kbl", max_file: MAX_BLOB_FILE, what: "снимок чека", this: "этот снимок", of: "снимка", feminine: false, check: receipt_ok },
    files: None,
};

fn store_err(e: Error) -> kit_core::Error {
    kit_core::Error::Store(e.to_string())
}

/// Чего обмену с облаком нужно от базы «Кармана» сверх обмена по Wi-Fi.
impl CloudStore for Store {
    fn origin(e: &Event) -> (&str, u64) {
        (&e.device, e.seq)
    }
    fn events_range(&self, author: &str, from: u64, to: u64) -> kit_core::Result<Vec<Event>> {
        Store::events_range(self, author, from, to).map_err(store_err)
    }
    fn differs(&self, e: &Event) -> kit_core::Result<bool> {
        Ok(Store::event_body(self, &e.device, e.seq).map_err(store_err)?.is_some_and(|b| b != e.body))
    }
    fn present_blobs(&self) -> kit_core::Result<Vec<String>> {
        Store::present_blobs(self).map_err(store_err)
    }
}

/// Ключ облака. Хранится в базе устройства (она сама зашифрована) — для обмена пароль не нужен.
pub struct CloudKey(kit::CloudKey);

impl CloudKey {
    /// Новый ключ для облака, которое подключают впервые.
    pub fn generate() -> Result<CloudKey> {
        Ok(CloudKey(kit::CloudKey::generate(&FORMAT)?))
    }

    pub fn group(&self) -> &str {
        self.0.group()
    }

    /// Для хранения в базе: «1:номер облака:ключ».
    pub fn to_meta(&self) -> String {
        self.0.to_meta()
    }

    pub fn from_meta(s: &str) -> Result<CloudKey> {
        Ok(CloudKey(kit::CloudKey::from_meta(&FORMAT, s)?))
    }

    /// Файл ключа для облака: ключ облака, зашифрованный паролем копий (около секунды).
    pub fn key_file(&self, password: &str, kdf: Kdf, changed: i64) -> Result<KeyFile> {
        Ok(self.0.key_file(password, kdf, changed)?)
    }

    /// Ключ облака из файла ключа по паролю копий (около секунды).
    pub fn from_key_file(kf: &KeyFile, password: &str) -> Result<CloudKey> {
        Ok(CloudKey(kit::CloudKey::from_key_file(&FORMAT, kf, password)?))
    }

    /// Имя файла снимка в облаке: по нему не узнать отпечаток снимка.
    pub fn blob_name(&self, id: &str) -> String {
        self.0.blob_name(id)
    }
}

/// Файл ключа из облака.
pub fn parse_key_file(data: &[u8]) -> Result<KeyFile> {
    Ok(KeyFile::parse(&FORMAT, data)?)
}

pub fn read_key_file(cloud: &dyn Cloud) -> Result<Option<KeyFile>> {
    Ok(kit::read_key_file(&FORMAT, cloud)?)
}

/// Подключает устройство по паролю копий: ключ облака уже есть — открывает его, нет — создаёт.
/// Возвращает ключ и `true`, если облако подключено впервые.
pub fn join(cloud: &dyn Cloud, password: &str, kdf: Kdf, now: i64) -> Result<(CloudKey, bool)> {
    let (key, created) = kit::join(&FORMAT, cloud, password, kdf, now)?;
    Ok((CloudKey(key), created))
}

/// Новый пароль копий: тот же ключ облака, зашифрованный новым паролем.
pub fn put_key_file(cloud: &dyn Cloud, key: &CloudKey, kf: &KeyFile) -> Result<()> {
    Ok(kit::put_key_file(cloud, &key.0, kf)?)
}

/// Удаляет данные «Кармана» из облака. Сначала ключ: другие устройства увидят это и перестанут слать файлы.
pub fn wipe(cloud: &dyn Cloud, cancel: &AtomicBool) -> Result<usize> {
    Ok(kit::wipe(&FORMAT, cloud, cancel)?)
}

/// Имя файла событий: устройство и номера событий «с…по».
pub fn event_name(author: &str, from: u64, to: u64) -> String {
    kit::event_name(&FORMAT, author, from, to)
}

/// Разбирает имя файла событий. Чужие и неканонические имена («007» вместо «7») не подходят.
pub fn parse_event_name(name: &str) -> Option<EventFileName> {
    kit::parse_event_name(&FORMAT, name)
}

/// Один обмен с облаком.
pub struct Round<'a> {
    pub cloud: &'a dyn Cloud,
    pub store: &'a Mutex<Store>,
    pub key: &'a CloudKey,
    /// Сейчас: секунды Unix.
    pub now: i64,
    /// Остановить обмен между запросами.
    pub cancel: &'a AtomicBool,
}

/// Чем занят обмен — для экрана.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    Events,
    Upload { done: usize, total: usize },
    Download { done: usize, total: usize },
}

impl From<kit::Step> for Step {
    fn from(s: kit::Step) -> Self {
        match s {
            kit::Step::Events => Step::Events,
            kit::Step::Upload { done, total } => Step::Upload { done, total },
            // Больших файлов у «Кармана» нет; на всякий случай — как скачивание.
            kit::Step::Download { done, total } | kit::Step::Files { done, total } => Step::Download { done, total },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Новых событий пришло.
    pub received: usize,
    /// Событий отправлено в облако.
    pub sent: usize,
    /// Снимков и файлов чеков пришло и ушло.
    pub files_received: usize,
    pub files_sent: usize,
    /// Снимков, которых в облаке пока нет: их ещё не отправило другое устройство.
    pub files_waiting: usize,
    /// Что не получилось, но обмену не помешало.
    pub problems: Vec<String>,
}

impl From<kit::Report> for Report {
    fn from(r: kit::Report) -> Self {
        Report { received: r.received, sent: r.sent, files_received: r.files_received, files_sent: r.files_sent, files_waiting: r.files_waiting, problems: r.problems }
    }
}

/// Обмен с облаком: забрать новое, отправить недостающее, слить мелкие файлы, снимки чеков.
pub fn sync(r: &Round, state: &mut State, progress: &mut dyn FnMut(Step)) -> Result<Report> {
    let round = kit::Round { cloud: r.cloud, store: r.store, key: &r.key.0, now: r.now, cancel: r.cancel };
    Ok(kit::sync(&round, state, &mut |s| progress(s.into()))?.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CloudFail;
    use std::sync::MutexGuard;

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }
    use crate::sync::EventBody;
    use std::path::Path;

    const FAST: Kdf = Kdf { m_kib: 8 * 1024, t: 1, p: 1 };
    const PW: &str = "отпуск в Сочи";

    fn jpeg(n: usize, fill: u8) -> Vec<u8> {
        let mut d = vec![fill; n];
        d[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
        d
    }

    /// База с демо-данными и снимком чека у первой операции.
    fn demo(dir: &Path, name: &str, device: &str) -> Mutex<Store> {
        let mut s = Store::open(&dir.join(name), &[5u8; 32], device).unwrap();
        crate::book::ensure_defaults(&mut s).unwrap();
        s.append_all(crate::demo::book_events()).unwrap();
        let id = s.put_blob("image/jpeg", &jpeg(50_000, 7)).unwrap();
        let mut op = s.ops().unwrap().remove(0);
        op.att = vec![crate::trip::Attachment { id, mime: "image/jpeg".into(), size: 50_000, name: "чек.jpg".into() }];
        s.put_op(op).unwrap();
        Mutex::new(s)
    }

    fn fresh(dir: &Path, name: &str, device: &str) -> Mutex<Store> {
        let mut s = Store::open(&dir.join(name), &[6u8; 32], device).unwrap();
        crate::book::ensure_defaults(&mut s).unwrap();
        Mutex::new(s)
    }

    fn round(cloud: &dyn Cloud, store: &Mutex<Store>, key: &CloudKey, state: &mut State) -> Result<Report> {
        let cancel = AtomicBool::new(false);
        sync(&Round { cloud, store, key, now: 1_762_600_000, cancel: &cancel }, state, &mut |_| {})
    }

    fn same_data(a: &Mutex<Store>, b: &Mutex<Store>) {
        let (a, b) = (lock(a), lock(b));
        assert_eq!(serde_json::to_value(a.ops().unwrap()).unwrap(), serde_json::to_value(b.ops().unwrap()).unwrap());
        assert_eq!(serde_json::to_value(a.trips().unwrap()).unwrap(), serde_json::to_value(b.trips().unwrap()).unwrap());
        assert_eq!(serde_json::to_value(a.accounts().unwrap()).unwrap(), serde_json::to_value(b.accounts().unwrap()).unwrap());
        assert_eq!(a.version_vector().unwrap(), b.version_vector().unwrap());
    }

    #[test]
    fn names_are_canonical() {
        assert_eq!(parse_event_name("0a1b-1-500.kev"), Some(EventFileName { author: "0a1b".into(), from: 1, to: 500 }));
        for bad in ["0a1b-01-500.kev", "0a1b-0-5.kev", "0a1b-9-5.kev", "0a1b-1-5.kev.tmp", "a-b-1-5.kev", "-1-5.kev", "x/y-1-5.kev", "karman.key"] {
            assert_eq!(parse_event_name(bad), None, "{bad}");
        }
        let mk = CloudKey::generate().unwrap();
        let id = db::blob_id(b"x");
        let name = mk.blob_name(&id);
        assert_eq!(name.len(), 32 + 4);
        assert!(!name.contains(&id[..16]), "по имени не узнать отпечаток снимка");
        assert_eq!(name, mk.blob_name(&id));
        assert_ne!(name, CloudKey::generate().unwrap().blob_name(&id), "у другого облака другие имена");
    }

    #[test]
    fn two_devices_exchange_records_and_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let cloud = MemCloud::default();
        let a = demo(dir.path(), "a.db", "aaaa");
        let b = fresh(dir.path(), "b.db", "bbbb");
        let (ka, created) = join(&cloud, PW, FAST, 1).unwrap();
        assert!(created);
        assert!(matches!(join(&cloud, "чужой пароль", FAST, 1), Err(Error::CloudPassword)));
        let (kb, created) = join(&cloud, PW, FAST, 2).unwrap();
        assert!(!created);
        assert_eq!(ka.group(), kb.group());

        let (mut sa, mut sb) = (State::default(), State::default());
        let r = round(&cloud, &a, &ka, &mut sa).unwrap();
        let total = lock(&a).event_count().unwrap() as usize;
        assert_eq!((r.received, r.sent, r.files_sent), (0, total, 1));
        let r = round(&cloud, &b, &kb, &mut sb).unwrap();
        assert_eq!((r.received, r.files_received, r.files_waiting), (total, 1, 0));
        assert_eq!(r.sent, 3, "новое устройство отправляет свои три счёта по умолчанию");
        let r = round(&cloud, &a, &ka, &mut sa).unwrap();
        assert_eq!((r.received, r.sent), (3, 0));
        same_data(&a, &b);
        assert!(lock(&b).missing_blobs().unwrap().is_empty(), "снимок чека пришёл");

        // Запись на одном устройстве появляется на другом.
        let mut op = lock(&b).ops().unwrap().remove(1);
        op.title = "Такси до аэропорта".into();
        lock(&b).put_op(op).unwrap();
        assert_eq!(round(&cloud, &b, &kb, &mut sb).unwrap().sent, 1);
        assert_eq!(round(&cloud, &a, &ka, &mut sa).unwrap().received, 1);
        assert!(lock(&a).ops().unwrap().iter().any(|o| o.title == "Такси до аэропорта"));
        // Без изменений обмен ничего не гоняет.
        let r = round(&cloud, &a, &ka, &mut sa).unwrap();
        assert_eq!((r.received, r.sent, r.files_sent, r.files_received), (0, 0, 0, 0));
        same_data(&a, &b);
    }

    #[test]
    fn records_from_wifi_go_through_the_cloud_and_are_not_sent_twice() {
        let dir = tempfile::tempdir().unwrap();
        let cloud = MemCloud::default();
        let a = demo(dir.path(), "a.db", "aaaa");
        let b = fresh(dir.path(), "b.db", "bbbb");
        let c = fresh(dir.path(), "c.db", "cccc");
        // C связан с B только по Wi-Fi.
        let to_b = lock(&c).events_after(&lock(&b).version_vector().unwrap()).unwrap();
        lock(&b).apply(&to_b).unwrap();
        let (k, _) = join(&cloud, PW, FAST, 1).unwrap();
        let (mut sa, mut sb, mut sc) = (State::default(), State::default(), State::default());
        round(&cloud, &a, &k, &mut sa).unwrap();
        round(&cloud, &b, &k, &mut sb).unwrap();
        round(&cloud, &a, &k, &mut sa).unwrap();
        assert!(lock(&a).version_vector().unwrap().contains_key("cccc"), "записи C пришли через B");
        // Потом C подключает облако сам: своё он уже не отправляет.
        let r = round(&cloud, &c, &k, &mut sc).unwrap();
        assert_eq!(r.sent, 0);
        same_data(&a, &c);
    }

    #[test]
    fn small_files_are_merged_and_a_new_device_gets_everything() {
        let dir = tempfile::tempdir().unwrap();
        let cloud = MemCloud::default();
        let a = fresh(dir.path(), "a.db", "aaaa");
        let (k, _) = join(&cloud, PW, FAST, 1).unwrap();
        let mut sa = State::default();
        for i in 0..20 {
            lock(&a).append(EventBody::SettingPut { key: format!("k{i}"), value: i.to_string() }).unwrap();
            round(&cloud, &a, &k, &mut sa).unwrap();
        }
        let names = cloud.names(Dir::Events);
        assert!(names.len() <= COMPACT_AFTER + 1, "мелкие файлы слиты: {names:?}");
        assert!(names.contains(&event_name("aaaa", 1, 23)) || names.iter().any(|n| n.starts_with("aaaa-1-")));
        let b = fresh(dir.path(), "b.db", "bbbb");
        let mut sb = State::default();
        round(&cloud, &b, &k, &mut sb).unwrap();
        assert_eq!(lock(&b).version_vector().unwrap().get("aaaa"), Some(&23));
        assert_eq!(lock(&b).settings().unwrap().get("k19").map(String::as_str), Some("19"));
    }

    #[test]
    fn a_gap_in_the_cloud_is_not_applied_and_heals() {
        let dir = tempfile::tempdir().unwrap();
        let cloud = MemCloud::default();
        let a = fresh(dir.path(), "a.db", "aaaa");
        let (k, _) = join(&cloud, PW, FAST, 1).unwrap();
        let mut sa = State::default();
        round(&cloud, &a, &k, &mut sa).unwrap(); // aaaa-1-3
        for i in 0..2 {
            lock(&a).append(EventBody::SettingPut { key: format!("g{i}"), value: "1".into() }).unwrap();
            round(&cloud, &a, &k, &mut sa).unwrap(); // aaaa-4-4, aaaa-5-5
        }
        cloud.remove(Dir::Events, &event_name("aaaa", 4, 4));
        let b = fresh(dir.path(), "b.db", "bbbb");
        let mut sb = State::default();
        round(&cloud, &b, &k, &mut sb).unwrap();
        assert_eq!(lock(&b).version_vector().unwrap().get("aaaa"), Some(&3), "после пропуска ничего не применяется");
        // Устройство A видит пропуск и отправляет недостающее снова.
        assert_eq!(round(&cloud, &a, &k, &mut sa).unwrap().sent, 1);
        round(&cloud, &b, &k, &mut sb).unwrap();
        assert_eq!(lock(&b).version_vector().unwrap().get("aaaa"), Some(&5));
    }

    #[test]
    fn deleted_or_reconnected_cloud_stops_the_exchange() {
        let dir = tempfile::tempdir().unwrap();
        let cloud = MemCloud::default();
        let a = fresh(dir.path(), "a.db", "aaaa");
        let (k, _) = join(&cloud, PW, FAST, 1).unwrap();
        let mut sa = State::default();
        round(&cloud, &a, &k, &mut sa).unwrap();
        // Облако подключили заново на другом устройстве — с новым ключом.
        let cancel = AtomicBool::new(false);
        wipe(&cloud, &cancel).unwrap();
        assert!(cloud.names(Dir::Events).is_empty() && cloud.raw(Dir::Root, KEY_FILE).is_none());
        assert!(matches!(round(&cloud, &a, &k, &mut sa), Err(Error::Cloud(CloudFail::Gone, _))));
        let (k2, created) = join(&cloud, PW, FAST, 2).unwrap();
        assert!(created);
        assert!(matches!(round(&cloud, &a, &k, &mut sa), Err(Error::Cloud(CloudFail::Replaced, _))));
        // По паролю устройство получает новый ключ и снова обменивается.
        let (k3, created) = join(&cloud, PW, FAST, 3).unwrap();
        assert!(!created);
        assert_eq!(k3.group(), k2.group());
        let mut fresh_state = State::default();
        assert_eq!(round(&cloud, &a, &k3, &mut fresh_state).unwrap().sent, 3);
        // Новый пароль копий: ключ облака тот же, открывается новым паролем.
        let kf = k3.key_file("новый пароль копий", FAST, 4).unwrap();
        put_key_file(&cloud, &k3, &kf).unwrap();
        assert!(matches!(join(&cloud, PW, FAST, 5), Err(Error::CloudPassword)));
        let (k4, _) = join(&cloud, "новый пароль копий", FAST, 5).unwrap();
        assert_eq!(k4.to_meta(), k3.to_meta());
        assert!(matches!(put_key_file(&cloud, &k, &k.key_file(PW, FAST, 6).unwrap()), Err(Error::Cloud(CloudFail::Replaced, _))));
    }

    #[test]
    fn only_karman_files_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let cloud = MemCloud::default();
        let a = demo(dir.path(), "a.db", "aaaa");
        let (k, _) = join(&cloud, PW, FAST, 1).unwrap();
        round(&cloud, &a, &k, &mut State::default()).unwrap();
        // Свои файлы человека в папках «Кармана».
        for (d, n) in [(Dir::Events, "заметки.txt"), (Dir::Blobs, "фото.jpg"), (Dir::Events, "aaaa-01-3.kev"), (Dir::Root, "план.docx")] {
            cloud.set_raw(d, n, b"my file".to_vec());
        }
        let cancel = AtomicBool::new(false);
        wipe(&cloud, &cancel).unwrap();
        assert_eq!(cloud.names(Dir::Events), ["aaaa-01-3.kev", "заметки.txt"]);
        assert_eq!(cloud.names(Dir::Blobs), ["фото.jpg"]);
        assert_eq!(cloud.names(Dir::Root), ["план.docx"]);
        // Подключение заново тоже убирает только файлы «Кармана».
        round(&cloud, &a, &join(&cloud, PW, FAST, 2).unwrap().0, &mut State::default()).unwrap();
        cloud.remove(Dir::Root, KEY_FILE);
        join(&cloud, PW, FAST, 3).unwrap();
        assert_eq!(cloud.names(Dir::Events), ["aaaa-01-3.kev", "заметки.txt"]);
        assert_eq!(cloud.names(Dir::Blobs), ["фото.jpg"]);
    }

    #[test]
    fn same_device_number_with_other_records_is_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let cloud = MemCloud::default();
        let a = fresh(dir.path(), "a.db", "aaaa");
        lock(&a).append(EventBody::SettingPut { key: "rate".into(), value: "70000".into() }).unwrap();
        let (k, _) = join(&cloud, PW, FAST, 1).unwrap();
        let mut sa = State::default();
        round(&cloud, &a, &k, &mut sa).unwrap();
        // База начата заново под тем же номером устройства и с другой записью.
        let again = fresh(dir.path(), "a2.db", "aaaa");
        lock(&again).append(EventBody::SettingPut { key: "rate".into(), value: "90000".into() }).unwrap();
        let mut s2 = State::default();
        assert!(matches!(round(&cloud, &again, &k, &mut s2), Err(Error::Cloud(CloudFail::Conflict, _))));
        assert_eq!(lock(&again).settings().unwrap().get("rate").map(String::as_str), Some("90000"), "база не тронута");
        // Та же база, что отправляла, расхождений не находит.
        let mut s3 = State::default();
        assert!(round(&cloud, &a, &k, &mut s3).is_ok());
    }

    #[test]
    fn cancel_stops_between_requests() {
        let dir = tempfile::tempdir().unwrap();
        let cloud = MemCloud::default();
        let a = demo(dir.path(), "a.db", "aaaa");
        let (k, _) = join(&cloud, PW, FAST, 1).unwrap();
        let cancel = AtomicBool::new(true);
        let mut st = State::default();
        let r = sync(&Round { cloud: &cloud, store: &a, key: &k, now: 1, cancel: &cancel }, &mut st, &mut |_| {});
        assert!(matches!(r, Err(Error::Cloud(CloudFail::Cancelled, _))));
        assert!(cloud.names(Dir::Events).is_empty());
    }
}
