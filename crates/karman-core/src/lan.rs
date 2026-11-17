//! Связь по Wi-Fi с 0.15: поиск устройств в сети, связь по коду и обмен со связанными устройствами.
//!
//! С 0.15.8 всё устройство связи — общее, из kit (`kit_core::lan`): связь по коду (SPAKE2), ключ пары,
//! шифрованный канал сеанса и поиск в сети. Здесь — то, чем «Карман» отличается: свои порты, метки
//! и строки ключей ([`LAN`]), чтобы приложения в одной сети не мешали друг другу, и прежние тексты ошибок.
//! Менять настройки нельзя: устройства с прежней версией перестанут находить и узнавать друг друга.
//! Связь по QR-коду до 0.15 (одноразовый ключ в QR) — в [`crate::sync`].

use crate::db::Store;
use crate::error::Result;
use crate::sync::{SyncReport, PROFILE};
use kit_core::lan as kit;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use kit::{valid_id, CodeState, Found, Invite, OnPaired, Peer, CODE_TRIES, CODE_TTL};

/// Связь по Wi-Fi «Кармана». Порт обмена — тот же, что у связи по QR-коду до 0.15; у «Полки» свои.
pub const LAN: kit::Lan = kit::Lan {
    names: crate::NAMES,
    profile: &PROFILE,
    tcp_port: 47500,
    discovery_port: 47501,
    magic: *b"KRML",
    query_magic: *b"KRMQ",
    answer_magic: *b"KRMA",
    joiner: b"karman-joiner",
    host: b"karman-host",
    personal: b"karman-lan",
    not_ours: "это не «Карман» 0.15 или новее",
    qr_scheme: crate::pairing::SCHEME,
    qr_version: 2,
};

/// Что нужно связи по Wi-Fi в приложении от базы: местные настройки и число событий.
impl kit::Device for Store {
    fn local(&self, key: &str) -> Option<String> {
        self.meta(key).ok().flatten()
    }
    fn set_local(&self, key: &str, value: &str) -> kit_core::Result<()> {
        self.set_meta(key, value).map_err(|e| kit_core::Error::Store(e.to_string()))
    }
    fn event_count(&self) -> u64 {
        Store::event_count(self).unwrap_or(0)
    }
}

/// Порт обмена (TCP).
pub const TCP_PORT: u16 = LAN.tcp_port;
/// Порт поиска в сети (UDP).
pub const DISCOVERY_PORT: u16 = LAN.discovery_port;

type KeyFn = dyn Fn(&str) -> Option<[u8; 32]> + Send + Sync;
type PairedFn = dyn Fn(&Peer, &[u8; 32]) -> std::result::Result<(), String> + Send + Sync;
type SyncedFn = dyn Fn(&str, &std::result::Result<SyncReport, String>) + Send + Sync;

/// Это устройство в сети: чем отвечать и куда записывать. Общее у сервера и ответчика на поиск.
#[derive(Clone)]
pub struct Host {
    pub store: Arc<Mutex<Store>>,
    pub device: String,
    pub name: String,
    /// Показанный код; None — кода нет.
    pub code: Arc<Mutex<Option<CodeState>>>,
    /// Ключ пары по номеру связанного устройства.
    pub peer_key: Arc<KeyFn>,
    /// Связались по коду — запомнить устройство и ключ (до первого обмена). Ошибка — обрыв без обмена.
    pub paired: Arc<PairedFn>,
    /// Обмен закончился — с кем и чем.
    pub synced: Arc<SyncedFn>,
}

impl Host {
    /// То же устройство для общего модуля: ошибка обмена — словами «Кармана».
    fn kit(self) -> kit::Host<Store> {
        let synced = self.synced;
        kit::Host {
            lan: &LAN,
            store: self.store,
            device: self.device,
            name: self.name,
            code: self.code,
            peer_key: self.peer_key,
            paired: self.paired,
            synced: Arc::new(move |id, r| synced(id, &r.map(SyncReport::from).map_err(|e| crate::Error::from(e).to_string()))),
        }
    }
}

