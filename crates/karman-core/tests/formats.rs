//! Форматы «Кармана» закреплены образцами.
//!
//! В `tests/fixtures/v<версия>/` лежит то, что сделала эта версия «Кармана»: зашифрованная база,
//! резервная копия, папка облака и строки (QR-коды связи, ключи копий и облака, хеш PIN-кода,
//! доказательство коннектора). Проверки ниже читают образцы **всех** версий текущим кодом: если
//! после правки старый файл перестал открываться или читается иначе — проверка падает.
//!
//! Образцы не пересоздают. Новая версия добавляет свою папку рядом — `make_fixtures` ниже
//! (запускается руками), старые папки остаются как есть.

use karman_core::backup::{self, BackupKey, Kdf, MadeBy, Reader};
use karman_core::cloud::{self, CloudKey, Dir, MemCloud, Round, State};
use karman_core::db::{blob_id, Store};
use karman_core::pairing::{self, Invite, Pairing, Qr};
use karman_core::sync::EventBody;
use karman_core::mcp;
// PIN-код теперь в общем модуле kit — проверяем тем же кодом, которым его проверяет приложение.
use kit_core::pin;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// Ключ базы-образца, её устройство и пароль копий и облака.
const DB_KEY: [u8; 32] = [0x15; 32];
const DEVICE: &str = "aaaa";
const PASSWORD: &str = "отпуск в Сочи";
/// «Сейчас» для образцов: 8 ноября 2025 года.
const NOW: i64 = 1_762_600_000;
const DIRS: [Dir; 3] = [Dir::Root, Dir::Events, Dir::Blobs];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

/// Папки образцов всех версий.
fn versions() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(fixtures())
        .expect("нет папки образцов")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('v')))
        .collect();
    out.sort();
    assert!(!out.is_empty(), "образцов нет");
    out
}

fn info(dir: &Path) -> Value {
    serde_json::from_slice(&fs::read(dir.join("info.json")).unwrap()).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Записи по номерам — так порядок выдачи не важен.
fn by_id<T: serde::Serialize>(items: Vec<T>) -> Value {
    let mut out = Map::new();
    for item in items {
        let v = serde_json::to_value(item).unwrap();
        out.insert(v["id"].as_str().expect("у записи нет номера").to_string(), v);
    }
    Value::Object(out)
}

/// Всё, что видно в базе: записи каждого вида, настройки, вложения и номера событий.
fn contents(s: &Store) -> Value {
    let blobs: BTreeMap<String, Value> = s
        .present_blobs()
        .unwrap()
        .into_iter()
        .map(|id| {
            let (mime, data) = s.blob(&id).unwrap().unwrap();
            assert_eq!(blob_id(&data), id, "вложение не совпало с отпечатком");
            (id, json!({ "mime": mime, "size": data.len() }))
        })
        .collect();
    json!({
        "ops": by_id(s.ops().unwrap()),
        "trips": by_id(s.trips().unwrap()),
        "accounts": by_id(s.accounts().unwrap()),
        "debts": by_id(s.debts().unwrap()),
        "plans": by_id(s.plans().unwrap()),
        "goals": by_id(s.goals().unwrap()),
        "budgets": by_id(s.budgets().unwrap()),
        "categories": by_id(s.categories().unwrap()),
        "orders": by_id(s.orders().unwrap()),
        "work_days": by_id(s.work_days().unwrap()),
        "settings": s.settings().unwrap(),
        "blobs": blobs,
        "versions": s.version_vector().unwrap(),
        "events": s.event_count().unwrap(),
    })
}

/// Всё из `old` есть в `new` с теми же значениями. Новые поля в `new` допустимы: версии добавляют
/// поля, а убирать или менять прежние нельзя.
fn assert_kept(old: &Value, new: &Value, path: &str) {
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            for (k, v) in o {
                let p = format!("{path}/{k}");
                assert_kept(v, n.get(k).unwrap_or_else(|| panic!("пропало: {p}")), &p);
            }
        }
        (Value::Array(o), Value::Array(n)) => {
            assert_eq!(o.len(), n.len(), "другая длина списка: {path}");
            for (i, (a, b)) in o.iter().zip(n).enumerate() {
                assert_kept(a, b, &format!("{path}/{i}"));
            }
        }
        // Числа сравниваем как числа: 700 и 700.0 — одно и то же.
        (Value::Number(a), Value::Number(b)) => assert_eq!(a.as_f64(), b.as_f64(), "изменилось: {path}"),
        _ => assert_eq!(old, new, "изменилось: {path}"),
    }
}

