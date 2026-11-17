//! Коннектор «Кармана» к приложению Claude. Протокол MCP (JSON-RPC 2.0 построчно через stdio), доказательство
//! ключа связи и файл расширения `.mcpb` — общие из kit (`kit_core::mcp`); здесь то, чем «Карман» отличается:
//! имя, подсказка для Claude, строка ключа связи, описание расширения и его инструменты ([`crate::tools`]).
//! Сам вызов инструмента — снаружи: посредник `karman --mcp` передаёт его запущенному «Карману».

use crate::tools;
use kit_core::mcp::{self as kit, Extension, Server, Tool};
use serde_json::Value;

pub use kit::{bundle, VERSIONS};

/// Подсказка для Claude: что это за данные и как с ними обращаться.
pub const INSTRUCTIONS: &str = "«Карман» — личные финансы и командировки пользователя. Суммы — в рублях, даты — ГГГГ-ММ-ДД. \
Начинайте со сводки (summary). Деньги поездки (подотчётные, аванс, суточные, расходы для отчёта 1С) и личные деньги считаются отдельно. \
Перед записью уточняйте у человека то, чего не знаете: поездка это или личное, категория, сумма, дата. \
Каждую запись человек видит в «Кармане» и может отменить кнопкой «Вернуть». \
Наряд-допуск со снимка или PDF: прочитайте его сами и передайте прочитанное в order_days как есть — время дней по правилам таблицы переработок посчитает «Карман». \
Покажите человеку дни таблицей и записывайте через save_order только после его согласия; по одному дню через set_work_day наряд не вписывайте. \
Снимки наряда «Карман» из разговора не получает — напомните добавить их в «Кармане». \
Если «Карман» заблокирован или запись выключена, скажите человеку, что нужно сделать в «Кармане».";

/// Коннектор «Кармана». Строку ключа связи менять нельзя: посредник прежней версии и запущенный «Карман»
/// перестанут узнавать друг друга.
pub const SERVER: Server = Server { name: "karman", title: "Карман", instructions: INSTRUCTIONS, link_personal: b"karman-link" };

/// Расширение «Кармана» для приложения Claude.
pub const EXTENSION: Extension = Extension {
    display_name: "Карман",
    description: "Личные деньги и командировки из «Кармана»: сводка, операции, поездки, переработки. Запись — если она разрешена в «Кармане».",
    long_description: "Расширение запускает «Карман» на этом компьютере в режиме коннектора. Данные не уходят в интернет, кроме того, что Claude прочитает по вашей просьбе. Включается в «Кармане»: Настройки → Claude.",
    author: "Карман",
    keywords: &["finance", "business trips", "overtime"],
    dir_env: "KARMAN_DIR",
};

/// Инструменты «Кармана» для протокола: удаляет только `delete_operation`; повторять можно чтение и `set_work_day`.
pub fn tools() -> Vec<Tool> {
    tools::list()
        .into_iter()
        .map(|t| Tool {
            destructive: t.name == "delete_operation",
            idempotent: t.name == "set_work_day",
            name: t.name,
            title: t.title,
            description: t.description,
            input_schema: t.input_schema,
            write: t.write,
        })
        .collect()
}

/// Доказательство ключа связи посредника (`karman --mcp`) и запущенного «Кармана»: MAC от одноразового
/// числа другой стороны. Сам ключ по сети не ходит, и программа, занявшая порт, ничего не узнает.
pub fn link_proof(token: &str, role: &str, nonce: &str) -> String {
    kit::link_proof(&SERVER, token, role, nonce)
}

/// Описание инструментов для `tools/list`.
pub fn tool_list() -> Value {
    kit::tool_list(&tools())
}

/// Ответ на одну строку запроса; `None` — для уведомлений, на них не отвечают.
/// `call(имя, аргументы)` выполняет инструмент: `Ok` — данные, `Err` — понятная человеку ошибка.
pub fn handle(line: &str, version: &str, call: &mut dyn FnMut(&str, &Value) -> Result<Value, String>) -> Option<String> {
    kit::handle(&SERVER, &tools(), line, version, call)
}

