//! Сводка с демо-данными из макета — для проверки экранов без приложения.
//! cargo run -p karman-core --example demo_snapshot > demo-snapshot.json

use karman_core::book;
use karman_core::db::Store;
use karman_core::demo;

fn main() {
    let dir = std::env::temp_dir().join(format!("karman-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Store::open(&dir.join("demo.db"), &[9u8; 32], "demo").unwrap();
    book::ensure_defaults(&mut s).unwrap();
    s.append_all(demo::book_events()).unwrap();
    let snap = book::snapshot(&s, demo::today()).unwrap();
    println!("{}", serde_json::to_string_pretty(&snap).unwrap());
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}
