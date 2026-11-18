//! Зашифрованная база (SQLCipher) с журналом событий.
//!
//! Каждое изменение — событие с номером от устройства. Таблица записей
//! (операции, поездки, счета, настройки) собирается из событий: при споре
//! двух устройств побеждает более позднее изменение (по часам Лэмпорта,
//! при равенстве — по номеру устройства). Открытие базы, таблицы, `meta`, выборки журнала
//! и вложения — общие, из kit (`kit_core::db`).

use crate::book::Account;
use crate::error::{Error, Result};
use crate::budget::Budget;
use crate::finance::{Debt, Plan};
use crate::savings::Goal;
use crate::sync::{Event, EventBody, VersionVector};
use crate::trip::{Op, Trip};
use kit_core::db;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Версия схемы. Меняется вместе с новыми видами событий: при открытии таблица
/// записей собирается заново, и события, которые прежняя версия не понимала, оживают.
const SCHEMA: &str = "7";

/// Вложения: не больше 15 МБ, только снимки и PDF.
pub const MAX_BLOB: usize = 15 * 1024 * 1024;
pub const BLOB_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "application/pdf"];

/// Номер вложения — SHA-256 содержимого.
pub use kit_core::cloud::blob_id;

/// Проверяет размер, тип и первые байты файла: под видом снимка не пролезет что-то другое.
pub fn check_blob(mime: &str, data: &[u8]) -> Result<()> {
    if data.is_empty() || data.len() > MAX_BLOB {
        return Err(Error::Format(format!("файл больше {} МБ или пустой", MAX_BLOB / 1024 / 1024)));
    }
    let ok = match mime {
        "image/jpeg" => data.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/png" => data.starts_with(&[0x89, b'P', b'N', b'G']),
        "image/webp" => data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP",
        "application/pdf" => data.starts_with(b"%PDF"),
        _ => false,
    };
    if !ok {
        return Err(Error::Format("подходят только снимки JPEG, PNG, WebP и PDF".into()));
    }
    Ok(())
}

pub struct Store {
    conn: Connection,
    device: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Обычный файл SQLite начинается с «SQLite format 3». Зашифрованный — нет.
pub fn is_plaintext_sqlite(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let mut f = std::fs::File::open(path)?;
    let n = f.read(&mut head)?;
    Ok(n == 16 && &head == b"SQLite format 3\0")
}

impl Store {
    /// Открывает или создаёт базу, зашифрованную 256-битным ключом.
    pub fn open(path: &Path, key: &[u8; 32], device: &str) -> Result<Store> {
        // Ключ, таблицы — общие (kit); строка с ключом после использования затирается.
        let conn = db::open(path, key).map_err(|e| match e {
            db::Open::WrongKey => Error::WrongKey,
            db::Open::Db(e) => e.into(),
        })?;
        let mut store = Store { conn, device: device.to_string() };
        let schema = store.meta("schema")?;
        if schema.as_deref() != Some(SCHEMA) {
            // Перед перестройкой — копия файла базы рядом (тем же ключом): если новая версия
            // ошибётся, прежнее состояние можно вернуть. Хранятся две последние.
            if let Some(old) = schema {
                store.copy_before_rebuild(path, key, &old)?;
            }
            store.rebuild()?;
            store.conn.execute_batch("DROP TABLE IF EXISTS ops;")?;
            store.set_meta("schema", SCHEMA)?;
        }
        Ok(store)
    }

