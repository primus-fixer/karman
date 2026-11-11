//! Выгрузка в Excel на демо-данных — проверить файл глазами.
//! cargo run -p karman-core --example export_demo -- demo.xlsx

use karman_core::book;
use karman_core::db::Store;
use karman_core::demo;
use karman_core::stats::{stats, Span};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "demo.xlsx".into());
    let dir = std::env::temp_dir().join(format!("karman-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Store::open(&dir.join("demo.db"), &[9u8; 32], "demo").unwrap();
    book::ensure_defaults(&mut s).unwrap();
    s.append_all(demo::book_events()).unwrap();
    let ops = s.ops().unwrap();
    let accounts = s.accounts().unwrap();
    let st = stats(&ops, &accounts, &s.trips().unwrap(), book::settings_rate(&s.settings().unwrap()), Span::Month, demo::today(), demo::today());
    std::fs::write(&out, karman_core::export::workbook(&ops, &accounts, &[], &st).unwrap()).unwrap();
    println!("{out}");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}