/// Записи и вложения — как в образце. Число событий не сравниваем: у базы, собранной из копии
/// или облака, оно может отличаться (слитые файлы, свои события устройства).
fn assert_same_records(expected: &Value, store: &Store, what: &str) {
    let got = contents(store);
    for kind in ["ops", "trips", "accounts", "debts", "plans", "goals", "budgets", "categories", "orders", "work_days", "settings", "blobs"] {
        assert_kept(&expected[kind], &got[kind], &format!("{what}/{kind}"));
        assert_eq!(expected[kind].as_object().map(Map::len), got[kind].as_object().map(Map::len), "{what}: другое число записей вида {kind}");
    }
}

fn cloud_from(dir: &Path) -> MemCloud {
    let cloud = MemCloud::default();
    for d in DIRS {
        let folder = dir.join("cloud").join(d.folder());
        for entry in fs::read_dir(&folder).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().is_file()) {
            cloud.set_raw(d, entry.file_name().to_str().unwrap(), fs::read(entry.path()).unwrap());
        }
    }
    cloud
}

#[test]
fn old_database_opens() {
    for dir in versions() {
        let info = info(&dir);
        let tmp = tempfile::tempdir().unwrap();
        // Образец не трогаем: открываем копию.
        let db = tmp.path().join("karman.db");
        fs::copy(dir.join("store.db"), &db).unwrap();
        let store = Store::open(&db, &DB_KEY, DEVICE).unwrap_or_else(|e| panic!("{}: база не открылась: {e}", dir.display()));
        let got = contents(&store);
        assert_kept(&info["contents"], &got, dir.file_name().unwrap().to_str().unwrap());
        assert_eq!(info["contents"]["events"], got["events"], "другое число событий");
        assert_eq!(store.unknown_events().unwrap(), 0, "старые события должны быть понятны");
        assert!(store.missing_blobs().unwrap().is_empty());
        drop(store);
        assert!(matches!(Store::open(&db, &[0x16; 32], DEVICE), Err(karman_core::Error::WrongKey)), "с чужим ключом база не открывается");
    }
}

#[test]
fn old_backup_restores() {
    for dir in versions() {
        let info = info(&dir);
        let file = fs::read(dir.join("backup.karman")).unwrap();
        assert!(Reader::open(file.as_slice(), "другой пароль копий").is_err());
        let checked = backup::verify(Reader::open(file.as_slice(), PASSWORD).unwrap(), "bbbb", |_| Ok(false)).unwrap();
        assert_eq!(checked.manifest.app, info["version"].as_str().unwrap());
        assert_eq!(checked.manifest.device_id, DEVICE);
        assert_eq!(json!(checked.events), info["contents"]["events"]);
        assert_eq!(checked.blobs as usize, info["contents"]["blobs"].as_object().unwrap().len());

        let tmp = tempfile::tempdir().unwrap();
        let mut fresh = Store::open(&tmp.path().join("b.db"), &[0x22; 32], "bbbb").unwrap();
        let restored = backup::restore_into(Reader::open(file.as_slice(), PASSWORD).unwrap(), &mut fresh).unwrap();
        assert_eq!(json!(restored.events), info["contents"]["events"]);
        assert_same_records(&info["contents"], &fresh, "копия");
        assert_eq!(json!(fresh.version_vector().unwrap()), info["contents"]["versions"]);

        // Ключ копий, как он лежит в базе устройства, по-прежнему открывает копию без пароля.
        let key = BackupKey::from_meta(info["strings"]["backup_key"].as_str().unwrap()).unwrap();
        assert!(key.matches(PASSWORD).unwrap());
        assert!(!key.matches("другой пароль копий").unwrap());
        assert!(Reader::open_with_key(file.as_slice(), &key).is_ok());
    }
}