    fn copy_before_rebuild(&self, path: &Path, key: &[u8; 32], old: &str) -> Result<()> {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return Ok(()) };
        let dir = path.parent().unwrap_or(Path::new("."));
        let safe: String = old.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
        self.export_copy(&dir.join(format!("{name}.schema-{safe}.bak")), key)?;
        let prefix = format!("{name}.schema-");
        let mut copies: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".bak")))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        copies.sort();
        while copies.len() > 2 {
            let _ = std::fs::remove_file(copies.remove(0).1);
        }
        Ok(())
    }

    /// Копия базы в файл тем же ключом (sqlcipher_export).
    pub fn export_copy(&self, dest: &Path, key: &[u8; 32]) -> Result<()> {
        let _ = std::fs::remove_file(dest);
        let mut raw = format!("x'{}'", hex(key)).into_bytes();
        let attached = self.conn.execute("ATTACH DATABASE ?1 AS copy KEY ?2", params![dest.to_string_lossy(), std::str::from_utf8(&raw).unwrap_or_default()]);
        raw.fill(0);
        std::hint::black_box(&raw);
        attached?;
        let done = self.conn.query_row("SELECT sqlcipher_export('copy')", [], |_| Ok(()));
        self.conn.execute("DETACH DATABASE copy", [])?;
        done?;
        Ok(())
    }

    /// Собирает таблицу записей заново из журнала событий.
    pub fn rebuild(&mut self) -> Result<()> {
        let events = self.all_events()?;
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM items", [])?;
        for ev in &events {
            materialize(&tx, ev)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn all_events(&self) -> Result<Vec<Event>> {
        db::events_by_lamport(&self.conn)?.into_iter().map(event).collect()
    }

    /// Сколько событий пришло из более новой версии и пока не показывается.
    pub fn unknown_events(&self) -> Result<usize> {
        Ok(self.all_events()?.iter().filter(|e| e.parsed().is_none()).count())
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn cipher_version(&self) -> Result<String> {
        Ok(self.conn.query_row("PRAGMA cipher_version", [], |r| r.get(0))?)
    }

    /// Записывает своё изменение.
    pub fn append(&mut self, body: EventBody) -> Result<Event> {
        Ok(self.append_all(vec![body])?.remove(0))
    }

    /// Несколько своих изменений одной транзакцией.
    pub fn append_all(&mut self, bodies: Vec<EventBody>) -> Result<Vec<Event>> {
        let (seq, lamport) = db::next(&self.conn, &self.device)?;
        let (mut seq, mut lamport) = (seq as u64, lamport as u64 - 1);
        let tx = self.conn.transaction()?;
        let mut out = Vec::with_capacity(bodies.len());
        for body in bodies {
            lamport += 1;
            let ev = Event::new(self.device.clone(), seq, lamport, &body)?;
            seq += 1;
            insert_event(&tx, &ev)?;
            materialize(&tx, &ev)?;
            out.push(ev);
        }
        tx.commit()?;
        Ok(out)
    }

    pub fn put_op(&mut self, op: Op) -> Result<Event> {
        self.append(EventBody::OpPut { op })
    }

    pub fn delete_op(&mut self, id: &str) -> Result<Event> {
        self.append(EventBody::OpDelete { id: id.to_string() })
    }

    /// Применяет чужие события. Повторы пропускаются. Возвращает число новых.
    pub fn apply(&mut self, events: &[Event]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        for ev in events {
            if insert_event(&tx, ev)? {
                materialize(&tx, ev)?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// Последний известный номер события от каждого устройства.
    pub fn version_vector(&self) -> Result<VersionVector> {
        Ok(db::versions(&self.conn)?.into_iter().map(|(d, s)| (d, s as u64)).collect())
    }

    /// События, которых нет у устройства с таким вектором версий.
    pub fn events_after(&self, known: &VersionVector) -> Result<Vec<Event>> {
        let mut out = Vec::new();
        for (device, max) in self.version_vector()? {
            let from = known.get(&device).copied().unwrap_or(0);
            if from >= max {
                continue;
            }
            for r in db::events_after(&self.conn, &device, from as i64)? {
                out.push(event(r)?);
            }
        }
        Ok(out)
    }

    /// События одного устройства с номерами from..=to, по порядку.
    pub fn events_range(&self, device: &str, from: u64, to: u64) -> Result<Vec<Event>> {
        db::events_range(&self.conn, device, from as i64, to as i64)?.into_iter().map(event).collect()
    }

    fn items<T: DeserializeOwned>(&self, kind: &str) -> Result<Vec<T>> {
        let mut st = self.conn.prepare("SELECT body FROM items WHERE kind = ?1 AND deleted = 0 AND body IS NOT NULL")?;
        let rows = st.query_map([kind], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for b in rows {
            out.push(serde_json::from_str(&b?)?);
        }
        Ok(out)
    }

    /// Операции, новые сверху.
    pub fn ops(&self) -> Result<Vec<Op>> {
        let mut out: Vec<Op> = self.items("op")?;
        out.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.id.cmp(&a.id)));
        Ok(out)
    }

    /// Долги: сначала с ближайшим сроком.
    pub fn debts(&self) -> Result<Vec<Debt>> {
        let mut out: Vec<Debt> = self.items("debt")?;
        out.sort_by(|a, b| a.due.is_none().cmp(&b.due.is_none()).then(a.due.cmp(&b.due)).then_with(|| a.date.cmp(&b.date)).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Регулярные платежи по дате начала.
    pub fn plans(&self) -> Result<Vec<Plan>> {
        let mut out: Vec<Plan> = self.items("plan")?;
        out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Цели: сначала с ближайшим сроком.
    pub fn goals(&self) -> Result<Vec<Goal>> {
        let mut out: Vec<Goal> = self.items("goal")?;
        out.sort_by(|a, b| a.done.cmp(&b.done).then(a.due.is_none().cmp(&b.due.is_none())).then(a.due.cmp(&b.due)).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Бюджеты: личный и семейный.
    pub fn budgets(&self) -> Result<Vec<Budget>> {
        self.items("budget")
    }

    /// Свои категории личных трат, по названию.
    pub fn categories(&self) -> Result<Vec<crate::category::Category>> {
        let mut out: Vec<crate::category::Category> = self.items("category")?;
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Наряды всех поездок.
    pub fn orders(&self) -> Result<Vec<crate::overtime::WorkOrder>> {
        let mut out: Vec<crate::overtime::WorkOrder> = self.items("order")?;
        out.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Рабочие дни всех поездок.
    pub fn work_days(&self) -> Result<Vec<crate::overtime::WorkDay>> {
        let mut out: Vec<crate::overtime::WorkDay> = self.items("workday")?;
        out.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.trip.cmp(&b.trip)));
        Ok(out)
    }

    /// Операция с таким номером есть или была (удалена) — на этом устройстве или пришла с другого.
    pub fn knows_op(&self, id: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT count(*) FROM items WHERE kind = 'op' AND id = ?1", [id], |r| r.get::<_, i64>(0))? > 0)
    }

    /// Поездки, новые сверху.
    pub fn trips(&self) -> Result<Vec<Trip>> {
        let mut out: Vec<Trip> = self.items("trip")?;
        out.sort_by(|a, b| b.start.cmp(&a.start).then_with(|| b.id.cmp(&a.id)));
        Ok(out)
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut out: Vec<Account> = self.items("account")?;
        out.sort_by_key(|a| a.order);
        Ok(out)
    }

    pub fn settings(&self) -> Result<BTreeMap<String, String>> {
        let mut st = self.conn.prepare("SELECT id, body FROM items WHERE kind = 'setting' AND deleted = 0 AND body IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (k, v) = row?;
            out.insert(k, serde_json::from_str::<String>(&v)?);
        }
        Ok(out)
    }

    /// Тело события, если оно есть в журнале.
    pub fn event_body(&self, device: &str, seq: u64) -> Result<Option<serde_json::Value>> {
        Ok(match db::event_body(&self.conn, device, seq as i64)? {
            Some(b) => Some(serde_json::from_str(&b)?),
            None => None,
        })
    }

    pub fn event_count(&self) -> Result<u64> {
        Ok(db::event_count(&self.conn)?)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(db::meta(&self.conn, key)?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        Ok(db::set_meta(&self.conn, key, value)?)
    }

    /// Сохраняет вложение и возвращает его номер (SHA-256). Повтор того же файла ничего не меняет.
    pub fn put_blob(&self, mime: &str, data: &[u8]) -> Result<String> {
        check_blob(mime, data)?;
        let id = blob_id(data);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        db::insert_blob(&self.conn, &id, mime, data, now)?;
        Ok(id)
    }

    /// Вложение, полученное с другого устройства: номер обязан совпасть с содержимым.
    pub fn put_blob_checked(&self, id: &str, mime: &str, data: &[u8]) -> Result<()> {
        if blob_id(data) != id {
            return Err(Error::Format("вложение повреждено: не совпал отпечаток".into()));
        }
        self.put_blob(mime, data).map(|_| ())
    }

    pub fn blob(&self, id: &str) -> Result<Option<(String, Vec<u8>)>> {
        Ok(db::blob(&self.conn, id)?)
    }

    pub fn has_blob(&self, id: &str) -> Result<bool> {
        Ok(db::has_blob(&self.conn, id)?)
    }

    /// Вложения, на которые ссылаются операции и наряды.
    pub fn referenced_blobs(&self) -> Result<BTreeSet<String>> {
        let ops = self.ops()?.into_iter().flat_map(|o| o.att.into_iter().map(|a| a.id));
        let orders = self.orders()?.into_iter().flat_map(|o| o.att.into_iter().map(|a| a.id));
        Ok(ops.chain(orders).collect())
    }

    /// Вложения из операций, которые есть на этом устройстве.
    pub fn present_blobs(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for id in self.referenced_blobs()? {
            if self.has_blob(&id)? {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Вложения из операций, которых на этом устройстве ещё нет.
    pub fn missing_blobs(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for id in self.referenced_blobs()? {
            if !self.has_blob(&id)? {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Удаляет вложения, на которые больше суток никто не ссылается
    /// (сутки — запас на снимок, который ещё не прикрепили к операции).
    pub fn gc_blobs(&self) -> Result<usize> {
        let keep = self.referenced_blobs()?;
        let mut st = self.conn.prepare("SELECT id FROM blobs WHERE added < CAST(strftime('%s','now') AS INTEGER) - 86400")?;
        let old: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        let mut n = 0;
        for id in old.into_iter().filter(|id| !keep.contains(id)) {
            n += self.conn.execute("DELETE FROM blobs WHERE id = ?1", [&id])?;
        }
        Ok(n)
    }

    /// Для проверки скорости: пачка строк в одной транзакции в отдельной таблице.
    pub fn bench_rows(&mut self, n: usize) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS bench(id INTEGER PRIMARY KEY, title TEXT NOT NULL, amount INTEGER NOT NULL)")?;
        {
            let mut st = tx.prepare("INSERT INTO bench(title, amount) VALUES(?1, ?2)")?;
            for i in 0..n {
                st.execute(params![format!("Проверка {i}"), (i as i64) * 100])?;
            }
        }
        tx.commit()?;
        Ok(self.conn.query_row("SELECT count(*) FROM bench", [], |r| r.get::<_, i64>(0))? as usize)
    }
}

fn insert_event(tx: &Transaction, ev: &Event) -> Result<bool> {
    Ok(db::insert_event(tx, &ev.device, ev.seq as i64, ev.lamport as i64, &serde_json::to_string(&ev.body)?)?)
}

/// Событие журнала из строки базы.
fn event(r: db::Row) -> Result<Event> {
    Ok(Event { device: r.device, seq: r.seq as u64, lamport: r.lamport as u64, body: serde_json::from_str(&r.body)? })
}

fn materialize(tx: &Transaction, ev: &Event) -> Result<()> {
    // Событие из более новой версии хранится в журнале и оживёт после обновления.
    let Some(parsed) = ev.parsed() else { return Ok(()) };
    let (kind, id, body) = parsed.item()?;
    let cur: Option<(i64, String)> = tx
        .query_row("SELECT lamport, device FROM items WHERE kind = ?1 AND id = ?2", params![kind, id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let newer = match cur {
        None => true,
        Some((l, d)) => (ev.lamport as i64, ev.device.as_str()) > (l, d.as_str()),
    };
    if newer {
        let deleted = body.is_none() as i64;
        tx.execute(
            "INSERT INTO items(kind, id, body, deleted, lamport, device) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(kind, id) DO UPDATE SET body = excluded.body, deleted = excluded.deleted, lamport = excluded.lamport, device = excluded.device",
            params![kind, id, body, deleted, ev.lamport as i64, ev.device],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    #[test]
    fn encrypted_on_disk_and_needs_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("karman.db");
        let key = [42u8; 32];
        {
            let mut s = Store::open(&path, &key, "dev-a").unwrap();
            for op in demo::ops().into_iter().take(5) {
                s.put_op(op).unwrap();
            }
            assert!(s.cipher_version().unwrap().starts_with('4'));
        }
        assert!(!is_plaintext_sqlite(&path).unwrap(), "файл не должен читаться как обычный SQLite");
        let s = Store::open(&path, &key, "dev-a").unwrap();
        assert_eq!(s.ops().unwrap().len(), 5);
        assert!(matches!(Store::open(&path, &[1u8; 32], "dev-a"), Err(Error::WrongKey)));
    }

    #[test]
    fn copy_of_the_base_before_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("karman.db");
        let key = [9u8; 32];
        {
            let mut s = Store::open(&path, &key, "dev-a").unwrap();
            for op in demo::ops().into_iter().take(3) {
                s.put_op(op).unwrap();
            }
        }
        assert!(!dir.path().join("karman.db.schema-5.bak").exists(), "новой базе копия не нужна");
        // Будто база осталась от прежней версии: при открытии она перестраивается — сначала копия.
        for old in ["4", "5", "x"] {
            Store::open(&path, &key, "dev-a").unwrap().set_meta("schema", old).unwrap();
            let s = Store::open(&path, &key, "dev-a").unwrap();
            assert_eq!(s.ops().unwrap().len(), 3);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let copies: Vec<String> = std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.contains(".schema-")).collect();
        assert_eq!(copies.len(), 2, "хранятся две последние: {copies:?}");
        assert!(!copies.iter().any(|n| n.ends_with("-4.bak")));
        let copy = Store::open(&dir.path().join("karman.db.schema-x.bak"), &key, "dev-a").unwrap();
        assert_eq!(copy.ops().unwrap().len(), 3, "копия открывается тем же ключом");
        assert!(!is_plaintext_sqlite(&dir.path().join("karman.db.schema-x.bak")).unwrap(), "и зашифрована");
        assert!(Store::open(&dir.path().join("karman.db.schema-x.bak"), &[1u8; 32], "dev-a").is_err());
    }

    #[test]
    fn last_writer_wins_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("a.db"), &[1u8; 32], "dev-a").unwrap();
        let mut op = demo::ops().remove(0);
        s.put_op(op.clone()).unwrap();
        op.title = "Такси до объекта, исправлено".into();
        s.put_op(op.clone()).unwrap();
        assert_eq!(s.ops().unwrap()[0].title, "Такси до объекта, исправлено");
        // Старое чужое событие не перетирает новое.
        let stale = Event::new("dev-b".into(), 1, 1, &EventBody::OpPut { op: demo::ops().remove(0) }).unwrap();
        assert_eq!(s.apply(&[stale.clone()]).unwrap(), 1);
        assert_eq!(s.apply(&[stale]).unwrap(), 0, "повтор пропускается");
        assert_eq!(s.ops().unwrap()[0].title, "Такси до объекта, исправлено");
        s.delete_op(&op.id).unwrap();
        assert!(s.ops().unwrap().is_empty());
        assert_eq!(s.version_vector().unwrap().get("dev-a"), Some(&3));
    }

    #[test]
    fn trips_accounts_settings_and_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.db");
        let key = [3u8; 32];
        {
            let mut s = Store::open(&path, &key, "dev-a").unwrap();
            let trips = demo::trips();
            let bodies: Vec<EventBody> = trips.iter().cloned().map(|trip| EventBody::TripPut { trip }).collect();
            s.append_all(bodies).unwrap();
            s.append(EventBody::SettingPut { key: "rate".into(), value: "80000".into() }).unwrap();
            s.append(EventBody::TripDelete { id: "540".into() }).unwrap();
            crate::book::ensure_defaults(&mut s).unwrap();
            assert_eq!(s.trips().unwrap().len(), 4);
            assert_eq!(s.settings().unwrap().get("rate").map(String::as_str), Some("80000"));
            assert_eq!(s.accounts().unwrap().len(), 3);
            // Таблица записей собирается из журнала один в один.
            s.rebuild().unwrap();
            assert_eq!(s.trips().unwrap().len(), 4);
        }
        let s = Store::open(&path, &key, "dev-a").unwrap();
        assert_eq!(s.trips().unwrap()[0].id, "588", "новые поездки сверху");
        assert_eq!(s.accounts().unwrap()[0].id, "acc-work");
    }

    #[test]
    fn order_photos_are_kept_until_the_order_is_deleted() {
        use crate::overtime::WorkOrder;
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("o.db"), &[5u8; 32], "dev-a").unwrap();
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpeg.extend_from_slice(&[7u8; 2000]);
        let id = s.put_blob("image/jpeg", &jpeg).unwrap();
        let order = WorkOrder {
            id: "ord-1".into(),
            trip: "573".into(),
            number: "3-2/23".into(),
            object: String::new(),
            from: crate::Date::parse("2026-10-23").unwrap(),
            to: crate::Date::parse("2026-10-26").unwrap(),
            att: vec![crate::trip::Attachment { id: id.clone(), mime: "image/jpeg".into(), name: "наряд.jpg".into(), size: jpeg.len() as u64 }],
            note: String::new(),
            extra: Default::default(),
        };
        s.append(EventBody::OrderPut { order }).unwrap();
        assert!(s.referenced_blobs().unwrap().contains(&id), "снимок наряда — в копиях и обмене");
        assert_eq!(s.present_blobs().unwrap(), vec![id.clone()]);
        // Снимок старше суток: пока наряд есть, он не удаляется.
        s.conn.execute("UPDATE blobs SET added = added - 3 * 86400", []).unwrap();
        assert_eq!(s.gc_blobs().unwrap(), 0);
        s.append(EventBody::OrderDelete { id: "ord-1".into() }).unwrap();
        assert_eq!(s.gc_blobs().unwrap(), 1, "наряд удалён — снимок больше не нужен");
        assert!(s.blob(&id).unwrap().is_none());
    }
}
