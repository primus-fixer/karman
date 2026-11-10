//! Второе устройство для сквозных проверок облака: своя база, тот же сервер WebDAV (имитация).
//!
//! `cargo run -p karman-cloud --features mock --example cloud_peer -- <адрес WebDAV> <папка> <пароль копий> [<название> <сумма в копейках>]`
//!
//! Подключается к облаку паролем копий, обменивается и печатает, что пришло (JSON).
//! С названием и суммой — ещё записывает операцию (по образцу первой пришедшей) и обменивается снова.

use karman_cloud::dav::{self, Dav};
use karman_cloud::mock::{DAV_PASSWORD, DAV_USER};
use karman_core::backup::Kdf;
use karman_core::cloud::{self, CloudKey, Round, State};
use karman_core::db::Store;
use serde_json::json;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

fn round(d: &Dav, st: &Mutex<Store>, key: &CloudKey, state: &mut State) -> cloud::Report {
    let cancel = AtomicBool::new(false);
    cloud::sync(&Round { cloud: d, store: st, key, now: karman_cloud::now(), cancel: &cancel }, state, &mut |_| {}).expect("обмен")
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let url = dav::server_url(&a[1], true).expect("адрес");
    let dir = std::path::PathBuf::from(&a[2]);
    std::fs::create_dir_all(&dir).expect("папка");
    let d = Dav::new(&url, DAV_USER, DAV_PASSWORD);
    let mut store = Store::open(&dir.join("peer.db"), &[5u8; 32], "c0ffee00c0ffee00").expect("база");
    karman_core::book::ensure_defaults(&mut store).expect("справочники");
    let st = Mutex::new(store);
    let key_file = dir.join("cloud.key");
    let key = match std::fs::read_to_string(&key_file) {
        Ok(k) => CloudKey::from_meta(&k).expect("ключ"),
        Err(_) => {
            let (k, _) = cloud::join(&d, &a[3], Kdf::DEFAULT, karman_cloud::now()).expect("пароль копий");
            std::fs::write(&key_file, k.to_meta()).expect("ключ");
            k
        }
    };
    let state_file = dir.join("state.json");
    let mut state: State = std::fs::read_to_string(&state_file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let first = round(&d, &st, &key, &mut state);
    let mut sent = 0;
    if let (Some(title), Some(amount)) = (a.get(4), a.get(5).and_then(|s| s.parse::<i64>().ok())) {
        let mut s = st.lock().expect("база");
        let mut op = s.ops().expect("операции").into_iter().next().expect("операции пришли");
        op.id = format!("peer-{}", karman_cloud::now());
        op.title = title.clone();
        op.amount = amount;
        op.att.clear();
        s.put_op(op).expect("запись");
        drop(s);
        sent = round(&d, &st, &key, &mut state).sent;
    }
    std::fs::write(&state_file, serde_json::to_string(&state).expect("состояние")).expect("состояние");
    let s = st.lock().expect("база");
    let ops = s.ops().expect("операции");
    let titles: Vec<String> = ops.iter().map(|o| o.title.clone()).collect();
    let blobs: usize = ops.iter().map(|o| o.att.len()).sum();
    let missing = s.missing_blobs().expect("снимки").len();
    println!("{}", json!({"received": first.received, "files_received": first.files_received, "sent": sent, "ops": titles, "attachments": blobs, "missing": missing}));
}
