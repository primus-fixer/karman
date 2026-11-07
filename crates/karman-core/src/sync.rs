//! Синхронизация: события, шифрование пакетов и обмен по локальной сети.
//!
//! Все каналы (Wi-Fi, облако, свой сервер) передают одни и те же
//! зашифрованные пакеты. Здесь — общий формат и обмен напрямую по TCP.

use crate::db::Store;
use crate::error::{Error, Result};
use crate::pairing::Pairing;
use crate::book::Account;
use crate::trip::{Op, Trip};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Последний известный номер события от каждого устройства.
pub type VersionVector = BTreeMap<String, u64>;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Event {
    pub device: String,
    pub seq: u64,
    pub lamport: u64,
    /// Тело события как JSON. Типы и поля из более новых версий хранятся
    /// и передаются дальше без потерь, даже если эта версия их не понимает.
    pub body: serde_json::Value,
}

impl Event {
    pub fn new(device: String, seq: u64, lamport: u64, body: &EventBody) -> crate::Result<Event> {
        Ok(Event { device, seq, lamport, body: serde_json::to_value(body)? })
    }

    /// Тело, понятное этой версии; None — событие из более новой версии.
    pub fn parsed(&self) -> Option<EventBody> {
        serde_json::from_value(self.body.clone()).ok()
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventBody {
    OpPut { op: Op },
    OpDelete { id: String },
    TripPut { trip: Trip },
    TripDelete { id: String },
    AccountPut { account: Account },
    AccountDelete { id: String },
    SettingPut { key: String, value: String },
}

impl EventBody {
    /// Вид записи, её номер и содержимое (None — удаление).
    pub fn item(&self) -> crate::Result<(&'static str, String, Option<String>)> {
        Ok(match self {
            EventBody::OpPut { op } => ("op", op.id.clone(), Some(serde_json::to_string(op)?)),
            EventBody::OpDelete { id } => ("op", id.clone(), None),
            EventBody::TripPut { trip } => ("trip", trip.id.clone(), Some(serde_json::to_string(trip)?)),
            EventBody::TripDelete { id } => ("trip", id.clone(), None),
            EventBody::AccountPut { account } => ("account", account.id.clone(), Some(serde_json::to_string(account)?)),
            EventBody::AccountDelete { id } => ("account", id.clone(), None),
            EventBody::SettingPut { key, value } => ("setting", key.clone(), Some(serde_json::to_string(value)?)),
        })
    }
}

const MAGIC: &[u8; 4] = b"KRM1";
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Шифрует пакет: «KRM1» + случайный nonce + шифротекст XChaCha20-Poly1305.
pub fn seal(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ct = cipher.encrypt(&nonce, plain).map_err(|_| Error::Decrypt)?;
    let mut out = Vec::with_capacity(4 + 24 + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn open(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 4 + 24 + 16 || &data[..4] != MAGIC {
        return Err(Error::Decrypt);
    }
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XNonce::from_slice(&data[4..28]);
    cipher.decrypt(nonce, &data[28..]).map_err(|_| Error::Decrypt)
}

/// Версия «Кармана»: другое устройство узнаёт её при обмене.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Устройство до 0.4 не может принять записи новых видов.
const OLD_PEER: &str = "На другом устройстве старая версия «Кармана», и она не приняла новые записи. Обновите «Карман» там и свяжите устройства снова";

/// Сообщения обмена. Поле `files` — устройство умеет передавать вложения
/// (версии до 0.4 его не присылают, и шаг с вложениями пропускается).
/// Поле `app` — версия «Кармана» (присылают версии с 0.5).
#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Msg {
    Hello {
        device: String,
        name: String,
        vv: VersionVector,
        #[serde(default)]
        files: bool,
        #[serde(default)]
        app: String,
    },
    Batch {
        device: String,
        name: String,
        vv: VersionVector,
        events: Vec<Event>,
        #[serde(default)]
        files: bool,
        #[serde(default)]
        app: String,
    },
    Done {
        applied: usize,
    },
    /// Каких вложений не хватает.
    Want { ids: Vec<String> },
    /// Вложения по запросу и свой список недостающих.
    Give { blobs: Vec<Blob>, want: Vec<String> },
}

#[derive(Serialize, Deserialize)]
struct Blob {
    id: String,
    mime: String,
    /// Содержимое в base64.
    data: String,
}

/// Столько байт вложений в одном пакете — с запасом до предела кадра.
const FILES_PER_PACKET: usize = 24 * 1024 * 1024;
/// Больше кругов обмена вложениями за одно подключение не делаем.
const MAX_ROUNDS: usize = 50;

/// Достаёт запрошенные вложения, пока не наберётся предел пакета.
/// Возвращает вложения и сколько из имеющихся не поместилось.
fn collect_blobs(store: &Mutex<Store>, ids: &[String]) -> Result<(Vec<Blob>, usize)> {
    let st = lock(store);
    let (mut out, mut total, mut left) = (Vec::new(), 0usize, 0usize);
    for id in ids {
        if let Some((mime, data)) = st.blob(id)? {
            if total + data.len() > FILES_PER_PACKET && !out.is_empty() {
                left += 1;
                continue;
            }
            total += data.len();
            out.push(Blob { id: id.clone(), mime, data: B64.encode(&data) });
        }
    }
    Ok((out, left))
}

/// Сохраняет полученные вложения: только запрошенные и только с верным отпечатком.
fn store_blobs(store: &Mutex<Store>, asked: &[String], blobs: Vec<Blob>) -> usize {
    let st = lock(store);
    let mut n = 0;
    for b in blobs {
        if !asked.contains(&b.id) {
            continue;
        }
        let Ok(data) = B64.decode(b.data.as_bytes()) else { continue };
        if st.put_blob_checked(&b.id, &b.mime, &data).is_ok() {
            n += 1;
        }
    }
    n
}

fn write_msg(s: &mut TcpStream, key: &[u8; 32], msg: &Msg) -> Result<()> {
    let frame = seal(key, &serde_json::to_vec(msg)?)?;
    s.write_all(&(frame.len() as u32).to_be_bytes())?;
    s.write_all(&frame)?;
    s.flush()?;
    Ok(())
}

fn read_msg(s: &mut TcpStream, key: &[u8; 32]) -> Result<Msg> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(Error::Format("слишком большой пакет".into()));
    }
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf)?;
    Ok(serde_json::from_slice(&open(key, &buf)?)?)
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SyncReport {
    pub peer: String,
    pub peer_name: String,
    /// Сколько событий ушло другому устройству и было им принято.
    pub sent: usize,
    /// Сколько новых событий пришло к нам.
    pub received: usize,
    /// Сколько снимков и файлов чеков ушло и пришло.
    #[serde(default)]
    pub files_sent: usize,
    #[serde(default)]
    pub files_received: usize,
    /// Версия «Кармана» на другом устройстве; пусто — версия до 0.5.
    #[serde(default)]
    pub peer_app: String,
}

fn lock(store: &Mutex<Store>) -> std::sync::MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|e| e.into_inner())
}

