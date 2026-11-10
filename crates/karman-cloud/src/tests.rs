//! Проверки на имитации серверов: вход по коду и по паролю (WebDAV), файлы, обновление входа,
//! ошибки и обмен целиком.

use super::*;
use crate::dav::{self, Dav};
use crate::mock::{Mock, DAV_PASSWORD, DAV_USER};
use karman_core::backup::Kdf;
use karman_core::cloud::{self, Cloud, Dir, Round, State};
use karman_core::db::Store;
use karman_core::error::Error;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

const FAST: Kdf = Kdf { m_kib: 8 * 1024, t: 1, p: 1 };
const BOTH: [Provider; 2] = [Provider::Yandex, Provider::Google];

fn auth(m: &Mock, p: Provider) -> Auth {
    Auth::new(p, Client { id: "client-id".into(), secret: "client-secret".into() }, Endpoints::mock(p, &m.base))
}

/// Вход по коду до конца.
fn sign_in(m: &Mock, p: Provider) -> Tokens {
    let a = auth(m, p);
    let code = a.start("karman-0123456789abcdef", "Ноутбук").unwrap();
    assert!(code.url.ends_with("/device") && !code.user_code.is_empty());
    assert!(matches!(a.poll(&code).unwrap(), Poll::Pending), "код ещё не ввели");
    m.approve(true);
    match a.poll(&code).unwrap() {
        Poll::Done(t) => t,
        _ => panic!("вход не завершился"),
    }
}

fn session(m: &Mock, p: Provider) -> Session {
    let t = sign_in(m, p);
    Session::new(auth(m, p), t)
}

#[test]
fn sign_in_by_code_and_refusal() {
    for p in BOTH {
        let m = Mock::start();
        let t = sign_in(&m, p);
        assert!(!t.access.is_empty() && !t.refresh.is_empty() && t.expires > now());
        let m = Mock::start();
        let a = auth(&m, p);
        let code = a.start("karman-0123456789abcdef", "Телефон").unwrap();
        m.deny();
        assert!(matches!(a.poll(&code), Err(Error::Cloud(CloudFail::Auth, msg)) if msg.contains("не разрешили")));
        // Без секрета приложения сервер не пускает.
        let bad = Auth::new(p, Client { id: "client-id".into(), secret: String::new() }, Endpoints::mock(p, &m.base));
        let code = bad.start("karman-0123456789abcdef", "Телефон").unwrap();
        assert!(matches!(bad.poll(&code), Err(Error::Cloud(CloudFail::Auth, msg)) if msg.contains("регистрацию")));
    }
}

#[test]
fn files_in_every_folder() {
    for p in BOTH {
        let m = Mock::start();
        let s = session(&m, p);
        assert!(s.list(Dir::Events).unwrap().is_empty(), "{p:?}: папки ещё нет — список пустой");
        assert_eq!(s.get(Dir::Root, cloud::KEY_FILE, 1024).unwrap(), None);
        assert!(!s.exists(Dir::Root, cloud::KEY_FILE).unwrap());
        s.put(Dir::Root, cloud::KEY_FILE, b"{\"k\":1}").unwrap();
        s.put(Dir::Events, "aaaa-1-3.kev", &[1, 2, 3]).unwrap();
        let big = vec![7u8; 6 << 20];
        s.put(Dir::Blobs, "b1.kbl", &big).unwrap();
        // Поверх прежнего — тот же файл, без двойников.
        s.put(Dir::Events, "aaaa-1-3.kev", &[4, 5]).unwrap();
        assert!(s.exists(Dir::Root, cloud::KEY_FILE).unwrap());
        assert_eq!(s.get(Dir::Events, "aaaa-1-3.kev", 1024).unwrap(), Some(vec![4, 5]));
        assert_eq!(s.get(Dir::Blobs, "b1.kbl", 7 << 20).unwrap().map(|d| d.len()), Some(6 << 20));
        assert!(matches!(s.get(Dir::Blobs, "b1.kbl", 1024), Err(Error::Cloud(CloudFail::Format, _))), "больше предела — не скачиваем");
        let names: Vec<String> = s.list(Dir::Events).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["aaaa-1-3.kev"]);
        assert_eq!(s.list(Dir::Blobs).unwrap()[0].size, 6 << 20);
        s.delete(Dir::Events, "aaaa-1-3.kev").unwrap();
        s.delete(Dir::Events, "aaaa-1-3.kev").unwrap();
        assert!(s.list(Dir::Events).unwrap().is_empty());
        let files: Vec<String> = m.files().into_iter().map(|(n, _)| n).collect();
        let want: &[&str] = match p {
            Provider::Yandex => &["yandex:app:/b/b1.kbl", "yandex:app:/karman.key"],
            Provider::Google => &["google:Карман/b/b1.kbl", "google:Карман/karman.key"],
        };
        assert_eq!(files, want, "{p:?}");
    }
}

