//! Прогоняет через настоящее ядро команды, которые отправил интерфейс в проверке экранов.
//! Ловит расхождения формата: всё, что фронтенд шлёт в Tauri, должно читаться ядром.
//! cargo run -p karman-core --example replay -- calls-phone.json [calls-desktop.json …]

use karman_core::book::{self, Account};
use karman_core::db::Store;
use karman_core::sync::EventBody;
use karman_core::trip::{Op, Trip};
use karman_core::{demo, Date};
use serde_json::Value;

fn text(a: &Value, key: &str) -> Result<String, String> {
    a.get(key).and_then(|v| v.as_str()).map(str::to_string).ok_or_else(|| format!("нет поля {key}"))
}

/// Повторяет команды из app/src-tauri/src/lib.rs. Возвращает false для команд только на чтение.
fn apply(s: &mut Store, cmd: &str, a: &Value, today: Date) -> Result<bool, String> {
    let e = |x: karman_core::Error| x.to_string();
    match cmd {
        "op_save" => {
            let mut op: Op = serde_json::from_value(a["op"].clone()).map_err(|x| format!("Op: {x}"))?;
            if op.amount <= 0 {
                return Err("сумма должна быть больше нуля".into());
            }
            if op.id.is_empty() {
                op.id = book::new_id().map_err(e)?;
            }
            s.put_op(op).map_err(e)?;
        }
        "op_delete" => {
            s.delete_op(&text(a, "id")?).map_err(e)?;
        }
        "trip_save" => {
            let trip: Trip = serde_json::from_value(a["trip"].clone()).map_err(|x| format!("Trip: {x}"))?;
            if trip.id.trim().is_empty() || trip.city.trim().is_empty() || trip.end < trip.start || trip.plan_end < trip.plan_start {
                return Err("поездка не прошла проверку".into());
            }
            s.append(EventBody::TripPut { trip }).map_err(e)?;
        }
        "trip_delete" => {
            let id = text(a, "id")?;
            let mut bodies: Vec<EventBody> = s.ops().map_err(e)?.into_iter().filter(|o| o.trip.as_deref() == Some(id.as_str())).map(|o| EventBody::OpDelete { id: o.id }).collect();
            bodies.push(EventBody::TripDelete { id });
            s.append_all(bodies).map_err(e)?;
        }
        "trip_advance" => {
            book::advance_trip(s, &text(a, "id")?, today).map_err(e)?;
        }
        "trip_settle" => {
            book::settle_trip(s, &text(a, "id")?, today).map_err(e)?;
        }
        "account_save" => {
            let mut account: Account = serde_json::from_value(a["account"].clone()).map_err(|x| format!("Account: {x}"))?;
            if account.id.is_empty() {
                account.id = format!("acc-{}", book::new_id().map_err(e)?);
            }
            s.append(EventBody::AccountPut { account }).map_err(e)?;
        }
        "setting_set" => {
            s.append(EventBody::SettingPut { key: text(a, "key")?, value: text(a, "value")? }).map_err(e)?;
        }
        _ => return Ok(false),
    }
    book::snapshot(s, today).map_err(e)?;
    Ok(true)
}

fn main() {
    let mut failed = 0;
    for file in std::env::args().skip(1) {
        let dir = std::env::temp_dir().join(format!("karman-replay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Store::open(&dir.join("replay.db"), &[7u8; 32], "replay").unwrap();
        book::ensure_defaults(&mut s).unwrap();
        s.append_all(demo::book_events()).unwrap();
        let calls: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let mut applied = 0;
        let mut last_today = demo::today();
        for c in &calls {
            let cmd = c["cmd"].as_str().unwrap_or("");
            let a = &c["args"];
            let today = a.get("today").and_then(|v| v.as_str()).and_then(Date::parse).unwrap_or(demo::today());
            last_today = today;
            match apply(&mut s, cmd, a, today) {
                Ok(true) => applied += 1,
                Ok(false) => {}
                Err(err) => {
                    failed += 1;
                    println!("✗ {cmd}: {err}\n  {a}");
                }
            }
        }
        let snap = book::snapshot(&s, last_today).unwrap();
        let json = serde_json::to_value(&snap).unwrap();
        println!(
            "{file}: применено {applied} изменений из {} вызовов; операций {}, поездок {}, «мои деньги» {}",
            calls.len(),
            json["ops"].as_array().map(|x| x.len()).unwrap_or(0),
            json["trips"].as_array().map(|x| x.len()).unwrap_or(0),
            json["mine"]
        );
        for t in json["trips"].as_array().unwrap() {
            println!("  {} {}: {}, баланс {}", t["trip"]["id"].as_str().unwrap(), t["trip"]["city"].as_str().unwrap(), t["trip"]["status"].as_str().unwrap(), t["s"]["balance"]);
        }
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
    if failed > 0 {
        println!("ошибок: {failed}");
        std::process::exit(1);
    }
}
