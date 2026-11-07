//! Зашифрованная база (SQLCipher) с журналом событий.
//!
//! Каждое изменение — событие с номером от устройства. Таблица записей
//! (операции, поездки, счета, настройки) собирается из событий: при споре
//! двух устройств побеждает более позднее изменение (по часам Лэмпорта,
//! при равенстве — по номеру устройства).

use crate::book::Account;
use crate::error::{Error, Result};
use crate::sync::{Event, EventBody, VersionVector};
use crate::trip::{Op, Trip};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Версия схемы. Меняется вместе с новыми видами событий: при открытии таблица
/// записей собирается заново, и события, которые прежняя версия не понимала, оживают.
const SCHEMA: &str = "3";

/// Вложения: не больше 15 МБ, только снимки и PDF.
pub const MAX_BLOB: usize = 15 * 1024 * 1024;
pub const BLOB_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "application/pdf"];

/// Номер вложения — SHA-256 содержимого.
pub fn blob_id(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

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
        let conn = Connection::open(path)?;
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex(key)))?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
            .map_err(|_| Error::WrongKey)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS events(
               device TEXT NOT NULL, seq INTEGER NOT NULL, lamport INTEGER NOT NULL, body TEXT NOT NULL,
               PRIMARY KEY(device, seq));
             CREATE TABLE IF NOT EXISTS items(
               kind TEXT NOT NULL, id TEXT NOT NULL, body TEXT, deleted INTEGER NOT NULL DEFAULT 0,
               lamport INTEGER NOT NULL, device TEXT NOT NULL,
               PRIMARY KEY(kind, id));
             CREATE TABLE IF NOT EXISTS blobs(
               id TEXT PRIMARY KEY, mime TEXT NOT NULL, size INTEGER NOT NULL, data BLOB NOT NULL,
               added INTEGER NOT NULL);",
        )?;
        let mut store = Store { conn, device: device.to_string() };
        if store.meta("schema")?.as_deref() != Some(SCHEMA) {
            store.rebuild()?;
            store.conn.execute_batch("DROP TABLE IF EXISTS ops;")?;
            store.set_meta("schema", SCHEMA)?;
        }
        Ok(store)
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
        let mut st = self.conn.prepare("SELECT device, seq, lamport, body FROM events ORDER BY lamport, device")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (device, seq, lamport, body) = row?;
            out.push(Event { device, seq: seq as u64, lamport: lamport as u64, body: serde_json::from_str(&body)? });
        }
        Ok(out)
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn cipher_version(&self) -> Result<String> {
        Ok(self.conn.query_row("PRAGMA cipher_version", [], |r| r.get(0))?)
    }

    fn max_lamport(&self) -> Result<u64> {
        Ok(self.conn.query_row("SELECT coalesce(max(lamport), 0) FROM events", [], |r| r.get::<_, i64>(0))? as u64)
    }

    fn next_seq(&self) -> Result<u64> {
        let s: i64 = self
            .conn
            .query_row("SELECT coalesce(max(seq), 0) FROM events WHERE device = ?1", [&self.device], |r| r.get(0))?;
        Ok(s as u64 + 1)
    }

    /// Записывает своё изменение.
    pub fn append(&mut self, body: EventBody) -> Result<Event> {
        Ok(self.append_all(vec![body])?.remove(0))
    }

    /// Несколько своих изменений одной транзакцией.
    pub fn append_all(&mut self, bodies: Vec<EventBody>) -> Result<Vec<Event>> {
        let mut seq = self.next_seq()?;
        let mut lamport = self.max_lamport()?;
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
        let mut st = self.conn.prepare("SELECT device, max(seq) FROM events GROUP BY device")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
        let mut vv = VersionVector::new();
        for row in rows {
            let (d, s) = row?;
            vv.insert(d, s);
        }
        Ok(vv)
    }

    /// События, которых нет у устройства с таким вектором версий.
    pub fn events_after(&self, known: &VersionVector) -> Result<Vec<Event>> {
        let mine = self.version_vector()?;
        let mut out = Vec::new();
        let mut st = self.conn.prepare("SELECT device, seq, lamport, body FROM events WHERE device = ?1 AND seq > ?2 ORDER BY seq")?;
        for (device, max) in mine {
            let from = known.get(&device).copied().unwrap_or(0);
            if from >= max {
                continue;
            }
            let rows = st.query_map(params![device, from as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
            })?;
            for row in rows {
                let (device, seq, lamport, body) = row?;
                out.push(Event { device, seq: seq as u64, lamport: lamport as u64, body: serde_json::from_str(&body)? });
            }
        }
        Ok(out)
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

    pub fn event_count(&self) -> Result<u64> {
        Ok(self.conn.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))? as u64)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
        Ok(())
    }

    /// Сохраняет вложение и возвращает его номер (SHA-256). Повтор того же файла ничего не меняет.
    pub fn put_blob(&self, mime: &str, data: &[u8]) -> Result<String> {
        check_blob(mime, data)?;
        let id = blob_id(data);
        self.conn.execute(
            "INSERT OR IGNORE INTO blobs(id, mime, size, data, added) VALUES(?1, ?2, ?3, ?4, CAST(strftime('%s','now') AS INTEGER))",
            params![id, mime, data.len() as i64, data],
        )?;
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
        Ok(self
            .conn
            .query_row("SELECT mime, data FROM blobs WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    pub fn has_blob(&self, id: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT count(*) FROM blobs WHERE id = ?1", [id], |r| r.get::<_, i64>(0))? > 0)
    }

    /// Вложения, на которые ссылаются операции.
    pub fn referenced_blobs(&self) -> Result<BTreeSet<String>> {
        Ok(self.ops()?.into_iter().flat_map(|o| o.att.into_iter().map(|a| a.id)).collect())
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
    let n = tx.execute(
        "INSERT OR IGNORE INTO events(device, seq, lamport, body) VALUES(?1, ?2, ?3, ?4)",
        params![ev.device, ev.seq as i64, ev.lamport as i64, serde_json::to_string(&ev.body)?],
    )?;
    Ok(n == 1)
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
}
