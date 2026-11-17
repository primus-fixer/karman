//! Синхронизация: события, шифрование пакетов и обмен по локальной сети.
//!
//! Все каналы (Wi-Fi, облако, свой сервер) передают одни и те же
//! зашифрованные пакеты. Протокол обмена — общий из kit (`kit_core::sync`), тот же, что у «Полки»;
//! здесь — события «Кармана», его журнал для протокола, настройки обмена (поле `files` для версий
//! до 0.4) и прежняя связь по QR-коду с ключом (до 0.15) поверх общего обмена.

use crate::db::Store;
use crate::error::{Error, Result};
use crate::pairing::Pairing;
use crate::book::Account;
use crate::budget::Budget;
use crate::category::Category;
use crate::finance::{Debt, Plan};
use crate::overtime::{WorkDay, WorkOrder};
use crate::savings::Goal;
use crate::trip::{Op, Trip};
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use kit_core::sync::{self as kit, Journal, NoFile, Profile};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Последний известный номер события от каждого устройства.
pub type VersionVector = BTreeMap<String, u64>;

pub(crate) use kit::{prepare, Wire, MAX_FRAME};

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
    DebtPut { debt: Debt },
    DebtDelete { id: String },
    PlanPut { plan: Plan },
    PlanDelete { id: String },
    GoalPut { goal: Goal },
    GoalDelete { id: String },
    BudgetPut { budget: Budget },
    /// Наряд поездки.
    OrderPut { order: WorkOrder },
    OrderDelete { id: String },
    /// Рабочий день поездки: время работ и комментарий.
    DayPut { day: WorkDay },
    DayDelete { id: String },
    /// Своя категория личных трат (с 0.14.0). Удаления нет: категорию скрывают.
    CategoryPut { category: Category },
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
            EventBody::DebtPut { debt } => ("debt", debt.id.clone(), Some(serde_json::to_string(debt)?)),
            EventBody::DebtDelete { id } => ("debt", id.clone(), None),
            EventBody::PlanPut { plan } => ("plan", plan.id.clone(), Some(serde_json::to_string(plan)?)),
            EventBody::PlanDelete { id } => ("plan", id.clone(), None),
            EventBody::GoalPut { goal } => ("goal", goal.id.clone(), Some(serde_json::to_string(goal)?)),
            EventBody::GoalDelete { id } => ("goal", id.clone(), None),
            EventBody::BudgetPut { budget } => ("budget", budget.id.clone(), Some(serde_json::to_string(budget)?)),
            EventBody::OrderPut { order } => ("order", order.id.clone(), Some(serde_json::to_string(order)?)),
            EventBody::OrderDelete { id } => ("order", id.clone(), None),
            EventBody::DayPut { day } => ("workday", day.id.clone(), Some(serde_json::to_string(day)?)),
            EventBody::DayDelete { id } => ("workday", id.clone(), None),
            EventBody::CategoryPut { category } => ("category", category.id.clone(), Some(serde_json::to_string(category)?)),
        })
    }
}

const MAGIC: &[u8; 4] = b"KRM1";
/// QR-код действует 15 минут: потом приём подключений останавливается сам.
pub const SERVER_TTL: Duration = Duration::from_secs(15 * 60);

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

/// Обмен «Кармана»: поле `files` (версии до 0.4 вложений не передают), больших файлов нет.
pub const PROFILE: Profile = Profile { version: APP_VERSION, files_flag: true, old_peer: OLD_PEER, chunked_since: None };

/// Сообщения обмена.
pub(crate) type Msg = kit::Msg<Event>;

