//! Резервная копия «Кармана»: весь журнал событий и снимки чеков в одном файле, зашифрованном паролем копии.
//!
//! Формат файла — общий из kit (`kit_core::backup`): заголовок со своей меткой `KARMANBK`, ключ копий
//! (Argon2id от пароля), шифрование кусками по 1 МБ, сведения о копии, пачки событий, вложения и итог.
//! Восстановление добавляет события к базе так же, как синхронизация: повторы пропускаются, при споре
//! побеждает более позднее изменение.

use crate::db::Store;
use crate::error::Result;
use crate::sync::{Event, VersionVector};
use kit_core::backup::{self as kit, Format};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;

pub use kit::{check_password, derive_raw, random, wipe, BackupKey, Kdf, MadeBy, FORMAT, MIN_PASSWORD};

/// Метка в начале файла копии. **Менять нельзя:** старые копии перестанут узнаваться.
pub const MAGIC: &[u8; 8] = b"KARMANBK";
/// Расширение файла копии.
pub const EXT: &str = "karman";

/// Копии «Кармана» для формата kit.
pub const BACKUP: Format = Format { magic: MAGIC, ext: EXT, names: crate::NAMES, common: &["karman", "карман"], records: &[] };

/// Новый пароль копий: не короче восьми символов и не из самых простых (в том числе не «карман») — им
/// шифруются копии и данные в облаке.
pub fn check_new_password(password: &str) -> Result<()> {
    Ok(kit::check_new_password(&BACKUP, password)?)
}

/// Самые простые пароли: возвращает, чем плох пароль.
pub fn weak_password(password: &str) -> Option<&'static str> {
    kit::weak_password(&BACKUP, password)
}

/// Файл начинается как копия «Кармана».
pub fn is_backup(path: &Path) -> bool {
    kit::is_backup(&BACKUP, path)
}

/// Счётчики «Кармана» в сведениях о копии.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub ops: u64,
    pub trips: u64,
}

/// Сведения о копии — первая запись. По ним видно, что восстанавливается.
pub type Manifest = kit::Manifest<Counts>;

/// Сведения о копии «Кармана»: сколько событий, снимков, операций и поездок.
pub fn manifest(by: &MadeBy, events: u64, blobs: u64, ops: u64, trips: u64) -> Manifest {
    Manifest::new(by, events, blobs, Counts { ops, trips })
}

/// Что записано.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Made {
    pub events: u64,
    pub blobs: u64,
    pub bytes: u64,
}

/// Файл копии пишется по частям: события, затем вложения по одному — в памяти не больше куска.
pub struct Writer<W: Write>(kit::Writer<W>);

impl<W: Write> Writer<W> {
    pub fn new(out: W, key: &BackupKey, manifest: &Manifest) -> Result<Writer<W>> {
        Ok(Writer(kit::Writer::new(&BACKUP, out, key, manifest)?))
    }

    pub fn events(&mut self, events: &[Event]) -> Result<()> {
        Ok(self.0.events(events)?)
    }

    pub fn blob(&mut self, id: &str, mime: &str, data: &[u8]) -> Result<()> {
        Ok(self.0.blob(id, mime, data)?)
    }

    /// Итог и последний кусок. Возвращает, сколько записано, и куда писали.
    pub fn finish(self) -> Result<(Made, W)> {
        let (m, out) = self.0.finish()?;
        Ok((Made { events: m.events, blobs: m.blobs, bytes: m.bytes }, out))
    }
}

/// Что встретилось в копии.
pub enum Item {
    Events(Vec<Event>),
    Blob { id: String, mime: String, data: Vec<u8> },
}

/// Чтение копии: ключ проверяется сразу, куски — по мере чтения.
pub struct Reader<R: Read> {
    inner: kit::Reader<R, Counts>,
    pub manifest: Manifest,
    pub events: u64,
    pub blobs: u64,
}

impl<R: Read> Reader<R> {
    /// Открывает копию паролем (ключ считается заново — около секунды).
    pub fn open(input: R, password: &str) -> Result<Reader<R>> {
        Ok(Reader::wrap(kit::Reader::open(&BACKUP, input, password)?))
    }

    /// Открывает копию сохранённым ключом — проверка только что записанной копии.
    pub fn open_with_key(input: R, key: &BackupKey) -> Result<Reader<R>> {
        Ok(Reader::wrap(kit::Reader::open_with_key(&BACKUP, input, key)?))
    }

    fn wrap(inner: kit::Reader<R, Counts>) -> Reader<R> {
        let manifest = inner.manifest.clone();
        Reader { inner, manifest, events: 0, blobs: 0 }
    }

