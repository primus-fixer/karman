//! Зашифрованная база (SQLCipher) с журналом событий.
//!
//! Каждое изменение — событие с номером от устройства. Таблица операций
//! собирается из событий: при споре двух устройств побеждает более позднее
//! изменение (по часам Лэмпорта, при равенстве — по номеру устройства).

use crate::error::{Error, Result};
use crate::sync::{Event, EventBody, VersionVector};
use crate::trip::Op;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::Path;

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
             CREATE TABLE IF NOT EXISTS ops(
               id TEXT PRIMARY KEY, body TEXT, deleted INTEGER NOT NULL DEFAULT 0,
               lamport INTEGER NOT NULL, device TEXT NOT NULL);",
        )?;
        Ok(Store { conn, device: device.to_string() })
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
        let ev = Event { device: self.device.clone(), seq: self.next_seq()?, lamport: self.max_lamport()? + 1, body };
        let tx = self.conn.transaction()?;
        insert_event(&tx, &ev)?;
        materialize(&tx, &ev)?;
        tx.commit()?;
        Ok(ev)
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

    pub fn ops(&self) -> Result<Vec<Op>> {
        let mut st = self.conn.prepare("SELECT body FROM ops WHERE deleted = 0 AND body IS NOT NULL")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for b in rows {
            out.push(serde_json::from_str(&b?)?);
        }
        out.sort_by(|a: &Op, b: &Op| b.date.cmp(&a.date).then_with(|| a.id.cmp(&b.id)));
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

    /// Для проверки скорости: пачка строк в одной транзакции во временной таблице.
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
    let (id, body, deleted) = match &ev.body {
        EventBody::OpPut { op } => (op.id.clone(), Some(serde_json::to_string(op)?), 0),
        EventBody::OpDelete { id } => (id.clone(), None, 1),
    };
    let cur: Option<(i64, String)> = tx
        .query_row("SELECT lamport, device FROM ops WHERE id = ?1", [&id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let newer = match cur {
        None => true,
        Some((l, d)) => (ev.lamport as i64, ev.device.as_str()) > (l, d.as_str()),
    };
    if newer {
        tx.execute(
            "INSERT INTO ops(id, body, deleted, lamport, device) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET body = excluded.body, deleted = excluded.deleted, lamport = excluded.lamport, device = excluded.device",
            params![id, body, deleted, ev.lamport as i64, ev.device],
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
        let stale = Event { device: "dev-b".into(), seq: 1, lamport: 1, body: EventBody::OpPut { op: demo::ops().remove(0) } };
        assert_eq!(s.apply(&[stale.clone()]).unwrap(), 1);
        assert_eq!(s.apply(&[stale]).unwrap(), 0, "повтор пропускается");
        assert_eq!(s.ops().unwrap()[0].title, "Такси до объекта, исправлено");
        s.delete_op(&op.id).unwrap();
        assert!(s.ops().unwrap().is_empty());
        assert_eq!(s.version_vector().unwrap().get("dev-a"), Some(&3));
    }
}