/// Связь по коду с устройством по адресу: ключ пары и первый обмен. `paired` запоминает устройство
/// сразу, как оба подтвердили код, — до обмена; ошибка в нём — обрыв без обмена.
pub fn pair_join(addr: SocketAddr, code: &str, store: &Mutex<Store>, device: &str, name: &str, paired: &OnPaired) -> Result<(Peer, SyncReport)> {
    let (peer, report) = kit::pair_join(&LAN, addr, code, store, device, name, paired)?;
    Ok((peer, report.into()))
}

/// Обмен со связанным устройством по ключу пары.
pub fn sync_peer(addr: SocketAddr, me: &str, peer: &str, peer_name: &str, key: &[u8; 32], store: &Mutex<Store>, name: &str) -> Result<SyncReport> {
    Ok(kit::sync_peer(&LAN, addr, me, peer, peer_name, key, store, name)?.into())
}

/// Приём подключений в фоне: связь по коду и обмен со связанными устройствами.
pub struct Server {
    pub port: u16,
    inner: kit::Server,
}

impl Server {
    /// Слушает `port`; занят — любой свободный.
    pub fn start(host: Host, port: u16) -> Result<Server> {
        let inner = kit::Server::start(host.kit(), port)?;
        Ok(Server { port: inner.port, inner })
    }

    pub fn stop(self) {
        self.inner.stop();
    }
}

/// Отвечает на поиск в сети, пока не остановят.
pub struct Responder {
    pub port: u16,
    inner: kit::Responder,
}

impl Responder {
    /// Слушает `port` (UDP); `tcp_port` — порт сервера для ответа.
    pub fn start(host: Host, port: u16, tcp_port: u16) -> Result<Responder> {
        let inner = kit::Responder::start(host.kit(), port, tcp_port)?;
        Ok(Responder { port: inner.port, inner })
    }

    pub fn stop(self) {
        self.inner.stop();
    }
}