    /// Следующая запись: пачка событий или вложение. `None` — копия прочитана целиком и сошлась.
    pub fn next_item(&mut self) -> Result<Option<Item>> {
        loop {
            let item = self.inner.next_item::<Event>()?;
            (self.events, self.blobs) = (self.inner.events, self.inner.blobs);
            return Ok(match item {
                None => None,
                Some(kit::Item::Events(events)) => Some(Item::Events(events)),
                Some(kit::Item::Blob { id, mime, data }) => Some(Item::Blob { id, mime, data }),
                // Своих записей у «Кармана» нет: kit их и не отдаёт.
                Some(kit::Item::Custom { .. }) => continue,
            });
        }
    }
}

/// Итог проверки: копия прочитана целиком.
#[derive(Clone, Debug, Serialize)]
pub struct Checked {
    pub manifest: Manifest,
    pub events: u64,
    pub blobs: u64,
    /// События этого же устройства, которые расходятся с базой (копия с устройства,
    /// база которого была начата заново под тем же номером).
    pub conflicts: u64,
}

/// Читает копию целиком, ничего не меняя. `conflict` получает события этого устройства
/// и говорит, расходятся ли они с базой.
pub fn verify<R: Read>(mut r: Reader<R>, device: &str, mut conflict: impl FnMut(&Event) -> Result<bool>) -> Result<Checked> {
    let mut conflicts = 0;
    while let Some(item) = r.next_item()? {
        if let Item::Events(events) = item {
            for ev in events.iter().filter(|e| e.device == device) {
                if conflict(ev)? {
                    conflicts += 1;
                }
            }
        }
    }
    Ok(Checked { manifest: r.manifest.clone(), events: r.events, blobs: r.blobs, conflicts })
}

/// Событие этого устройства расходится с базой: под тем же номером записано другое.
pub fn conflicts_with(store: &Store, ev: &Event) -> Result<bool> {
    Ok(store.event_body(&ev.device, ev.seq)?.is_some_and(|b| b != ev.body))
}

/// Сколько добавилось при восстановлении.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Restored {
    pub events: u64,
    pub blobs: u64,
}

/// Копия всей базы: события и вложения, на которые ссылаются операции.
pub fn write_store<W: Write>(store: &Store, key: &BackupKey, by: &MadeBy, out: W) -> Result<Made> {
    let events = store.events_after(&VersionVector::new())?;
    let blobs = store.present_blobs()?;
    let manifest = manifest(by, events.len() as u64, blobs.len() as u64, store.ops()?.len() as u64, store.trips()?.len() as u64);
    let mut w = Writer::new(out, key, &manifest)?;
    w.events(&events)?;
    for id in &blobs {
        if let Some((mime, data)) = store.blob(id)? {
            w.blob(id, &mime, &data)?;
        }
    }
    Ok(w.finish()?.0)
}