/// Описание расширения для приложения Claude (`manifest.json` внутри `.mcpb`).
/// `command` — полный путь к `karman.exe`, `dir` — папка данных «Кармана», `platform` — win32, darwin или linux.
pub fn manifest(version: &str, command: &str, dir: &str, platform: &str) -> Value {
    kit::manifest(&SERVER, &EXTENSION, &tools(), version, command, dir, platform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn link_proof_depends_on_key_role_and_nonce() {
        let token = "ab".repeat(32);
        let p = link_proof(&token, "karman", "01".repeat(32).as_str());
        assert_eq!(p.len(), 64);
        assert_eq!(p, link_proof(&token, "karman", "01".repeat(32).as_str()), "одинаково для одних и тех же данных");
        assert_ne!(p, link_proof(&token, "bridge", "01".repeat(32).as_str()), "у сторон разные доказательства");
        assert_ne!(p, link_proof(&token, "karman", "02".repeat(32).as_str()));
        assert_ne!(p, link_proof(&"cd".repeat(32), "karman", "01".repeat(32).as_str()));
        assert!(!p.contains(&token));
        assert_eq!(link_proof(&"x".repeat(65), "karman", "01"), "", "ключ длиннее 64 байт не подходит");
        assert_eq!(link_proof("", "karman", "01"), "");
    }

    fn run(line: &str) -> Value {
        let mut calls = Vec::new();
        let out = handle(line, "0.11.0", &mut |name, args| {
            calls.push((name.to_string(), args.clone()));
            if name == "summary" {
                Ok(json!({ "mine": 142513.5 }))
            } else {
                Err("«Карман» заблокирован".into())
            }
        });
        serde_json::from_str(&out.expect("ответ")).unwrap()
    }

    #[test]
    fn handshake_lists_tools_and_calls_them() {
        let r = run(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-ai","version":"1"}}}"#);
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18", "версию клиента повторяем, если знаем её");
        assert_eq!(r["result"]["serverInfo"]["name"], "karman");
        let r = run(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#);
        assert_eq!(r["result"]["protocolVersion"], VERSIONS[0], "незнакомую — заменяем своей новой");
        assert_eq!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, "x", &mut |_, _| Ok(json!(null))), None);
        let r = run(r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#);
        let list = r["result"]["tools"].as_array().unwrap();
        assert_eq!(list.len(), tools::list().len());
        let add = list.iter().find(|t| t["name"] == "add_operation").unwrap();
        assert_eq!(add["annotations"]["readOnlyHint"], false);
        assert_eq!(add["inputSchema"]["required"][0], "type");
        let r = run(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"summary","arguments":{}}}"#);
        assert_eq!(r["result"]["isError"], false);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("142513.5"));
        let r = run(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"accounts"}}"#);
        assert_eq!(r["result"]["isError"], true, "ошибка инструмента — в ответе, а не сбоем протокола");
        assert_eq!(r["result"]["content"][0]["text"], "«Карман» заблокирован");
        let r = run(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"rm_rf"}}"#);
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(run(r#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#)["result"], json!({}));
        assert_eq!(run(r#"{"jsonrpc":"2.0","id":7,"method":"sampling/createMessage"}"#)["error"]["code"], -32601);
        assert_eq!(run("не json")["error"]["code"], -32700);
        assert_eq!(run("[1,2]")["error"]["code"], -32600);
    }

    #[test]
    fn bundle_is_a_zip_with_a_valid_manifest() {
        let m = manifest("0.15.3", r"C:\Users\Пётр\AppData\Local\Karman\karman.exe", r"C:\Users\Пётр\AppData\Roaming\io.github.primusfixer.karman", "win32");
        assert_eq!(m["name"], "karman");
        assert_eq!(m["server"]["entry_point"], "karman.exe");
        assert_eq!(m["server"]["mcp_config"]["args"][0], "--mcp");
        assert_eq!(m["tools"].as_array().unwrap().len(), tools::list().len());
        let data = bundle(&m, b"\x89PNG\r\n\x1a\nicon").unwrap();
        assert_eq!(&data[..4], b"PK\x03\x04");
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut z.by_name("manifest.json").unwrap(), &mut text).unwrap();
        let back: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back["server"]["mcp_config"]["env"]["KARMAN_DIR"], r"C:\Users\Пётр\AppData\Roaming\io.github.primusfixer.karman");
        assert_eq!(z.by_name("icon.png").unwrap().size(), 12);
    }

    /// Аннотации инструментов — как были до общего модуля: удаляет только delete_operation.
    #[test]
    fn annotations_are_the_same_as_before_kit() {
        let list = tool_list();
        for t in list["tools"].as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            let write = tools::is_write(name);
            assert_eq!(t["annotations"]["readOnlyHint"], !write, "{name}");
            assert_eq!(t["annotations"]["destructiveHint"], name == "delete_operation", "{name}");
            assert_eq!(t["annotations"]["idempotentHint"], !write || name == "set_work_day", "{name}");
        }
    }
}
