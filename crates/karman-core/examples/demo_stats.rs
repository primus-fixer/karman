//! Статистика на демо-данных за разные периоды — для проверки экрана «Статистика» без приложения.
//! cargo run -p karman-core --example demo_stats > demo-stats.json
//! Ключ — «месяц|квартал|год:первый день периода», например «month:2026-11-01».

use karman_core::book;
use karman_core::date::Date;
use karman_core::db::Store;
use karman_core::demo;
use karman_core::stats::{stats, Span};
use std::collections::BTreeMap;

fn main() {
    let dir = std::env::temp_dir().join(format!("karman-stats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Store::open(&dir.join("demo.db"), &[9u8; 32], "demo").unwrap();
    book::ensure_defaults(&mut s).unwrap();
    s.append_all(demo::book_events()).unwrap();
    let ops = s.ops().unwrap();
    let accounts = s.accounts().unwrap();
    let trips = s.trips().unwrap();
    let rate = book::settings_rate(&s.settings().unwrap());
    let today = demo::today();
    let mut out = BTreeMap::new();
    for (span, name) in [(Span::Month, "month"), (Span::Quarter, "quarter"), (Span::Year, "year")] {
        let mut at = today;
        // Назад на три года — дальше пусто.
        while at >= Date::parse("2024-01-01").unwrap() {
            let st = stats(&ops, &accounts, &trips, rate, span, at, today);
            let from = st.from;
            out.insert(format!("{name}:{}", from), st);
            at = from.add_days(-1);
        }
    }
    println!("{}", serde_json::to_string(&out).unwrap());
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}