/// Добавляет копию к базе: события — как при синхронизации, вложения — если их нет.
/// Копию нужно сначала проверить целиком (`verify`): здесь записи применяются по мере чтения.
pub fn restore_into<R: Read>(mut r: Reader<R>, store: &mut Store) -> Result<Restored> {
    let mut out = Restored::default();
    while let Some(item) = r.next_item()? {
        match item {
            Item::Events(events) => out.events += store.apply(&events)? as u64,
            Item::Blob { id, mime, data } => {
                if !store.has_blob(&id)? {
                    store.put_blob_checked(&id, &mime, &data)?;
                    out.blobs += 1;
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::EventBody;
    use std::io::Cursor;

    const FAST: Kdf = Kdf { m_kib: 8 * 1024, t: 1, p: 1 };

    fn by() -> MadeBy {
        MadeBy { app: "0.9.0".into(), created: 1_762_540_200, device: "Ноутбук".into(), device_id: "dev-a".into() }
    }

    fn jpeg(n: usize, fill: u8) -> Vec<u8> {
        let mut d = vec![fill; n];
        d[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
        d
    }

    /// База с демо-данными и двумя снимками чеков, один — больше куска.
    fn demo_store(dir: &Path, name: &str, device: &str) -> Store {
        let mut s = Store::open(&dir.join(name), &[5u8; 32], device).unwrap();
        crate::book::ensure_defaults(&mut s).unwrap();
        s.append_all(crate::demo::book_events()).unwrap();
        let small = s.put_blob("image/jpeg", &jpeg(40_000, 7)).unwrap();
        let big = s.put_blob("image/jpeg", &jpeg((1 << 20) + 300_000, 9)).unwrap();
        let mut op = s.ops().unwrap().remove(0);
        op.att = vec![crate::trip::Attachment { id: small, mime: "image/jpeg".into(), size: 40_000, name: "чек.jpg".into() }, crate::trip::Attachment { id: big, mime: "image/jpeg".into(), size: ((1 << 20) + 300_000) as u64, name: "чек2.jpg".into() }];
        s.put_op(op).unwrap();
        s
    }

    fn backup_of(s: &Store, key: &BackupKey) -> Vec<u8> {
        let mut out = Vec::new();
        let made = write_store(s, key, &by(), &mut out).unwrap();
        assert_eq!(made.bytes as usize, out.len());
        assert_eq!(made.blobs, 2);
        out
    }

    #[test]
    fn restores_everything_on_a_new_device() {
        let dir = tempfile::tempdir().unwrap();
        let src = demo_store(dir.path(), "a.db", "dev-a");
        let key = BackupKey::new_with("отпуск в Сочи", FAST).unwrap();
        let file = backup_of(&src, &key);
        assert_eq!(&file[..8], MAGIC);

        let checked = verify(Reader::open(Cursor::new(&file), "отпуск в Сочи").unwrap(), "dev-b", |_| Ok(false)).unwrap();
        assert_eq!((checked.events, checked.blobs, checked.conflicts), (src.event_count().unwrap(), 2, 0));
        assert_eq!(checked.manifest.counts.ops, src.ops().unwrap().len() as u64);
        assert_eq!((checked.manifest.device.as_str(), checked.manifest.app.as_str()), ("Ноутбук", "0.9.0"));

        let mut dst = Store::open(&dir.path().join("b.db"), &[6u8; 32], "dev-b").unwrap();
        let r = restore_into(Reader::open(Cursor::new(&file), "отпуск в Сочи").unwrap(), &mut dst).unwrap();
        assert_eq!(r, Restored { events: src.event_count().unwrap(), blobs: 2 });
        assert_eq!(serde_json::to_value(dst.ops().unwrap()).unwrap(), serde_json::to_value(src.ops().unwrap()).unwrap());
        assert_eq!(dst.trips().unwrap().len(), src.trips().unwrap().len());
        assert_eq!(dst.accounts().unwrap().len(), src.accounts().unwrap().len());
        assert!(dst.missing_blobs().unwrap().is_empty(), "снимки чеков на месте");
        // Новые записи на новом устройстве идут под его номером и не спорят со старыми.
        dst.append(EventBody::SettingPut { key: "rate".into(), value: "80000".into() }).unwrap();
        assert_eq!(dst.version_vector().unwrap().get("dev-b"), Some(&1));
    }

    #[test]
    fn merge_adds_only_missing_and_keeps_newer_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut src = demo_store(dir.path(), "a.db", "dev-a");
        let key = BackupKey::new_with("отпуск в Сочи", FAST).unwrap();
        let file = backup_of(&src, &key);
        // После копии операцию поправили, а другую удалили — восстановление старой копии это не откатит.
        let mut op = src.ops().unwrap().remove(1);
        op.title = "Исправлено после копии".into();
        src.put_op(op.clone()).unwrap();
        let gone = src.ops().unwrap().last().unwrap().id.clone();
        src.delete_op(&gone).unwrap();
        let before = src.event_count().unwrap();
        let r = restore_into(Reader::open(Cursor::new(&file), "отпуск в Сочи").unwrap(), &mut src).unwrap();
        assert_eq!(r, Restored::default(), "всё из копии уже есть");
        assert_eq!(src.event_count().unwrap(), before);
        assert!(src.ops().unwrap().iter().any(|o| o.title == "Исправлено после копии"));
        assert!(!src.ops().unwrap().iter().any(|o| o.id == gone));
    }

    #[test]
    fn same_device_restarted_under_old_number_is_caught() {
        let dir = tempfile::tempdir().unwrap();
        let src = demo_store(dir.path(), "a.db", "dev-a");
        let key = BackupKey::new_with("отпуск в Сочи", FAST).unwrap();
        let file = backup_of(&src, &key);
        // База начата заново под тем же номером устройства и уже с новой записью.
        let mut fresh = Store::open(&dir.path().join("c.db"), &[7u8; 32], "dev-a").unwrap();
        fresh.append(EventBody::SettingPut { key: "rate".into(), value: "90000".into() }).unwrap();
        let checked = verify(Reader::open(Cursor::new(&file), "отпуск в Сочи").unwrap(), "dev-a", |ev| conflicts_with(&fresh, ev)).unwrap();
        assert_eq!(checked.conflicts, 1);
        // Та же база, что сделала копию, расхождений не находит.
        let checked = verify(Reader::open(Cursor::new(&file), "отпуск в Сочи").unwrap(), "dev-a", |ev| conflicts_with(&src, ev)).unwrap();
        assert_eq!(checked.conflicts, 0);
    }
}