/// Ищет в сети устройства с показанным кодом и связанные устройства (`peers` — номера и ключи пары).
/// `targets` — куда слать запрос: широковещательные адреса сетей и порт поиска.
pub fn discover(targets: &[SocketAddr], me: &str, peers: &[(String, [u8; 32])], wait: Duration) -> Result<Vec<Found>> {
    Ok(kit::discover(&LAN, targets, me, peers, wait)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Error, LanFail};
    use crate::sync::APP_VERSION;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::{TcpStream, UdpSocket};
    use std::sync::MutexGuard;
    use std::time::Instant;

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ключи связи выводятся как в 0.15.0: иначе устройства с прежней версией перестанут
    /// узнавать друг друга. Значения посчитаны версией 0.15.1 и не меняются.
    #[test]
    fn keys_are_derived_as_before() {
        let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let mac = |k: &[u8; 32], parts: &[&[u8]]| LAN.mac(k, parts);
        let k = [0x11u8; 32];
        let (nc, ns) = ([0x22u8; 32], [0x33u8; 32]);
        assert_eq!(hex(mac(&k, &[b"pair key"])), "aabab280e54c704ab6e05232726f10bc96b5c82962d0e967ddc508ad014dfc1f");
        assert_eq!(hex(mac(&k, &[b"joiner confirm"])), "2759a4300e7305a0a46f3352be33d0c35fa3bcece1ecb601f9af51405d044ba7");
        assert_eq!(hex(mac(&k, &[b"host confirm"])), "7193ad7b9e7c6666ca83b8a779f10695bbe7b7ede7eee426d13ee62e304b59b1");
        assert_eq!(hex(mac(&k, &[b"sync c2s", &nc, &ns])), "0aa9e7686201ec9e2dc5eff40091c196097b41b049d03fcf49e7b78f300b5921");
        assert_eq!(hex(mac(&k, &[b"sync s2c", &nc, &ns])), "9edd66e3dae789177ee32b8c9b9bd081ebda5b6657505124dafb3529ecfe31b7");
        assert_eq!(hex(mac(&k, &[b"query", b"aaaa", b"bbbb", &nc])), "f24f3f3795362077b87008ce52825d5f56315dd5528b2e566514fff9d4f2f7a1");
        assert_eq!(hex(mac(&k, &[b"answer", b"bbbb", b"aaaa", &nc, b"47500"])), "a88d68b9754fc83de670a3507cbd17010fa24dd0bb8803085f126ce9e1dab01f");
        assert_eq!((LAN.magic, LAN.query_magic, LAN.answer_magic), (*b"KRML", *b"KRMQ", *b"KRMA"));
        assert_eq!((LAN.joiner, LAN.host), (&b"karman-joiner"[..], &b"karman-host"[..]));
        assert_eq!((TCP_PORT, DISCOVERY_PORT), (47500, 47501));
    }
    use crate::demo;

    struct Side {
        store: Arc<Mutex<Store>>,
        host: Host,
        keys: Arc<Mutex<BTreeMap<String, [u8; 32]>>>,
        log: Arc<Mutex<Vec<(String, std::result::Result<SyncReport, String>)>>>,
    }

    fn side(dir: &std::path::Path, device: &str, name: &str) -> Side {
        let store = Arc::new(Mutex::new(Store::open(&dir.join(format!("{device}.db")), &[5u8; 32], device).unwrap()));
        let keys: Arc<Mutex<BTreeMap<String, [u8; 32]>>> = Arc::default();
        let log: Arc<Mutex<Vec<(String, std::result::Result<SyncReport, String>)>>> = Arc::default();
        let (k1, k2, l) = (keys.clone(), keys.clone(), log.clone());
        let host = Host {
            store: store.clone(),
            device: device.into(),
            name: name.into(),
            code: Arc::default(),
            peer_key: Arc::new(move |id| lock(&k1).get(id).copied()),
            paired: Arc::new(move |p, k| {
                lock(&k2).insert(p.device.clone(), *k);
                Ok(())
            }),
            synced: Arc::new(move |id, r| lock(&l).push((id.to_string(), r.clone()))),
        };
        Side { store, host, keys, log }
    }

    fn local(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    /// Ждём, пока сервер в своём потоке допишет журнал.
    fn wait_log(s: &Side, n: usize) {
        let t0 = Instant::now();
        while lock(&s.log).len() < n && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn pair_by_code_then_sync_by_pair_key() {
        let dir = tempfile::tempdir().unwrap();
        let laptop = side(dir.path(), "laptop", "Ноутбук");
        let phone = side(dir.path(), "phone", "Телефон");
        let ops = demo::ops();
        lock(&laptop.store).put_op(ops[0].clone()).unwrap();
        lock(&phone.store).put_op(ops[1].clone()).unwrap();
        let server = Server::start(laptop.host.clone(), 0).unwrap();
        *lock(&laptop.host.code) = Some(CodeState::new("6WVGHEWK".into()));
        let pk = phone.keys.clone();
        let (peer, r) = pair_join(local(server.port), "6wvg-hewk", &phone.store, "phone", "Телефон", &|p, k| {
            lock(&pk).insert(p.device.clone(), *k);
            Ok(())
        })
        .unwrap();
        assert_eq!((peer.device.as_str(), peer.name.as_str(), peer.app.as_str()), ("laptop", "Ноутбук", APP_VERSION));
        assert_eq!((r.sent, r.received), (1, 1), "первый обмен — сразу при связи");
        wait_log(&laptop, 1);
        let kl = lock(&laptop.keys).get("phone").copied().expect("ноутбук запомнил телефон");
        let kp = lock(&phone.keys).get("laptop").copied().expect("телефон запомнил ноутбук");
        assert_eq!(kl, kp, "ключ пары у обоих один");
        assert!(lock(&laptop.host.code).is_none(), "код сработал один раз");
        assert_eq!(lock(&laptop.store).ops().unwrap().len(), 2);
        // Код больше не действует.
        let again = pair_join(local(server.port), "6WVGHEWK", &phone.store, "phone", "Телефон", &|_, _| Ok(())).unwrap_err();
        assert!(matches!(again, Error::Lan(LanFail::NoCode, _)), "{again}");

        // Обмен по ключу пары — в обе стороны, повторный ничего не передаёт.
        lock(&phone.store).put_op(ops[2].clone()).unwrap();
        let r = sync_peer(local(server.port), "phone", "laptop", "Ноутбук", &kp, &phone.store, "Телефон").unwrap();
        assert_eq!((r.peer.as_str(), r.sent, r.received), ("laptop", 1, 0));
        let r = sync_peer(local(server.port), "phone", "laptop", "Ноутбук", &kp, &phone.store, "Телефон").unwrap();
        assert_eq!((r.sent, r.received), (0, 0));
        // Чужой ключ: ни одной записи, понятная ошибка.
        lock(&phone.store).put_op(ops[3].clone()).unwrap();
        let bad = sync_peer(local(server.port), "phone", "laptop", "Ноутбук", &[7u8; 32], &phone.store, "Телефон").unwrap_err();
        assert!(matches!(bad, Error::Lan(LanFail::NotPeer, _) | Error::Io(_)), "{bad}");
        assert_eq!(lock(&laptop.store).ops().unwrap().len(), 3, "с чужим ключом ничего не пришло");
        // Не связанное устройство и не тот адресат.
        let unknown = sync_peer(local(server.port), "stranger", "laptop", "Ноутбук", &kp, &phone.store, "Чужой").unwrap_err();
        assert!(matches!(unknown, Error::Lan(LanFail::Unknown, ref m) if m.contains("«Ноутбук» не узнало")), "{unknown}");
        let other = sync_peer(local(server.port), "phone", "desktop", "Компьютер", &kp, &phone.store, "Телефон").unwrap_err();
        assert!(matches!(other, Error::Lan(LanFail::NotPeer, _)), "{other}");
        server.stop();
    }

    #[test]
    fn five_wrong_codes_cancel_the_code() {
        let dir = tempfile::tempdir().unwrap();
        let laptop = side(dir.path(), "laptop", "Ноутбук");
        let phone = side(dir.path(), "phone", "Телефон");
        let server = Server::start(laptop.host.clone(), 0).unwrap();
        *lock(&laptop.host.code) = Some(CodeState::new("6WVGHEWK".into()));
        for left in (0..CODE_TRIES).rev() {
            let e = pair_join(local(server.port), "6WVG-HEWM", &phone.store, "phone", "Телефон", &|_, _| panic!("не должно связаться")).unwrap_err();
            assert!(matches!(e, Error::Lan(LanFail::WrongCode(l), _) if l == left), "{e}");
        }
        let st = lock(&laptop.host.code).clone().unwrap();
        assert_eq!((st.left, st.wrong), (0, 5));
        // И верный код больше не проходит.
        let e = pair_join(local(server.port), "6WVGHEWK", &phone.store, "phone", "Телефон", &|_, _| panic!()).unwrap_err();
        assert!(matches!(e, Error::Lan(LanFail::Cancelled, ref m) if m.contains("пять неверных")), "{e}");
        assert!(lock(&laptop.keys).is_empty() && lock(&laptop.store).ops().unwrap().is_empty());
        // Истёкший код — как не показанный.
        *lock(&laptop.host.code) = Some(CodeState { until: Instant::now() - Duration::from_secs(1), ..CodeState::new("6WVGHEWK".into()) });
        let e = pair_join(local(server.port), "6WVGHEWK", &phone.store, "phone", "Телефон", &|_, _| panic!()).unwrap_err();
        assert!(matches!(e, Error::Lan(LanFail::NoCode, _)), "{e}");
        server.stop();
    }

    /// Подставное устройство, которое не знает кода: связь не проходит, и оно ничего не получает.
    #[test]
    fn fake_host_without_code_gets_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fake = side(dir.path(), "fake", "Ноутбук");
        let phone = side(dir.path(), "phone", "Телефон");
        lock(&phone.store).put_op(demo::ops().remove(0)).unwrap();
        let server = Server::start(fake.host.clone(), 0).unwrap();
        // У подставного свой код — человек вводит код с настоящего ноутбука.
        *lock(&fake.host.code) = Some(CodeState::new("ABCDEFGH".into()));
        let e = pair_join(local(server.port), "6WVGHEWK", &phone.store, "phone", "Телефон", &|_, _| panic!()).unwrap_err();
        assert!(matches!(e, Error::Lan(LanFail::WrongCode(4), _)), "{e}");
        assert!(lock(&fake.store).ops().unwrap().is_empty() && lock(&phone.keys).is_empty());
        server.stop();
    }

    #[test]
    fn server_drops_junk_quickly() {
        let dir = tempfile::tempdir().unwrap();
        let laptop = side(dir.path(), "laptop", "Ноутбук");
        let server = Server::start(laptop.host.clone(), 0).unwrap();
        // Огромный открытый запрос — закрыто сразу, без чтения.
        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        s.write_all(b"KRMLP\x01\xff\xff").unwrap();
        let t0 = Instant::now();
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = [0u8; 8];
        assert!(matches!(s.read(&mut buf), Ok(0) | Err(_)));
        assert!(t0.elapsed() < Duration::from_secs(2));
        // Связь по QR-коду прежней версии (сразу шифрованный пакет) — тоже отказ.
        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        s.write_all(&[0, 0, 0, 40]).unwrap();
        s.write_all(b"KRM1").unwrap();
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        assert!(matches!(s.read(&mut buf), Ok(0) | Err(_)));
        // Ни один не связался и не обменялся.
        assert!(lock(&laptop.keys).is_empty() && lock(&laptop.log).is_empty());
        server.stop();
    }

    #[test]
    fn discovery_answers_only_with_code_or_to_peers() {
        let dir = tempfile::tempdir().unwrap();
        let laptop = side(dir.path(), "laptop", "Ноутбук");
        let key = [9u8; 32];
        lock(&laptop.keys).insert("phone".into(), key);
        let resp = Responder::start(laptop.host.clone(), 0, 47_500).unwrap();
        let to = [local(resp.port)];
        let wait = Duration::from_millis(400);
        // Чужому без кода — тишина.
        assert!(discover(&to, "stranger", &[], wait).unwrap().is_empty(), "чужому в сети не отвечаем");
        // Связанному — ответ с адресом, подписанный ключом пары.
        let f = discover(&to, "phone", &[("laptop".into(), key)], wait).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].device.as_str(), f[0].peer, f[0].pairing, f[0].addr), ("laptop", true, false, local(47_500)));
        assert_eq!(f[0].name, "", "имя связанным не нужно и не отдаётся");
        // Подпись чужим ключом — тишина.
        assert!(discover(&to, "phone", &[("laptop".into(), [8u8; 32])], wait).unwrap().is_empty());
        // Показан код — отвечаем всем: имя и порт.
        *lock(&laptop.host.code) = Some(CodeState::new("6WVGHEWK".into()));
        let f = discover(&to, "stranger", &[], wait).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].name.as_str(), f[0].pairing, f[0].peer), ("Ноутбук", true, false));
        // Код отменён — снова тишина.
        lock(&laptop.host.code).as_mut().unwrap().left = 0;
        assert!(discover(&to, "stranger", &[], wait).unwrap().is_empty());
        resp.stop();
    }

    /// Подставной ответ на поиск без ключа пары не принимается.
    #[test]
    fn forged_answer_is_ignored() {
        let fake = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = fake.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            fake.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            for _ in 0..2 {
                let Ok((_, src)) = fake.recv_from(&mut buf) else { return };
                let a = json!({ "t": "peer", "d": "laptop", "p": 47_500, "m": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=" });
                let mut d = b"KRMA\x01".to_vec();
                d.extend(serde_json::to_vec(&a).unwrap());
                fake.send_to(&d, src).unwrap();
            }
        });
        let f = discover(&[local(port)], "phone", &[("laptop".into(), [9u8; 32])], Duration::from_millis(500)).unwrap();
        assert!(f.is_empty(), "{f:?}");
        t.join().unwrap();
    }
}