/// Сообщения поверх канала — для связи по коду (кто я) и проверок.
pub(crate) trait MsgWire: Wire + Sized {
    fn send(&mut self, msg: &Msg) -> Result<()> {
        Ok(kit::send(self, msg)?)
    }
    /// Следующее сообщение, не больше `max` байт.
    fn recv_max(&mut self, max: usize) -> Result<Msg> {
        Ok(kit::recv_max(self, max)?)
    }
    /// Следующее сообщение любого размера до предела — в проверках канала.
    #[cfg(test)]
    fn recv(&mut self) -> Result<Msg> {
        self.recv_max(MAX_FRAME)
    }
}

impl<W: Wire> MsgWire for W {}

/// Канал с одним ключом на обе стороны и случайным nonce в каждом пакете — связь по QR-коду до 0.15.
struct KeyWire<'a> {
    s: TcpStream,
    key: &'a [u8; 32],
}

impl Wire for KeyWire<'_> {
    fn send_bytes(&mut self, data: &[u8]) -> kit_core::Result<()> {
        let frame = seal(self.key, data).map_err(|_| kit_core::Error::Decrypt)?;
        self.s.write_all(&(frame.len() as u32).to_be_bytes())?;
        self.s.write_all(&frame)?;
        self.s.flush()?;
        Ok(())
    }
    fn recv_bytes(&mut self, max: usize) -> kit_core::Result<Vec<u8>> {
        let mut len = [0u8; 4];
        self.s.read_exact(&mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        if len > max {
            return Err(kit_core::Error::Format("слишком большой пакет".into()));
        }
        let mut buf = vec![0u8; len];
        self.s.read_exact(&mut buf)?;
        open(self.key, &buf).map_err(|_| kit_core::Error::Decrypt)
    }
    fn stream(&self) -> &TcpStream {
        &self.s
    }
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

impl From<kit::SyncReport> for SyncReport {
    fn from(r: kit::SyncReport) -> Self {
        SyncReport { peer: r.peer, peer_name: r.peer_name, sent: r.sent, received: r.received, files_sent: r.files_sent, files_received: r.files_received, peer_app: r.peer_app }
    }
}

fn store_err(e: Error) -> kit_core::Error {
    kit_core::Error::Store(e.to_string())
}

/// Журнал «Кармана» для общего протокола: события и снимки чеков.
impl Journal for Store {
    type Event = Event;
    type File = NoFile;

    fn device(&self) -> String {
        Store::device(self).to_string()
    }
    fn versions(&self) -> kit_core::Result<VersionVector> {
        self.version_vector().map_err(store_err)
    }
    fn events_missing(&self, theirs: &VersionVector) -> kit_core::Result<Vec<Event>> {
        self.events_after(theirs).map_err(store_err)
    }
    fn apply_remote(&mut self, events: &[Event]) -> kit_core::Result<usize> {
        self.apply(events).map_err(store_err)
    }
    fn missing_blobs(&self) -> kit_core::Result<Vec<String>> {
        Store::missing_blobs(self).map_err(store_err)
    }
    fn blob(&self, id: &str) -> kit_core::Result<Option<(String, Vec<u8>)>> {
        Store::blob(self, id).map_err(store_err)
    }
    fn put_blob_checked(&mut self, id: &str, mime: &str, data: &[u8]) -> kit_core::Result<()> {
        Store::put_blob_checked(self, id, mime, data).map_err(store_err)
    }
}

/// Сторона, которая подключается (обычно телефон, отсканировавший QR).
pub fn sync_over(s: TcpStream, key: &[u8; 32], store: &Mutex<Store>, name: &str) -> Result<SyncReport> {
    run_client(&mut KeyWire { s, key }, store, name, None)
}

/// Обмен со стороны подключившегося — по любому каналу. `expect` — какое устройство должно ответить
/// (по ключу пары): ответ другого — обрыв до того, как его записи приняты.
pub(crate) fn run_client(w: &mut impl Wire, store: &Mutex<Store>, name: &str, expect: Option<&str>) -> Result<SyncReport> {
    Ok(kit::run_client(&PROFILE, w, store, name, expect)?.into())
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
pub fn serve_one(s: TcpStream, key: &[u8; 32], store: &Mutex<Store>, name: &str) -> Result<SyncReport> {
    run_server(&mut KeyWire { s, key }, store, name, None)
}

/// Обмен со стороны ждущего — по любому каналу. `expect` — какое устройство должно прийти
/// (по ключу пары): другое имя в приветствии — обрыв.
pub(crate) fn run_server(w: &mut impl Wire, store: &Mutex<Store>, name: &str, expect: Option<&str>) -> Result<SyncReport> {
    Ok(kit::run_server(&PROFILE, w, store, name, expect)?.into())
}

/// Фоновый приём подключений, пока не остановят.
pub struct Server {
    pub port: u16,
    stop: Arc<AtomicBool>,
    pub log: Arc<Mutex<Vec<std::result::Result<SyncReport, String>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Принимает подключения 15 минут — пока действует QR-код.
    pub fn start(store: Arc<Mutex<Store>>, key: [u8; 32], name: String, port: u16) -> Result<Server> {
        Server::start_for(store, key, name, port, SERVER_TTL)
    }

    pub fn start_for(store: Arc<Mutex<Store>>, key: [u8; 32], name: String, port: u16, ttl: Duration) -> Result<Server> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (stop2, log2) = (stop.clone(), log.clone());
        let until = std::time::Instant::now() + ttl;
        let thread = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) && std::time::Instant::now() < until {
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

    /// Ещё принимает подключения (не остановлен и QR-код не истёк).
    pub fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
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
    fn server_stops_by_itself_and_rejects_big_hello() {
        let dir = tempfile::tempdir().unwrap();
        let b = Arc::new(Mutex::new(Store::open(&dir.path().join("b.db"), &[2u8; 32], "laptop").unwrap()));
        let key = crate::pairing::random_key().unwrap();
        let server = Server::start_for(b.clone(), key, "Ноутбук".into(), 0, Duration::from_millis(300)).unwrap();
        assert!(server.running());
        // Чужой без ключа объявляет огромное приветствие — отказ сразу, без ожидания данных.
        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        s.write_all(&((kit::HELLO_MAX as u32 + 1).to_be_bytes())).unwrap();
        let t0 = std::time::Instant::now();
        let mut buf = [0u8; 1];
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.read(&mut buf);
        assert!(t0.elapsed() < Duration::from_secs(3), "соединение закрыто сразу");
        std::thread::sleep(Duration::from_millis(600));
        assert!(!server.running(), "QR-код истёк — приём остановлен");
        let log = server.log.lock().unwrap().clone();
        assert!(log.iter().any(|r| matches!(r, Err(m) if m.contains("слишком большой"))), "{log:?}");
        let a = Mutex::new(Store::open(&dir.path().join("a.db"), &[1u8; 32], "phone").unwrap());
        let p = Pairing { device: "laptop".into(), name: "Ноутбук".into(), hosts: vec!["127.0.0.1".into()], port: server.port, key };
        assert!(sync_with(&p, &a, "Телефон").is_err(), "после остановки не подключиться");
    }

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

    /// Свои категории приходят с другого устройства, как остальные записи: последняя правка — главная.
    #[test]
    fn categories_sync_like_other_records() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = Store::open(&dir.path().join("c.db"), &[4u8; 32], "a").unwrap();
        let put = |seq: u64, name: &str| Event { device: "b".into(), seq, lamport: seq, body: serde_json::json!({"type": "category_put", "category": {"id": "c-1", "name": name, "icon": "sprout", "color": "teal"}}) };
        assert_eq!(st.apply(&[put(1, "Дача"), put(2, "Дача и сад")]).unwrap(), 2);
        let cats = st.categories().unwrap();
        assert_eq!((cats.len(), cats[0].name.as_str(), &cats[0].extra["color"]), (1, "Дача и сад", &serde_json::json!("teal")), "поле новой версии не теряется");
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