#[test]
fn long_lists_are_read_page_by_page() {
    for p in BOTH {
        let m = Mock::start();
        let s = session(&m, p);
        for i in 1..=5 {
            s.put(Dir::Events, &cloud::event_name("aaaa", i, i), b"x").unwrap();
        }
        m.small_pages(true);
        let fresh = Session::new(auth(&m, p), s.tokens());
        assert_eq!(fresh.list(Dir::Events).unwrap().len(), 5, "{p:?}");
    }
}

#[test]
fn expired_access_is_renewed_and_revoked_access_asks_to_sign_in() {
    for p in BOTH {
        let m = Mock::start();
        let s = session(&m, p);
        s.put(Dir::Root, cloud::KEY_FILE, b"1").unwrap();
        let before = s.tokens();
        m.expire();
        assert!(s.exists(Dir::Root, cloud::KEY_FILE).unwrap(), "{p:?}: доступ обновился сам");
        assert!(s.tokens_changed());
        let after = s.tokens();
        assert_ne!(after.access, before.access);
        assert_eq!(after.refresh, before.refresh, "refresh-токен остаётся");
        // Доступ, который вот-вот истечёт, обновляется заранее.
        let soon = Session::new(auth(&m, p), Tokens { expires: now() + 30, ..after.clone() });
        assert!(soon.exists(Dir::Root, cloud::KEY_FILE).unwrap());
        assert!(soon.tokens_changed());
        m.revoke();
        let gone = Session::new(auth(&m, p), soon.tokens());
        assert!(matches!(gone.list(Dir::Events), Err(Error::Cloud(CloudFail::Auth, _))), "{p:?}");
    }
}

#[test]
fn full_disk_and_no_connection() {
    for p in BOTH {
        let m = Mock::start();
        let s = session(&m, p);
        s.put(Dir::Events, "aaaa-1-1.kev", b"x").unwrap();
        m.full(true);
        assert!(matches!(s.put(Dir::Blobs, "b.kbl", b"photo"), Err(Error::Cloud(CloudFail::Space, _))), "{p:?}");
        let t = s.tokens();
        let base = m.base.clone();
        drop(m);
        let offline = Session::new(Auth::new(p, Client { id: "i".into(), secret: "s".into() }, Endpoints::mock(p, &base)), t);
        assert!(matches!(offline.list(Dir::Events), Err(Error::Cloud(CloudFail::Offline, _))), "{p:?}");
    }
}