fn prepare(s: &TcpStream) -> Result<()> {
    // Пакет со снимками по медленному Wi-Fi идёт долго.
    s.set_read_timeout(Some(Duration::from_secs(60)))?;
    s.set_write_timeout(Some(Duration::from_secs(60)))?;
    s.set_nodelay(true)?;
    Ok(())
}

/// Сторона, которая подключается (обычно телефон, отсканировавший QR).
pub fn sync_over(mut s: TcpStream, key: &[u8; 32], store: &Mutex<Store>, name: &str) -> Result<SyncReport> {
    prepare(&s)?;
    let (device, vv) = {
        let st = lock(store);
        (st.device().to_string(), st.version_vector()?)
    };
    write_msg(&mut s, key, &Msg::Hello { device: device.clone(), name: name.into(), vv, files: true, app: APP_VERSION.into() })?;
    let (peer, peer_name, their_vv, events, peer_files, peer_app) = match read_msg(&mut s, key)? {
        Msg::Batch { device, name, vv, events, files, app } => (device, name, vv, events, files, app),
        _ => return Err(Error::Format("неожиданный ответ".into())),
    };
    let (received, ours) = {
        let mut st = lock(store);
        let n = st.apply(&events)?;
        (n, st.events_after(&their_vv)?)
    };
    let vv = lock(store).version_vector()?;
    write_msg(&mut s, key, &Msg::Batch { device, name: name.into(), vv, events: ours, files: true, app: APP_VERSION.into() })?;
    let sent = match read_msg(&mut s, key) {
        Ok(Msg::Done { applied }) => applied,
        // Старая версия не разобрала пакет с записями новых видов и оборвала связь.
        Err(_) if !peer_files => return Err(Error::Peer(OLD_PEER.into())),
        Err(e) => return Err(e),
        _ => return Err(Error::Format("неожиданный ответ".into())),
    };
    let (mut files_sent, mut files_received) = (0, 0);
    if peer_files {
        for _ in 0..MAX_ROUNDS {
            let want = lock(store).missing_blobs()?;
            write_msg(&mut s, key, &Msg::Want { ids: want.clone() })?;
            let (blobs, their_want) = match read_msg(&mut s, key)? {
                Msg::Give { blobs, want } => (blobs, want),
                _ => return Err(Error::Format("неожиданный ответ".into())),
            };
            let got = store_blobs(store, &want, blobs);
            files_received += got;
            let (mine, left) = collect_blobs(store, &their_want)?;
            write_msg(&mut s, key, &Msg::Give { blobs: mine, want: Vec::new() })?;
            match read_msg(&mut s, key)? {
                Msg::Done { applied } => files_sent += applied,
                _ => return Err(Error::Format("неожиданный ответ".into())),
            }
            let still = lock(store).missing_blobs()?.len();
            if (still == 0 || got == 0) && left == 0 {
                break;
            }
        }
        write_msg(&mut s, key, &Msg::Done { applied: 0 })?;
    }
    Ok(SyncReport { peer, peer_name, sent, received, files_sent, files_received, peer_app })
}