#[test]
fn old_cloud_is_readable() {
    for dir in versions() {
        let info = info(&dir);
        let cloud = cloud_from(&dir);
        assert!(matches!(cloud::join(&cloud, "другой пароль копий", Kdf::DEFAULT, NOW), Err(karman_core::Error::CloudPassword)));
        let (key, created) = cloud::join(&cloud, PASSWORD, Kdf::DEFAULT, NOW).unwrap();
        assert!(!created, "облако уже начато — новый ключ не создаётся");
        assert_eq!(key.group(), info["strings"]["cloud_group"].as_str().unwrap());
        // Ключ облака, как он лежит в базе устройства, — тот же.
        let stored = CloudKey::from_meta(info["strings"]["cloud_key"].as_str().unwrap()).unwrap();
        assert_eq!(stored.to_meta(), key.to_meta());

        // Имена файлов облака — прежние: по ним новое устройство находит старые файлы.
        let names: Vec<Vec<String>> = DIRS.iter().map(|d| cloud.names(*d)).collect();
        assert_eq!(json!(names), info["cloud_files"]);

        let tmp = tempfile::tempdir().unwrap();
        let fresh = Mutex::new(Store::open(&tmp.path().join("c.db"), &[0x33; 32], "cccc").unwrap());
        let cancel = AtomicBool::new(false);
        let report = cloud::sync(&Round { cloud: &cloud, store: &fresh, key: &key, now: NOW + 60, cancel: &cancel }, &mut State::default(), &mut |_| {}).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(json!(report.received), info["contents"]["events"]);
        assert_eq!(report.files_received, info["contents"]["blobs"].as_object().unwrap().len());
        let fresh = fresh.into_inner().unwrap();
        assert_same_records(&info["contents"], &fresh, "облако");
        assert_eq!(json!(fresh.version_vector().unwrap()), info["contents"]["versions"]);
    }
}

#[test]
fn old_strings_are_understood() {
    for dir in versions() {
        let info = info(&dir);
        let s = &info["strings"];
        let text = |k: &str| s[k].as_str().unwrap_or_else(|| panic!("нет строки {k}"));

        // QR-код связи до 0.15: адрес и одноразовый ключ.
        match pairing::parse_qr(text("qr_legacy")).unwrap() {
            Qr::Legacy(p) => assert_eq!(p, legacy_pairing()),
            other => panic!("QR-код прежней версии разобран как {other:?}"),
        }
        // QR-код связи с 0.15: адрес и код связи.
        match pairing::parse_qr(text("qr_invite")).unwrap() {
            Qr::Invite(i) => assert_eq!(i, invite()),
            other => panic!("QR-код с кодом связи разобран как {other:?}"),
        }
        assert_eq!(pairing::normalize_code(text("code_shown")).unwrap(), invite().code);

        // PIN-код: хеш из настроек устройства.
        assert!(pin::verify("2580", text("pin_hash")));
        assert!(!pin::verify("2581", text("pin_hash")));

        // Коннектор для Claude: доказательство ключа связи.
        assert_eq!(mcp::link_proof(&"ab".repeat(32), "karman", &"01".repeat(32)), text("link_proof"));

        // Имя файла событий в облаке.
        assert_eq!(cloud::event_name(DEVICE, 1, 500), text("event_name"));
        let parsed = cloud::parse_event_name(text("event_name")).unwrap();
        assert_eq!((parsed.author.as_str(), parsed.from, parsed.to), (DEVICE, 1, 500));
    }
}

fn legacy_pairing() -> Pairing {
    Pairing { device: "a1b2c3".into(), name: "Ноутбук".into(), hosts: vec!["192.168.1.20".into(), "10.0.0.5".into()], port: 47_123, key: [0x42; 32] }
}

fn invite() -> Invite {
    Invite { device: "a1b2c3".into(), name: "Ноутбук".into(), hosts: vec!["192.168.1.20".into(), "10.0.0.5".into()], port: 47_500, code: "6WVGHEWK".into() }
}