fn store(dir: &std::path::Path, name: &str, device: &str, demo: bool) -> Mutex<Store> {
    let mut s = Store::open(&dir.join(name), &[3u8; 32], device).unwrap();
    karman_core::book::ensure_defaults(&mut s).unwrap();
    if demo {
        s.append_all(karman_core::demo::book_events()).unwrap();
        let mut photo = vec![9u8; 80_000];
        photo[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
        let id = s.put_blob("image/jpeg", &photo).unwrap();
        let mut op = s.ops().unwrap().remove(0);
        op.att = vec![karman_core::trip::Attachment { id, mime: "image/jpeg".into(), size: 80_000, name: "чек.jpg".into() }];
        s.put_op(op).unwrap();
    }
    Mutex::new(s)
}

fn round(c: &dyn Cloud, st: &Mutex<Store>, key: &cloud::CloudKey, state: &mut State) -> cloud::Report {
    let cancel = AtomicBool::new(false);
    cloud::sync(&Round { cloud: c, store: st, key, now: now(), cancel: &cancel }, state, &mut |_| {}).unwrap()
}

/// Обмен целиком через настоящие запросы: компьютер и телефон.
fn exchange(c1: &dyn Cloud, c2: &dyn Cloud, label: &str) {
    let dir = tempfile::tempdir().unwrap();
    let pc = store(dir.path(), "pc.db", "aaaa", true);
    let phone = store(dir.path(), "phone.db", "bbbb", false);
    let (k1, created) = cloud::join(c1, "пароль копий", FAST, now()).unwrap();
    assert!(created);
    let (k2, created) = cloud::join(c2, "пароль копий", FAST, now()).unwrap();
    assert!(!created);
    let (mut st1, mut st2) = (State::default(), State::default());
    let r = round(c1, &pc, &k1, &mut st1);
    assert_eq!(r.files_sent, 1, "{label}");
    let r = round(c2, &phone, &k2, &mut st2);
    assert_eq!((r.files_received, r.sent), (1, 3), "{label}");
    round(c1, &pc, &k1, &mut st1);
    let (a, b) = (pc.lock().unwrap(), phone.lock().unwrap());
    assert_eq!(a.version_vector().unwrap(), b.version_vector().unwrap(), "{label}");
    assert_eq!(serde_json::to_value(a.ops().unwrap()).unwrap(), serde_json::to_value(b.ops().unwrap()).unwrap());
    assert!(b.missing_blobs().unwrap().is_empty());
    // В облаке — только зашифрованное: названий операций там не найти.
    let title = a.ops().unwrap()[0].title.clone();
    for e in c1.list(Dir::Events).unwrap() {
        let data = c1.get(Dir::Events, &e.name, 1 << 26).unwrap().unwrap();
        assert!(!String::from_utf8_lossy(&data).contains(&title), "{label}");
    }
}

#[test]
fn phone_and_computer_exchange_through_each_cloud() {
    for p in BOTH {
        let m = Mock::start();
        let s1 = session(&m, p);
        let s2 = Session::new(auth(&m, p), sign_in(&m, p));
        exchange(&s1, &s2, p.title());
    }
    let m = Mock::start();
    exchange(&dav(&m), &dav(&m), "WebDAV");
    assert!(m.files().iter().all(|(n, _)| n.starts_with("dav:/Карман/")), "всё — в папке «Карман»");
}

// ——— WebDAV: Яндекс Диск по паролю приложения, свой сервер ———

fn dav(m: &Mock) -> Dav {
    Dav::new(&dav::server_url(&m.dav_url(), true).unwrap(), DAV_USER, DAV_PASSWORD)
}

#[test]
fn webdav_sign_in_with_password() {
    let m = Mock::start();
    let url = dav::server_url(&m.dav_url(), true).unwrap();
    let wrong = Dav::new(&url, DAV_USER, "not-the-password");
    assert!(matches!(wrong.connect(), Err(Error::Cloud(CloudFail::Auth, msg)) if msg.contains("не принял")));
    assert!(m.dav_dirs().is_empty(), "без входа ничего не создаётся");
    let d = dav(&m);
    assert!(!d.connect().unwrap(), "данных «Кармана» ещё нет");
    assert_eq!(m.dav_dirs(), ["/Карман"]);
    assert!(!dav(&m).connect().unwrap(), "папка уже есть — не ошибка");
    d.put(Dir::Root, cloud::KEY_FILE, b"{}").unwrap();
    assert!(dav(&m).connect().unwrap(), "в облаке уже есть данные: пароль копий вводят, а не придумывают");
    let nowhere = Dav::new(&format!("{}/nowhere", m.base), DAV_USER, DAV_PASSWORD);
    assert!(matches!(nowhere.connect(), Err(Error::Cloud(CloudFail::Auth, msg)) if msg.contains("адрес")));
    m.dav_forbid(true);
    assert!(matches!(dav(&m).connect(), Err(Error::Cloud(CloudFail::Auth, msg)) if msg.contains("403")));
}

#[test]
fn webdav_files_in_every_folder() {
    let m = Mock::start();
    let s = dav(&m);
    assert!(s.list(Dir::Events).unwrap().is_empty(), "папки ещё нет — список пустой");
    assert_eq!(s.get(Dir::Root, cloud::KEY_FILE, 1024).unwrap(), None);
    assert!(!s.exists(Dir::Root, cloud::KEY_FILE).unwrap());
    s.put(Dir::Root, cloud::KEY_FILE, b"{\"k\":1}").unwrap();
    s.put(Dir::Events, "aaaa-1-3.kev", &[1, 2, 3]).unwrap();
    let big = vec![7u8; 6 << 20];
    s.put(Dir::Blobs, "b1.kbl", &big).unwrap();
    s.put(Dir::Events, "aaaa-1-3.kev", &[4, 5]).unwrap();
    assert!(s.exists(Dir::Root, cloud::KEY_FILE).unwrap());
    assert!(!s.exists(Dir::Root, "e").unwrap(), "папка — не файл");
    assert_eq!(s.get(Dir::Events, "aaaa-1-3.kev", 1024).unwrap(), Some(vec![4, 5]));
    assert_eq!(s.get(Dir::Blobs, "b1.kbl", 7 << 20).unwrap().map(|d| d.len()), Some(6 << 20));
    assert!(matches!(s.get(Dir::Blobs, "b1.kbl", 1024), Err(Error::Cloud(CloudFail::Format, _))), "больше предела — не скачиваем");
    let names: Vec<String> = s.list(Dir::Events).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["aaaa-1-3.kev"]);
    assert_eq!(s.list(Dir::Blobs).unwrap()[0].size, 6 << 20);
    assert_eq!(s.list(Dir::Root).unwrap().into_iter().map(|e| e.name).collect::<Vec<_>>(), [cloud::KEY_FILE], "папки e и b — не файлы");
    s.delete(Dir::Events, "aaaa-1-3.kev").unwrap();
    s.delete(Dir::Events, "aaaa-1-3.kev").unwrap();
    assert!(s.list(Dir::Events).unwrap().is_empty());
    let files: Vec<String> = m.files().into_iter().map(|(n, _)| n).collect();
    assert_eq!(files, ["dav:/Карман/b/b1.kbl", "dav:/Карман/karman.key"]);
    // Папку «Карман» удалили в облаке, пока «Карман» работал: она создаётся снова.
    m.dav_remove("/Карман");
    s.put(Dir::Events, "aaaa-4-4.kev", b"again").unwrap();
    assert_eq!(s.get(Dir::Events, "aaaa-4-4.kev", 1024).unwrap().as_deref(), Some(&b"again"[..]));
    // Данные удалили: пустые папки «Кармана» убираются, чужие файлы остаются.
    s.put(Dir::Root, "заметки.txt", b"my notes").unwrap();
    cloud::wipe(&s, &AtomicBool::new(false)).unwrap();
    s.tidy().unwrap();
    assert_eq!(m.files().into_iter().map(|(n, _)| n).collect::<Vec<_>>(), ["dav:/Карман/заметки.txt"]);
    assert_eq!(m.dav_dirs(), ["/Карман"]);
    s.delete(Dir::Root, "заметки.txt").unwrap();
    s.tidy().unwrap();
    assert!(m.dav_dirs().is_empty());
}