/// Подключается к устройству из QR-кода: пробует все его адреса по очереди.
pub fn sync_with(p: &Pairing, store: &Mutex<Store>, name: &str) -> Result<SyncReport> {
    let mut last = None;
    for host in &p.hosts {
        let addrs: Vec<SocketAddr> = match (host.as_str(), p.port).to_socket_addrs() {
            Ok(a) => a.collect(),
            Err(e) => {
                last = Some(Error::Io(e));
                continue;
            }
        };
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, Duration::from_secs(4)) {
                Ok(s) => return sync_over(s, &p.key, store, name),
                Err(e) => last = Some(Error::Io(e)),
            }
        }
    }
    Err(last.unwrap_or_else(|| Error::Format("нет адресов для подключения".into())))
}

/// Сторона, которая ждёт подключения (обычно компьютер, показавший QR).
pub fn serve_one(mut s: TcpStream, key: &[u8; 32], store: &Mutex<Store>, name: &str) -> Result<SyncReport> {
    prepare(&s)?;
    let (peer, peer_name, their_vv, peer_files, peer_app) = match read_msg(&mut s, key)? {
        Msg::Hello { device, name, vv, files, app } => (device, name, vv, files, app),
        _ => return Err(Error::Format("ожидалось приветствие".into())),
    };
    let (device, vv, events) = {
        let st = lock(store);
        (st.device().to_string(), st.version_vector()?, st.events_after(&their_vv)?)
    };
    let sent = events.len();
    write_msg(&mut s, key, &Msg::Batch { device, name: name.into(), vv, events, files: true, app: APP_VERSION.into() })?;
    let incoming = match read_msg(&mut s, key) {
        Ok(Msg::Batch { events, .. }) => events,
        Err(_) if !peer_files => return Err(Error::Peer(OLD_PEER.into())),
        Err(e) => return Err(e),
        _ => return Err(Error::Format("ожидались события".into())),
    };
    let received = lock(store).apply(&incoming)?;
    write_msg(&mut s, key, &Msg::Done { applied: received })?;
    let (mut files_sent, mut files_received) = (0, 0);
    if peer_files {
        for _ in 0..MAX_ROUNDS {
            match read_msg(&mut s, key)? {
                Msg::Want { ids } => {
                    let (blobs, _) = collect_blobs(store, &ids)?;
                    files_sent += blobs.len();
                    let want = lock(store).missing_blobs()?;
                    write_msg(&mut s, key, &Msg::Give { blobs, want: want.clone() })?;
                    let incoming = match read_msg(&mut s, key)? {
                        Msg::Give { blobs, .. } => blobs,
                        _ => return Err(Error::Format("ожидались вложения".into())),
                    };
                    let n = store_blobs(store, &want, incoming);
                    files_received += n;
                    write_msg(&mut s, key, &Msg::Done { applied: n })?;
                }
                Msg::Done { .. } => break,
                _ => return Err(Error::Format("неожиданное сообщение".into())),
            }
        }
    }
    Ok(SyncReport { peer, peer_name, sent, received, files_sent, files_received, peer_app })
}