fn jpeg(n: usize, fill: u8) -> Vec<u8> {
    // Не одни и те же байты: так файл не сжимается и занимает в копии больше одного куска.
    let mut d: Vec<u8> = (0..n).map(|i| (i as u32).wrapping_mul(2_654_435_761).to_be_bytes()[0] ^ fill).collect();
    d[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
    d
}

/// Делает образцы текущей версией: `cargo test -p karman-core --test formats -- --ignored make_fixtures`.
/// Готовую папку версии не перезаписывает.
#[test]
#[ignore = "создаёт образцы форматов этой версии; запускается руками"]
fn make_fixtures() {
    let version = env!("CARGO_PKG_VERSION");
    let out = fixtures().join(format!("v{version}"));
    assert!(!out.exists(), "образцы версии {version} уже есть — их не пересоздают");
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("karman.db");

    // База: счета по умолчанию, демо-данные всех видов, своя категория, настройка, удалённая
    // операция и два вложения — маленькое и больше куска копии (1 МБ).
    let mut store = Store::open(&db, &DB_KEY, DEVICE).unwrap();
    karman_core::book::ensure_defaults(&mut store).unwrap();
    store.append_all(karman_core::demo::book_events()).unwrap();
    store.append_all(karman_core::demo::finance_events()).unwrap();
    store.append_all(karman_core::demo::overtime_events()).unwrap();
    store.append(EventBody::SettingPut { key: "per_diem".into(), value: "700".into() }).unwrap();
    let category = karman_core::category::Category { id: "c-sad".into(), name: "Сад".into(), icon: "sprout".into(), extra: Default::default() };
    store.append(EventBody::CategoryPut { category }).unwrap();
    let mut ops = store.ops().unwrap();
    let small = store.put_blob("image/jpeg", &jpeg(40_000, 7)).unwrap();
    let big = store.put_blob("image/jpeg", &jpeg(1_100_000, 9)).unwrap();
    let mut first = ops.remove(0);
    first.att = vec![
        karman_core::trip::Attachment { id: small, mime: "image/jpeg".into(), size: 40_000, name: "чек.jpg".into() },
        karman_core::trip::Attachment { id: big, mime: "image/jpeg".into(), size: 1_100_000, name: "договор.jpg".into() },
    ];
    store.put_op(first).unwrap();
    store.delete_op(&ops.pop().unwrap().id).unwrap();
    let contents = contents(&store);

    // Резервная копия — с обычными параметрами ключа, как у людей.
    let backup_key = BackupKey::new(PASSWORD).unwrap();
    let by = MadeBy { app: version.into(), created: NOW, device: "Ноутбук".into(), device_id: DEVICE.into() };
    let mut backup_file = Vec::new();
    backup::write_store(&store, &backup_key, &by, &mut backup_file).unwrap();

    // Облако: ключ из пароля и один обмен.
    let cloud = MemCloud::default();
    let (cloud_key, created) = cloud::join(&cloud, PASSWORD, Kdf::DEFAULT, NOW).unwrap();
    assert!(created);
    let store = Mutex::new(store);
    let cancel = AtomicBool::new(false);
    cloud::sync(&Round { cloud: &cloud, store: &store, key: &cloud_key, now: NOW, cancel: &cancel }, &mut State::default(), &mut |_| {}).unwrap();
    // База закрыта — журнал записи (WAL) слит в файл.
    drop(store);

    fs::create_dir_all(&out).unwrap();
    fs::copy(&db, out.join("store.db")).unwrap();
    fs::write(out.join("backup.karman"), &backup_file).unwrap();
    for d in DIRS {
        let folder = out.join("cloud").join(d.folder());
        fs::create_dir_all(&folder).unwrap();
        for name in cloud.names(d) {
            fs::write(folder.join(&name), cloud.raw(d, &name).unwrap()).unwrap();
        }
    }
    let names: Vec<Vec<String>> = DIRS.iter().map(|d| cloud.names(*d)).collect();
    let info = json!({
        "version": version,
        "made": "cargo test -p karman-core --test formats -- --ignored make_fixtures",
        "db_key": hex(&DB_KEY),
        "device": DEVICE,
        "password": PASSWORD,
        "contents": contents,
        "cloud_files": names,
        "strings": {
            "qr_legacy": legacy_pairing().to_uri(),
            "qr_invite": invite().to_uri(),
            "code_shown": pairing::show_code(&invite().code),
            "backup_key": backup_key.to_meta(),
            "cloud_key": cloud_key.to_meta(),
            "cloud_group": cloud_key.group(),
            "pin_hash": pin::hash("2580").unwrap(),
            "link_proof": mcp::link_proof(&"ab".repeat(32), "karman", &"01".repeat(32)),
            "event_name": cloud::event_name(DEVICE, 1, 500),
        },
    });
    fs::write(out.join("info.json"), serde_json::to_string_pretty(&info).unwrap() + "\n").unwrap();
}