#[test]
fn webdav_full_disk_new_password_and_no_connection() {
    let m = Mock::start();
    let s = dav(&m);
    s.put(Dir::Events, "aaaa-1-1.kev", b"x").unwrap();
    m.full(true);
    assert!(matches!(s.put(Dir::Blobs, "b.kbl", b"photo"), Err(Error::Cloud(CloudFail::Space, _))));
    m.full(false);
    m.dav_password(Some("another-app-password"));
    assert!(matches!(s.list(Dir::Events), Err(Error::Cloud(CloudFail::Auth, _))), "пароль приложения удалили — войти снова");
    m.dav_password(None);
    m.dav_forbid(true);
    assert!(matches!(s.exists(Dir::Root, cloud::KEY_FILE), Err(Error::Cloud(CloudFail::Auth, _))));
    let url = dav::server_url(&m.dav_url(), true).unwrap();
    drop(m);
    assert!(matches!(Dav::new(&url, DAV_USER, DAV_PASSWORD).list(Dir::Events), Err(Error::Cloud(CloudFail::Offline, _))));
}

#[test]
fn mock_answers_like_the_clouds() {
    // Ссылка на скачивание с Яндекс Диска ведёт через переадресацию — её надо пройти без входа.
    let m = Mock::start();
    let s = session(&m, Provider::Yandex);
    s.put(Dir::Events, "aaaa-1-1.kev", b"redirect").unwrap();
    assert_eq!(s.get(Dir::Events, "aaaa-1-1.kev", 1024).unwrap().as_deref(), Some(&b"redirect"[..]));
    assert!(m.requests() > 0);
}