/// Фоновый приём подключений, пока не остановят.
pub struct Server {
    pub port: u16,
    stop: Arc<AtomicBool>,
    pub log: Arc<Mutex<Vec<std::result::Result<SyncReport, String>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(store: Arc<Mutex<Store>>, key: [u8; 32], name: String, port: u16) -> Result<Server> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (stop2, log2) = (stop.clone(), log.clone());
        let thread = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let r = serve_one(s, &key, &store, &name).map_err(|e| e.to_string());
                        log2.lock().unwrap_or_else(|e| e.into_inner()).push(r);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(80)),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        });
        Ok(Server { port, stop, log, thread: Some(thread) })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    #[test]
    fn seal_open_and_tamper() {
        let key = [9u8; 32];
        let p = seal(&key, b"privet").unwrap();
        assert_eq!(open(&key, &p).unwrap(), b"privet");
        let mut bad = p.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(matches!(open(&key, &bad), Err(Error::Decrypt)));
        assert!(matches!(open(&[8u8; 32], &p), Err(Error::Decrypt)));
    }

    #[test]
    fn two_devices_sync_over_tcp() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(Mutex::new(Store::open(&dir.path().join("a.db"), &[1u8; 32], "phone").unwrap()));
        let b = Arc::new(Mutex::new(Store::open(&dir.path().join("b.db"), &[2u8; 32], "laptop").unwrap()));
        let ops = demo::ops();
        for op in ops[0..3].iter().cloned() {
            a.lock().unwrap().put_op(op).unwrap();
        }
        for op in ops[3..5].iter().cloned() {
            b.lock().unwrap().put_op(op).unwrap();
        }
        // Оба меняют одну и ту же операцию: на телефоне правка позже.
        let mut shared = ops[0].clone();
        shared.title = "Изменено на ноутбуке".into();
        b.lock().unwrap().put_op(shared.clone()).unwrap();
        shared.title = "Изменено на телефоне".into();
        a.lock().unwrap().put_op(shared.clone()).unwrap();
        a.lock().unwrap().put_op({ let mut x = shared.clone(); x.title = "Изменено на телефоне ещё раз".into(); x }).unwrap();

        let key = crate::pairing::random_key().unwrap();
        let server = Server::start(b.clone(), key, "Ноутбук".into(), 0).unwrap();
        let p = Pairing { device: "laptop".into(), name: "Ноутбук".into(), hosts: vec!["127.0.0.1".into()], port: server.port, key };
        let r = sync_with(&p, &a, "Телефон").unwrap();
        assert_eq!(r.peer, "laptop");
        assert_eq!((r.peer_name.as_str(), r.peer_app.as_str()), ("Ноутбук", APP_VERSION));
        assert_eq!(r.received, 3);
        assert_eq!(r.sent, 5);
        let oa = a.lock().unwrap().ops().unwrap();
        let ob = b.lock().unwrap().ops().unwrap();
        assert_eq!(oa.len(), 5);
        assert_eq!(oa, ob, "после обмена данные одинаковые");
        let edited = oa.iter().find(|o| o.id == shared.id).unwrap();
        assert_eq!(edited.title, "Изменено на телефоне ещё раз");
        // Повторный обмен ничего не передаёт.
        let r2 = sync_with(&p, &a, "Телефон").unwrap();
        assert_eq!((r2.sent, r2.received), (0, 0));
        // Чужой ключ не проходит.
        let wrong = Pairing { key: [0u8; 32], ..p };
        assert!(sync_with(&wrong, &a, "Телефон").is_err());
        server.stop();
    }

    /// Минимальный JPEG для проверок: заголовок и разное содержимое.
    fn jpeg(seed: u8, len: usize) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend((0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)));
        v
    }

    #[test]
    fn receipts_travel_with_operations() {
        let dir = tempfile::tempdir().unwrap();
        let phone = Arc::new(Mutex::new(Store::open(&dir.path().join("p.db"), &[1u8; 32], "phone").unwrap()));
        let laptop = Arc::new(Mutex::new(Store::open(&dir.path().join("l.db"), &[2u8; 32], "laptop").unwrap()));
        // Снимок на телефоне, PDF на ноутбуке; один большой, чтобы понадобилось несколько пакетов.
        let photo = jpeg(1, 300_000);
        let big: Vec<Vec<u8>> = (0..3).map(|i| jpeg(10 + i, 9 * 1024 * 1024)).collect();
        let pdf = b"%PDF-1.7 tiny".to_vec();
        let mut op = demo::ops().remove(2);
        {
            let p = phone.lock().unwrap();
            let mut att = vec![];
            for data in std::iter::once(&photo).chain(big.iter()) {
                let id = p.put_blob("image/jpeg", data).unwrap();
                att.push(crate::trip::Attachment { id, mime: "image/jpeg".into(), name: "чек.jpg".into(), size: data.len() as u64 });
            }
            op.att = att;
        }
        phone.lock().unwrap().put_op(op.clone()).unwrap();
        let mut op2 = demo::ops().remove(4);
        {
            let l = laptop.lock().unwrap();
            let id = l.put_blob("application/pdf", &pdf).unwrap();
            op2.att = vec![crate::trip::Attachment { id, mime: "application/pdf".into(), name: "билет.pdf".into(), size: pdf.len() as u64 }];
        }
        laptop.lock().unwrap().put_op(op2.clone()).unwrap();

        let key = crate::pairing::random_key().unwrap();
        let server = Server::start(laptop.clone(), key, "Ноутбук".into(), 0).unwrap();
        let p = Pairing { device: "laptop".into(), name: "Ноутбук".into(), hosts: vec!["127.0.0.1".into()], port: server.port, key };
        let r = sync_with(&p, &phone, "Телефон").unwrap();
        assert_eq!((r.files_sent, r.files_received), (4, 1), "{r:?}");
        assert!(phone.lock().unwrap().missing_blobs().unwrap().is_empty());
        assert!(laptop.lock().unwrap().missing_blobs().unwrap().is_empty());
        let (mime, data) = laptop.lock().unwrap().blob(&op.att[0].id).unwrap().unwrap();
        assert_eq!((mime.as_str(), data), ("image/jpeg", photo));
        // Второй обмен ничего не гоняет.
        let r2 = sync_with(&p, &phone, "Телефон").unwrap();
        assert_eq!((r2.sent, r2.received, r2.files_sent, r2.files_received), (0, 0, 0, 0));
        server.stop();
    }

    #[test]
    fn foreign_blob_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&dir.path().join("x.db"), &[3u8; 32], "x").unwrap();
        let data = jpeg(5, 100);
        assert!(st.put_blob_checked(&"0".repeat(64), "image/jpeg", &data).is_err(), "отпечаток не совпал");
        assert!(st.put_blob("image/jpeg", b"MZ\x90\x00 exe").is_err(), "не снимок");
        assert!(st.put_blob("text/html", b"<html>").is_err(), "чужой тип");
        assert!(st.put_blob("image/jpeg", &vec![0xFFu8; crate::db::MAX_BLOB + 1]).is_err(), "слишком большой");
    }

    #[test]
    fn events_from_newer_versions_survive() {
        // Событие неизвестного вида и операция с незнакомым полем приходят от новой версии.
        let dir = tempfile::tempdir().unwrap();
        let mut st = Store::open(&dir.path().join("o.db"), &[4u8; 32], "old").unwrap();
        let mut op = serde_json::to_value(demo::ops().remove(0)).unwrap();
        op["future_field"] = serde_json::json!({"x": 1});
        let events = vec![
            Event { device: "new".into(), seq: 1, lamport: 1, body: serde_json::json!({"type": "op_put", "op": op}) },
            Event { device: "new".into(), seq: 2, lamport: 2, body: serde_json::json!({"type": "budget_put", "budget": {"id": "b1", "limit": 100}}) },
        ];
        assert_eq!(st.apply(&events).unwrap(), 2, "оба события приняты, обмен не сломался");
        let stored = st.events_after(&VersionVector::new()).unwrap();
        assert_eq!(stored[1].body["type"], "budget_put", "неизвестное событие хранится как есть");
        let mut o = st.ops().unwrap().remove(0);
        assert_eq!(o.extra["future_field"], serde_json::json!({"x": 1}));
        // Правка на старой версии не теряет поле новой.
        o.title = "Правка".into();
        st.put_op(o).unwrap();
        let again = st.ops().unwrap().remove(0);
        assert_eq!((again.title.as_str(), &again.extra["future_field"]), ("Правка", &serde_json::json!({"x": 1})));
    }

    /// Старая версия (до 0.4) не разбирает записи новых видов и рвёт связь — говорим об этом понятно.
    #[test]
    fn old_peer_gets_clear_error_and_versions_are_exchanged() {
        use std::net::TcpListener;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::open(&dir.path().join("n.db"), &[3u8; 32], "new").unwrap()));
        store.lock().unwrap().put_op(demo::ops().remove(0)).unwrap();
        let key = [7u8; 32];
        let raw_write = move |s: &mut TcpStream, json: &str| {
            let frame = seal(&key, json.as_bytes()).unwrap();
            s.write_all(&(frame.len() as u32).to_be_bytes()).unwrap();
            s.write_all(&frame).unwrap();
        };
        let raw_read = move |s: &mut TcpStream| {
            let mut len = [0u8; 4];
            s.read_exact(&mut len).unwrap();
            let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
            s.read_exact(&mut buf).unwrap();
            serde_json::from_slice::<serde_json::Value>(&open(&key, &buf).unwrap()).unwrap()
        };

        // Мы подключаемся к старому устройству.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let old = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let hello = raw_read(&mut s);
            raw_write(&mut s, r#"{"t":"batch","device":"old","name":"Старый","vv":{},"events":[]}"#);
            let _ours = raw_read(&mut s);
            hello
        });
        let r = sync_over(TcpStream::connect(addr).unwrap(), &key, &store, "Новый");
        let hello = old.join().unwrap();
        assert_eq!(hello["app"], APP_VERSION, "своя версия уходит в приветствии");
        assert!(matches!(r, Err(Error::Peer(ref m)) if m.contains("старая версия")), "{:?}", r.map(|_| ()));

        // Старое устройство подключается к нам.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let st2 = store.clone();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            serve_one(s, &key, &st2, "Новый").map(|_| ())
        });
        let mut c = TcpStream::connect(addr).unwrap();
        raw_write(&mut c, r#"{"t":"hello","device":"old","name":"Старый","vv":{}}"#);
        let batch = raw_read(&mut c);
        assert_eq!((batch["app"].as_str(), batch["events"].as_array().map(|e| e.len())), (Some(APP_VERSION), Some(1)));
        drop(c);
        assert!(matches!(server.join().unwrap(), Err(Error::Peer(_))));
    }
}
