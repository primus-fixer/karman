//! Синхронизация: события, шифрование пакетов и обмен по локальной сети.
//!
//! Все каналы (Wi-Fi, облако, свой сервер) передают одни и те же
//! зашифрованные пакеты. Здесь — общий формат и обмен напрямую по TCP.

use crate::db::Store;
use crate::error::{Error, Result};
use crate::pairing::Pairing;
use crate::trip::Op;
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
    pub body: EventBody,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventBody {
    OpPut { op: Op },
    OpDelete { id: String },
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

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Msg {
    Hello { device: String, name: String, vv: VersionVector },
    Batch { device: String, name: String, vv: VersionVector, events: Vec<Event> },
    Done { applied: usize },
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
}

fn lock(store: &Mutex<Store>) -> std::sync::MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|e| e.into_inner())
}

fn prepare(s: &TcpStream) -> Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(20)))?;
    s.set_write_timeout(Some(Duration::from_secs(20)))?;
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
    write_msg(&mut s, key, &Msg::Hello { device: device.clone(), name: name.into(), vv })?;
    let (peer, peer_name, their_vv, events) = match read_msg(&mut s, key)? {
        Msg::Batch { device, name, vv, events } => (device, name, vv, events),
        _ => return Err(Error::Format("неожиданный ответ".into())),
    };
    let (received, ours) = {
        let mut st = lock(store);
        let n = st.apply(&events)?;
        (n, st.events_after(&their_vv)?)
    };
    let vv = lock(store).version_vector()?;
    write_msg(&mut s, key, &Msg::Batch { device, name: name.into(), vv, events: ours })?;
    let sent = match read_msg(&mut s, key)? {
        Msg::Done { applied } => applied,
        _ => return Err(Error::Format("неожиданный ответ".into())),
    };
    Ok(SyncReport { peer, peer_name, sent, received })
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
    let (peer, peer_name, their_vv) = match read_msg(&mut s, key)? {
        Msg::Hello { device, name, vv } => (device, name, vv),
        _ => return Err(Error::Format("ожидалось приветствие".into())),
    };
    let (device, vv, events) = {
        let st = lock(store);
        (st.device().to_string(), st.version_vector()?, st.events_after(&their_vv)?)
    };
    let sent = events.len();
    write_msg(&mut s, key, &Msg::Batch { device, name: name.into(), vv, events })?;
    let incoming = match read_msg(&mut s, key)? {
        Msg::Batch { events, .. } => events,
        _ => return Err(Error::Format("ожидались события".into())),
    };
    let received = lock(store).apply(&incoming)?;
    write_msg(&mut s, key, &Msg::Done { applied: received })?;
    Ok(SyncReport { peer, peer_name, sent, received })
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
        assert_eq!(r.peer_name, "Ноутбук");
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
}
