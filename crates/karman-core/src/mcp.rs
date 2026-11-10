//! Протокол MCP (Model Context Protocol) для коннектора: JSON-RPC 2.0 построчно через stdio.
//!
//! Здесь только разбор и ответы: `initialize`, `ping`, `tools/list`, `tools/call`.
//! Сам вызов инструмента — снаружи: посредник `karman --mcp` передаёт его запущенному «Карману».

use crate::tools;
use serde_json::{json, Value};

/// Версии протокола, которые понимает «Карман», — новая первой.
pub const VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Подсказка для Claude: что это за данные и как с ними обращаться.
pub const INSTRUCTIONS: &str = "«Карман» — личные финансы и командировки пользователя. Суммы — в рублях, даты — ГГГГ-ММ-ДД. \
Начинайте со сводки (summary). Деньги поездки (подотчётные, аванс, суточные, расходы для отчёта 1С) и личные деньги считаются отдельно. \
Перед записью уточняйте у человека то, чего не знаете: поездка это или личное, категория, сумма, дата. \
Каждую запись человек видит в «Кармане» и может отменить кнопкой «Вернуть». \
Наряд-допуск со снимка или PDF: прочитайте его сами и передайте прочитанное в order_days как есть — время дней по правилам таблицы переработок посчитает «Карман». \
Покажите человеку дни таблицей и записывайте через save_order только после его согласия; по одному дню через set_work_day наряд не вписывайте. \
Снимки наряда «Карман» из разговора не получает — напомните добавить их в «Кармане». \
Если «Карман» заблокирован или запись выключена, скажите человеку, что нужно сделать в «Кармане».";

fn ok(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn fail(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// Описание инструментов для `tools/list`.
/// Доказательство ключа связи посредника (`karman --mcp`) и запущенного «Кармана»: MAC от одноразового
/// числа другой стороны. Сам ключ по сети не ходит, и программа, занявшая порт, ничего не узнает.
pub fn link_proof(token: &str, role: &str, nonce: &str) -> String {
    use blake2::digest::consts::U32;
    use blake2::digest::Mac;
    // Ключ BLAKE2b — не длиннее 64 байт (ключ связи — 64 шестнадцатеричных знака).
    if token.is_empty() || token.len() > 64 {
        return String::new();
    }
    let Ok(mut mac) = blake2::Blake2bMac::<U32>::new_with_salt_and_personal(token.as_bytes(), &[], b"karman-link") else { return String::new() };
    mac.update(role.as_bytes());
    mac.update(b"|");
    mac.update(nonce.as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn tool_list() -> Value {
    let tools: Vec<Value> = tools::list()
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": t.input_schema,
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": !t.write,
                    "destructiveHint": t.name == "delete_operation",
                    "idempotentHint": !t.write || t.name == "set_work_day",
                    "openWorldHint": false
                }
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// Ответ на одну строку запроса; `None` — для уведомлений, на них не отвечают.
/// `call(имя, аргументы)` выполняет инструмент: `Ok` — данные, `Err` — понятная человеку ошибка.
pub fn handle(line: &str, version: &str, call: &mut dyn FnMut(&str, &Value) -> Result<Value, String>) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(fail(&Value::Null, -32700, "Parse error")),
    };
    if !msg.is_object() {
        return Some(fail(&Value::Null, -32600, "Invalid Request"));
    }
    let method = msg.get("method").and_then(|m| m.as_str());
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    // Ответ клиента на наш запрос (мы их не шлём) или уведомление — молча.
    let (Some(method), Some(id)) = (method, id) else { return None };
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let version_used = if VERSIONS.contains(&asked) { asked } else { VERSIONS[0] };
            ok(
                &id,
                json!({
                    "protocolVersion": version_used,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "karman", "title": "Карман", "version": version },
                    "instructions": INSTRUCTIONS
                }),
            )
        }
        "ping" => ok(&id, json!({})),
        "tools/list" => ok(&id, tool_list()),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if tools::find(name).is_none() {
                return Some(fail(&id, -32602, &format!("Unknown tool: {name}")));
            }
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(a) if a.is_object() => a.clone(),
                Some(_) => return Some(fail(&id, -32602, "arguments must be an object")),
            };
            match call(name, &args) {
                Ok(v) => ok(&id, json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }], "isError": false })),
                Err(e) => ok(&id, json!({ "content": [{ "type": "text", "text": e }], "isError": true })),
            }
        }
        "resources/list" => ok(&id, json!({ "resources": [] })),
        "resources/templates/list" => ok(&id, json!({ "resourceTemplates": [] })),
        "prompts/list" => ok(&id, json!({ "prompts": [] })),
        _ => fail(&id, -32601, &format!("Method not found: {method}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
