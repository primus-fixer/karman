//! Инструменты для Claude: что можно узнать о деньгах и поездках и что записать.
//!
//! Один набор для коннектора (приложение Claude на компьютере) и, позже, для ИИ в самом «Кармане».
//! Чтение — `read`, запись — `plan`: она только готовит события журнала и обратные к ним
//! (для кнопки «Вернуть»), а записывает их приложение. Суммы — в рублях (числом), даты — ГГГГ-ММ-ДД.

use crate::book::{self, Snapshot};
use crate::date::Date;
use crate::db::Store;
use crate::export::cat_name;
use crate::money::Kop;
use crate::overtime::{self as ot, Rules, Sheet, WorkDay, WorkOrder};
use crate::sync::EventBody;
use crate::trip::{DocKind, Op, OpKind, Trip, TripStatus, ARTICLES};
use serde_json::{json, Map, Value};

/// Описание инструмента — как его видит Claude.
#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    /// Меняет данные: работает, только если запись разрешена в настройках «Кармана».
    pub write: bool,
}

/// Подготовленная запись: события журнала, обратные к ним и что сказать человеку.
#[derive(Clone, Debug)]
pub struct Change {
    pub bodies: Vec<EventBody>,
    pub undo: Vec<EventBody>,
    /// «Записано: Такси, 640 ₽, поездка 588».
    pub text: String,
    pub result: Value,
}

pub type ToolResult<T> = std::result::Result<T, String>;

/// Категории расходов и доходов — как в окне ввода.
pub const TRIP_OUT: [&str; 6] = ["tickets", "lodging", "taxi", "car", "food", "tools"];
pub const TRIP_IN: [&str; 2] = ["advance", "reimb"];
pub const OWN_OUT: [&str; 7] = ["groceries", "cafe", "transport", "home", "telecom", "gifts", "other"];
pub const OWN_IN: [&str; 3] = ["salary", "interest", "otherIn"];

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// Все инструменты по порядку: сначала чтение, потом запись.
pub fn list() -> Vec<ToolInfo> {
    let date = json!({ "type": "string", "description": "Дата ГГГГ-ММ-ДД" });
    vec![
        ToolInfo {
            name: "summary",
            title: "Сводка",
            description: "Сводка на сегодня: «Мои деньги», всего на счетах, подотчётные деньги компании, что вернуть компании и что она должна, личные долги, отложенное, ближайшие платежи, идущие поездки и бюджет месяца. С этого стоит начинать.",
            input_schema: obj(json!({}), &[]),
            write: false,
        },
        ToolInfo {
            name: "accounts",
            title: "Счета",
            description: "Счета с остатками: в валюте счёта и в рублях. Коды счетов нужны для записи операций.",
            input_schema: obj(json!({}), &[]),
            write: false,
        },
        ToolInfo {
            name: "operations",
            title: "Операции",
            description: "Операции за период, новые сверху, и итоги по найденному. По умолчанию — последние 30 дней.",
            input_schema: obj(
                json!({
                    "from": { "type": "string", "description": "С какой даты, ГГГГ-ММ-ДД" },
                    "to": { "type": "string", "description": "По какую дату, ГГГГ-ММ-ДД" },
                    "scope": { "type": "string", "enum": ["all", "personal", "family", "trip", "income", "transfer", "debt"], "description": "all — все; personal — личные расходы; family — семейные; trip — деньги поездок (рабочие расходы, питание, аванс, возмещение, возврат); income — личные доходы; transfer — переводы между своими счетами; debt — платежи по долгам" },
                    "trip": { "type": "string", "description": "Номер поездки" },
                    "category": { "type": "string", "description": "Код категории (см. categories)" },
                    "text": { "type": "string", "description": "Искать в названии и комментарии" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Сколько показать, по умолчанию 100" }
                }),
                &[],
            ),
            write: false,
        },
        ToolInfo {
            name: "trips",
            title: "Поездки",
            description: "Командировки: даты, статус, суточные, расходы с документами и без, сколько вернуть компании или сколько она должна, наряды и переработки.",
            input_schema: obj(json!({ "status": { "type": "string", "enum": ["open", "closed", "all"], "description": "open — не закрытые, closed — закрытые; по умолчанию все" } }), &[]),
            write: false,
        },
        ToolInfo {
            name: "trip",
            title: "Поездка",
            description: "Одна командировка подробно: заявка, расчёт с компанией, авансовый отчёт для 1С по статьям, операции, документы, наряды и переработки.",
            input_schema: obj(json!({ "id": { "type": "string", "description": "Номер поездки" } }), &["id"]),
            write: false,
        },
        ToolInfo {
            name: "overtime",
            title: "Переработки",
            description: "Таблица переработок по дням — по поездке или общая за месяц: время работы, часы с коэффициентами 1,5 и 2, итоги, наряды и сумма к оплате (она только для экрана, в отчёт не входит).",
            input_schema: obj(
                json!({
                    "trip": { "type": "string", "description": "Номер поездки" },
                    "month": { "type": "string", "description": "Месяц ГГГГ-ММ — общая таблица из всех поездок месяца" }
                }),
                &[],
            ),
            write: false,
        },
        ToolInfo {
            name: "budget",
            title: "Бюджет",
            description: "Бюджет текущего месяца, личный и семейный: лимиты и траты по категориям, сколько дней осталось.",
            input_schema: obj(json!({}), &[]),
            write: false,
        },
        ToolInfo {
            name: "stats",
            title: "Статистика",
            description: "Статистика за месяц, квартал или год: расходы по категориям в сравнении с прошлым таким же периодом, доходы и расходы по месяцам, командировки за год.",
            input_schema: obj(
                json!({
                    "span": { "type": "string", "enum": ["month", "quarter", "year"], "description": "Период, по умолчанию месяц" },
                    "date": { "type": "string", "description": "Любая дата внутри периода, ГГГГ-ММ-ДД; по умолчанию сегодня" }
                }),
                &[],
            ),
            write: false,
        },
        ToolInfo {
            name: "debts",
            title: "Долги и платежи",
            description: "Личные долги («я должен» и «мне должны»), кредиты, регулярные платежи и ближайшие сроки.",
            input_schema: obj(json!({}), &[]),
            write: false,
        },
        ToolInfo {
            name: "categories",
            title: "Категории и счета",
            description: "Коды категорий для каждого вида записи, счета и идущие поездки — всё, что нужно для записи операции.",
            input_schema: obj(json!({}), &[]),
            write: false,
        },
        ToolInfo {
            name: "add_operation",
            title: "Записать операцию",
            description: "Записать расход или доход. В поездке: рабочий расход (с документом идёт в отчёт 1С, без документа — из суточных), питание (за счёт суточных), аванс или возмещение. Вне поездки: личный или семейный расход, доход. Если неясно, поездка это или личное и какая категория, — спросите человека.",
            input_schema: obj(
                json!({
                    "type": { "type": "string", "enum": ["expense", "income"], "description": "expense — расход, income — доход или деньги от компании" },
                    "amount": { "type": "number", "exclusiveMinimum": 0, "description": "Сумма в валюте счёта, например 640 или 1250.50" },
                    "category": { "type": "string", "description": "Код категории. Поездка, расход: tickets, lodging, taxi, car, food, tools. Поездка, доход: advance, reimb. Личное, расход: groceries, cafe, transport, home, telecom, gifts, other. Личное, доход: salary, interest, otherIn" },
                    "scope": { "type": "string", "enum": ["trip", "personal", "family", "personal_tagged"], "description": "trip — деньги поездки; personal — личное; family — семейный расход; personal_tagged — личный расход с меткой поездки (в расчёт с компанией не входит)" },
                    "trip": { "type": "string", "description": "Номер поездки — для trip и personal_tagged; если не указан, берётся идущая поездка" },
                    "date": date,
                    "title": { "type": "string", "description": "Название, например «Такси до объекта»; по умолчанию — название категории" },
                    "account": { "type": "string", "description": "Код счёта (см. accounts); по умолчанию рабочая карта для поездки и личная — для остального" },
                    "has_document": { "type": "boolean", "description": "Только для рабочего расхода поездки: есть чек или другой документ — тогда расход идёт в отчёт 1С" },
                    "note": { "type": "string", "description": "Комментарий" }
                }),
                &["type", "amount", "category", "scope"],
            ),
            write: true,
        },
        ToolInfo {
            name: "update_operation",
            title: "Изменить операцию",
            description: "Поправить записанную операцию: сумму, дату, категорию, название, комментарий, отметку о документе. Код операции — из operations.",
            input_schema: obj(
                json!({
                    "id": { "type": "string", "description": "Код операции" },
                    "amount": { "type": "number", "exclusiveMinimum": 0 },
                    "date": date,
                    "category": { "type": "string" },
                    "title": { "type": "string" },
                    "note": { "type": "string" },
                    "has_document": { "type": "boolean" }
                }),
                &["id"],
            ),
            write: true,
        },
        ToolInfo {
            name: "delete_operation",
            title: "Удалить операцию",
            description: "Удалить операцию. В «Кармане» её можно вернуть.",
            input_schema: obj(json!({ "id": { "type": "string", "description": "Код операции" } }), &["id"]),
            write: true,
        },
        ToolInfo {
            name: "set_work_day",
            title: "Рабочий день",
            description: "Время работы за день поездки — для отчёта по переработкам. Время ЧЧ:ММ; работали за полночь — окончание как есть, например 2:00. Без времени и комментария день очищается.",
            input_schema: obj(
                json!({
                    "trip": { "type": "string", "description": "Номер поездки" },
                    "date": date,
                    "start": { "type": "string", "description": "Начало работы, например 8:00" },
                    "end": { "type": "string", "description": "Окончание работы, например 23:50" },
                    "comment": { "type": "string", "description": "Где и что делали" }
                }),
                &["trip", "date"],
            ),
            write: true,
        },
        ToolInfo {
            name: "add_work_order",
            title: "Записать наряд",
            description: "Наряд-допуск поездки: номер, с какого и по какое число, объект и работы. Снимки наряда добавляются в «Кармане».",
            input_schema: obj(
                json!({
                    "trip": { "type": "string", "description": "Номер поездки" },
                    "number": { "type": "string", "description": "Номер наряда, например 4/30" },
                    "from": date,
                    "to": date,
                    "object": { "type": "string", "description": "Объект и работы" }
                }),
                &["trip", "from", "to"],
            ),
            write: true,
        },
        ToolInfo {
            name: "create_trip",
            title: "Новая поездка",
            description: "Завести командировку по заявке: номер, город, даты, цель и суммы заявки по статьям в рублях. Аванс потом отмечается в «Кармане».",
            input_schema: obj(
                json!({
                    "id": { "type": "string", "description": "Номер поездки, например 590" },
                    "city": { "type": "string" },
                    "start": date,
                    "end": date,
                    "goal": { "type": "string", "description": "Описание цели, как в заявке" },
                    "request_no": { "type": "string", "description": "Номер заявки в 1С" },
                    "tickets": { "type": "number", "minimum": 0, "description": "Заявка: билеты" },
                    "hospitality": { "type": "number", "minimum": 0, "description": "Заявка: представительские" },
                    "lodging": { "type": "number", "minimum": 0, "description": "Заявка: проживание" },
                    "per_diem": { "type": "number", "minimum": 0, "description": "Заявка: суточные" },
                    "transport": { "type": "number", "minimum": 0, "description": "Заявка: транспорт, бензин" }
                }),
                &["id", "city", "start", "end"],
            ),
            write: true,
        },
        ToolInfo {
            name: "overtime_report",
            title: "Отчёт по переработкам",
            description: "Собрать отчёт по переработкам в папку «Загрузки» на компьютере: таблица Excel и папка «Наряды» с PDF. По поездке или общий за месяц.",
            input_schema: obj(
                json!({
                    "trip": { "type": "string", "description": "Номер поездки" },
                    "month": { "type": "string", "description": "Месяц ГГГГ-ММ" }
                }),
                &[],
            ),
            write: true,
        },
    ]
}

pub fn find(name: &str) -> Option<ToolInfo> {
    list().into_iter().find(|t| t.name == name)
}

pub fn is_write(name: &str) -> bool {
    find(name).is_some_and(|t| t.write)
}

// ——— Разбор аргументов ———

fn arg_str(args: &Value, key: &str) -> Option<String> {
    match args.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn arg_date(args: &Value, key: &str) -> ToolResult<Option<Date>> {
    match arg_str(args, key) {
        None => Ok(None),
        Some(s) => Date::parse(&s).map(Some).ok_or_else(|| format!("{key}: дата в виде ГГГГ-ММ-ДД, а пришло «{s}»")),
    }
}

/// Сумма в копейках: число или строка «1 250,50».
fn arg_kop(args: &Value, key: &str) -> ToolResult<Option<Kop>> {
    let v = match args.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(n)) => n.as_f64().ok_or_else(|| format!("{key}: не число"))?,
        Some(Value::String(s)) => {
            let t: String = s.chars().filter(|c| !c.is_whitespace() && *c != '\u{a0}').collect::<String>().replace(',', ".");
            t.parse::<f64>().map_err(|_| format!("{key}: сумма числом, а пришло «{s}»"))?
        }
        Some(_) => return Err(format!("{key}: сумма числом")),
    };
    if !v.is_finite() || v < 0.0 || v > 1e11 {
        return Err(format!("{key}: неверная сумма"));
    }
    Ok(Some((v * 100.0).round() as Kop))
}

fn arg_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}

fn rub(k: Kop) -> Value {
    json!((k as f64) / 100.0)
}

/// «1 250,50 ₽» — для сообщений человеку.
fn rub_text(k: Kop) -> String {
    let a = k.abs();
    let int = (a / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push('\u{a0}');
        }
        grouped.push(c);
    }
    let frac = a % 100;
    let sign = if k < 0 { "−" } else { "" };
    if frac == 0 {
        format!("{sign}{grouped}\u{a0}₽")
    } else {
        format!("{sign}{grouped},{frac:02}\u{a0}₽")
    }
}

fn hours(min: u32) -> Value {
    json!((min as f64) / 60.0)
}

// ——— Чтение ———

/// Выполнить инструмент чтения.
pub fn read(store: &Store, today: Date, name: &str, args: &Value) -> ToolResult<Value> {
    let e = |x: crate::error::Error| x.to_string();
    match name {
        "summary" => Ok(summary(&book::snapshot(store, today).map_err(e)?)),
        "accounts" => Ok(accounts(&book::snapshot(store, today).map_err(e)?)),
        "operations" => operations(&book::snapshot(store, today).map_err(e)?, args),
        "trips" => Ok(trips(&book::snapshot(store, today).map_err(e)?, args)),
        "trip" => trip(&book::snapshot(store, today).map_err(e)?, args),
        "overtime" => overtime(store, &book::snapshot(store, today).map_err(e)?, args),
        "budget" => Ok(budget(&book::snapshot(store, today).map_err(e)?)),
        "stats" => stats(store, today, args),
        "debts" => Ok(debts(&book::snapshot(store, today).map_err(e)?)),
        "categories" => Ok(categories(&book::snapshot(store, today).map_err(e)?)),
        _ if is_write(name) => Err(format!("{name} меняет данные — это запись, а не чтение")),
        _ => Err(format!("нет инструмента «{name}»")),
    }
}

fn account_name(s: &Snapshot, id: &str) -> String {
    s.accounts.iter().find(|a| a.account.id == id).map(|a| a.account.name.clone()).unwrap_or_else(|| "счёт удалён".into())
}

fn kind_title(o: &Op) -> &'static str {
    match o.kind {
        OpKind::Work => "Рабочий расход в поездке",
        OpKind::Food => "Питание в поездке",
        OpKind::Personal if o.family => "Семейный расход",
        OpKind::Personal if o.trip.is_some() => "Личный расход с меткой поездки",
        OpKind::Personal => "Личный расход",
        OpKind::Income => "Доход",
        OpKind::Advance => "Аванс от компании",
        OpKind::Reimbursement => "Возмещение от компании",
        OpKind::Return => "Возврат компании",
        OpKind::Debt if o.inflow => "Возврат долга мне",
        OpKind::Debt => "Платёж по долгу",
        OpKind::Transfer => "Перевод между своими счетами",
    }
}

fn kind_code(k: OpKind) -> &'static str {
    match k {
        OpKind::Work => "work",
        OpKind::Food => "food",
        OpKind::Personal => "personal",
        OpKind::Income => "income",
        OpKind::Advance => "advance",
        OpKind::Reimbursement => "reimbursement",
        OpKind::Return => "return",
        OpKind::Debt => "debt",
        OpKind::Transfer => "transfer",
    }
}

fn doc_code(d: Option<DocKind>) -> Value {
    match d {
        Some(DocKind::Photo) => json!("photo"),
        Some(DocKind::Qr) => json!("qr"),
        Some(DocKind::File) => json!("file"),
        None => Value::Null,
    }
}

fn op_view(s: &Snapshot, o: &Op) -> Value {
    let mut v = json!({
        "id": o.id,
        "date": o.date,
        "type": kind_title(o),
        "kind": kind_code(o.kind),
        "category": o.cat,
        "category_name": cat_name(&o.cat),
        "title": o.title,
        "amount": rub(o.amount),
        "account": account_name(s, &o.account),
        "trip": o.trip,
    });
    let m = v.as_object_mut().expect("объект");
    if let (Some(orig), Some(cur)) = (o.orig, o.cur.as_ref()) {
        m.insert("amount_in_currency".into(), json!({ "amount": (orig as f64) / 100.0, "currency": cur }));
    }
    if o.kind == OpKind::Work {
        m.insert("document".into(), doc_code(o.doc));
        m.insert("sent_to_office".into(), json!(o.sent));
    }
    if let Some(to) = &o.to {
        m.insert("to_account".into(), json!(account_name(s, to)));
    }
    if !o.note.is_empty() {
        m.insert("note".into(), json!(o.note));
    }
    if !o.att.is_empty() {
        m.insert("attachments".into(), json!(o.att.len()));
    }
    v
}

fn summary(s: &Snapshot) -> Value {
    let open: Vec<Value> = s
        .trips
        .iter()
        .filter(|t| t.trip.status != TripStatus::Closed)
        .map(|t| json!({ "id": t.trip.id, "city": t.trip.city, "start": t.trip.start, "end": t.trip.end, "status": t.trip.status.title() }))
        .collect();
    let upcoming: Vec<Value> = s
        .upcoming
        .iter()
        .take(10)
        .map(|u| json!({ "title": u.title, "date": u.date, "amount": rub(u.amount), "days_left": u.days, "kind": u.kind, "auto": u.auto }))
        .collect();
    json!({
        "today": s.today,
        "mine": rub(s.mine),
        "mine_explained": "«Мои деньги»: на счетах минус подотчётные деньги компании и то, что нужно вернуть компании; личные долги не вычитаются".to_string()
            + if s.saved_in_mine { ", отложенное входит" } else { ", отложенное — отдельно" },
        "on_accounts": rub(s.total),
        "company_money_in_trips": rub(s.held),
        "to_return_to_company": rub(s.due),
        "of_it_debt_to_company": rub(s.debt_due),
        "company_owes": rub(s.owed),
        "i_owe": rub(s.i_owe),
        "owed_to_me": rub(s.owed_to_me),
        "saved": rub(s.saved),
        "open_trips": open,
        "upcoming_payments": upcoming,
        "budget": s.budgets.iter().map(|b| json!({ "id": b.id, "limit": b.total.map(|t| (t as f64) / 100.0), "spent": rub(b.spent), "days_left": b.days_left })).collect::<Vec<_>>(),
    })
}

fn accounts(s: &Snapshot) -> Value {
    json!(s
        .accounts
        .iter()
        .map(|a| json!({
            "id": a.account.id,
            "name": a.account.name,
            "kind": a.account.kind,
            "currency": a.account.currency,
            "balance": rub(a.balance),
            "balance_rub": rub(a.balance_rub),
            "archived": a.account.archived,
        }))
        .collect::<Vec<_>>())
}

fn scope_of(o: &Op, scope: &str) -> bool {
    match scope {
        "personal" => o.kind == OpKind::Personal && !o.family,
        "family" => o.kind == OpKind::Personal && o.family,
        "trip" => matches!(o.kind, OpKind::Work | OpKind::Food | OpKind::Advance | OpKind::Reimbursement | OpKind::Return),
        "income" => o.kind == OpKind::Income,
        "transfer" => o.kind == OpKind::Transfer,
        "debt" => o.kind == OpKind::Debt,
        _ => true,
    }
}

fn operations(s: &Snapshot, args: &Value) -> ToolResult<Value> {
    let to = arg_date(args, "to")?.unwrap_or(s.today);
    let from = arg_date(args, "from")?.unwrap_or_else(|| to.add_days(-30));
    if from > to {
        return Err("«from» позже «to»".into());
    }
    let scope = arg_str(args, "scope").unwrap_or_else(|| "all".into());
    let trip = arg_str(args, "trip");
    let cat = arg_str(args, "category");
    let text = arg_str(args, "text").map(|t| t.to_lowercase());
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100).clamp(1, 1000) as usize;
    let mut found: Vec<&Op> = s
        .ops
        .iter()
        .filter(|o| o.date >= from && o.date <= to)
        .filter(|o| scope_of(o, &scope))
        .filter(|o| trip.as_ref().is_none_or(|t| o.trip.as_deref() == Some(t.as_str())))
        .filter(|o| cat.as_ref().is_none_or(|c| &o.cat == c))
        .filter(|o| text.as_ref().is_none_or(|t| o.title.to_lowercase().contains(t) || o.note.to_lowercase().contains(t)))
        .collect();
    found.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.id.cmp(&b.id)));
    let spent: Kop = found.iter().filter(|o| matches!(o.kind, OpKind::Work | OpKind::Food | OpKind::Personal)).map(|o| o.amount).sum();
    let income: Kop = found.iter().filter(|o| o.kind == OpKind::Income).map(|o| o.amount).sum();
    Ok(json!({
        "from": from,
        "to": to,
        "found": found.len(),
        "shown": found.len().min(limit),
        "spent": rub(spent),
        "income": rub(income),
        "operations": found.iter().take(limit).map(|o| op_view(s, o)).collect::<Vec<_>>(),
    }))
}

fn balance_note(b: Kop, status: TripStatus) -> &'static str {
    if b < 0 {
        "компания должна"
    } else if b == 0 {
        "в расчёте"
    } else if (status as u8) < TripStatus::AwaitingReport as u8 {
        "подотчётные деньги компании"
    } else {
        "вернуть компании"
    }
}

fn trips(s: &Snapshot, args: &Value) -> Value {
    let status = arg_str(args, "status").unwrap_or_else(|| "all".into());
    json!(s
        .trips
        .iter()
        .filter(|t| match status.as_str() {
            "open" => t.trip.status != TripStatus::Closed,
            "closed" => t.trip.status == TripStatus::Closed,
            _ => true,
        })
        .map(|t| {
            let c = &t.s;
            json!({
                "id": t.trip.id,
                "city": t.trip.city,
                "start": t.trip.start,
                "end": t.trip.end,
                "status": t.trip.status.title(),
                "days": c.days,
                "per_diem_rate": rub(t.rate),
                "per_diem": rub(c.per_diem),
                "advance": rub(c.issued),
                "with_documents": rub(c.confirmed),
                "without_documents": rub(c.no_doc),
                "food": rub(c.food),
                "balance": rub(c.balance.abs()),
                "balance_means": balance_note(c.balance, t.trip.status),
                "debt_to_company": rub(c.debt_due),
                "work_orders": t.ot.orders.len(),
                "overtime_hours": { "x1_5": hours(t.ot.sheet.x15), "x2": hours(t.ot.sheet.double()) },
            })
        })
        .collect::<Vec<_>>())
}

fn find_trip<'a>(s: &'a Snapshot, id: &str) -> ToolResult<&'a book::TripView> {
    s.trips.iter().find(|t| t.trip.id == id).ok_or_else(|| format!("поездки {id} нет"))
}

fn sheet_view(sheet: &Sheet) -> Value {
    json!({
        "days": sheet.rows.iter().map(|r| {
            let mut v = json!({ "date": r.date, "day_off": r.off });
            let m = v.as_object_mut().expect("объект");
            if !r.start.is_empty() {
                m.insert("start".into(), json!(r.start));
                m.insert("end".into(), json!(r.end));
            }
            if !r.comment.is_empty() {
                m.insert("comment".into(), json!(r.comment));
            }
            if let Some(t) = &r.trip {
                m.insert("trip".into(), json!(t));
            }
            if let Some(h) = r.hours {
                m.insert("hours".into(), json!({ "x1_5": hours(h.x15), "x2_weekday": hours(h.x2), "x2_day_off": hours(h.off) }));
            }
            v
        }).collect::<Vec<_>>(),
        "totals": { "x1_5": hours(sheet.x15), "x2_weekday": hours(sheet.x2), "x2_day_off": hours(sheet.off), "x2": hours(sheet.double()) },
        "summary": ot::summary(sheet),
    })
}

fn pay_view(pay: &[ot::Pay]) -> Value {
    json!(pay
        .iter()
        .map(|p| json!({
            "month": p.month,
            "norm_hours": hours(p.norm),
            "norm_source": if p.custom { "своя" } else if p.exact { "производственный календарь" } else { "оценка" },
            "hour_rate": rub(p.rate),
            "x1_5_hours": hours(p.x15),
            "x2_hours": hours(p.x2),
            "sum_before_tax": rub(p.sum),
        }))
        .collect::<Vec<_>>())
}

fn orders_view(orders: &[WorkOrder]) -> Value {
    json!(orders
        .iter()
        .map(|o| json!({
            "id": o.id,
            "trip": o.trip,
            "number": o.number,
            "from": o.from,
            "to": o.to,
            "object": o.object,
            "photos": o.att.len(),
            "file_name": ot::order_file_name(o) + ".pdf",
        }))
        .collect::<Vec<_>>())
}

fn trip(s: &Snapshot, args: &Value) -> ToolResult<Value> {
    let id = arg_str(args, "id").ok_or("нужен номер поездки")?;
    let t = find_trip(s, &id)?;
    let tr = &t.trip;
    let c = &t.s;
    let ops: Vec<&Op> = {
        let mut v: Vec<&Op> = s.ops.iter().filter(|o| o.trip.as_deref() == Some(tr.id.as_str())).collect();
        v.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
        v
    };
    let work: Vec<&&Op> = ops.iter().filter(|o| o.kind == OpKind::Work).collect();
    Ok(json!({
        "trip": {
            "id": tr.id,
            "city": tr.city,
            "start": tr.start,
            "end": tr.end,
            "plan_start": tr.plan_start,
            "plan_end": tr.plan_end,
            "status": tr.status.title(),
            "request_no": tr.req_no,
            "request_date": tr.req_date,
            "goal": tr.goal,
            "report_deadline": tr.deadline,
            "plan": ARTICLES.iter().zip(tr.plan.iter()).map(|((code, name), k)| json!({ "code": code, "name": name, "amount": rub(*k) })).collect::<Vec<_>>(),
        },
        "per_diem_rate": rub(t.rate),
        "settlement": {
            "days": c.days,
            "days_passed": c.elapsed,
            "finished": c.finished,
            "per_diem": rub(c.per_diem),
            "per_diem_full": rub(c.per_diem_full),
            "advance": rub(c.issued),
            "reimbursed": rub(c.reimbursed),
            "returned": rub(c.returned),
            "with_documents": rub(c.confirmed),
            "without_documents": rub(c.no_doc),
            "without_documents_count": c.no_doc_count,
            "food": rub(c.food),
            "personal_tagged": rub(c.tagged),
            "balance": rub(c.balance.abs()),
            "balance_means": balance_note(c.balance, tr.status),
            "per_diem_left": rub(c.leftover),
            "debt_to_company": rub(c.debt_due),
            "lost": rub(c.lost),
        },
        "report_1c": t.rows.iter().map(|r| json!({ "code": r.code, "name": r.name, "plan": rub(r.plan), "fact": rub(r.fact), "over_plan": rub(r.over) })).collect::<Vec<_>>(),
        "over_plan_total": rub(t.over_total),
        "over_plan_justification": tr.just.clone().unwrap_or_else(|| t.auto_just.clone()),
        "documents": {
            "work_expenses": work.len(),
            "with_document": work.iter().filter(|o| o.doc.is_some()).count(),
            "sent_to_office": work.iter().filter(|o| o.doc.is_some() && o.sent).count(),
        },
        "operations": ops.iter().map(|o| op_view(s, o)).collect::<Vec<_>>(),
        "overtime": {
            "x1_5_hours": hours(t.ot.sheet.x15),
            "x2_hours": hours(t.ot.sheet.double()),
            "summary": t.ot.summary,
            "work_orders": orders_view(&t.ot.orders),
        },
    }))
}

fn month_arg(args: &Value) -> ToolResult<Option<Date>> {
    match arg_str(args, "month") {
        None => Ok(None),
        Some(m) => Date::parse(&format!("{m}-01")).map(Some).ok_or_else(|| format!("month: месяц в виде ГГГГ-ММ, а пришло «{m}»")),
    }
}

fn overtime(store: &Store, s: &Snapshot, args: &Value) -> ToolResult<Value> {
    if let Some(id) = arg_str(args, "trip") {
        let t = find_trip(s, &id)?;
        return Ok(json!({
            "title": format!("Поездка {} {}", t.trip.id, t.trip.city),
            "table": sheet_view(&t.ot.sheet),
            "work_orders": orders_view(&t.ot.orders),
            "pay_on_screen_only": pay_view(&t.ot.pay),
            "rules": rules_text(&t.ot.rules),
        }));
    }
    let m = month_arg(args)?.unwrap_or_else(|| s.today.month_start());
    let e = |x: crate::error::Error| x.to_string();
    let trips = store.trips().map_err(e)?;
    let days = store.work_days().map_err(e)?;
    let rules = Rules::from_settings(&s.settings);
    let sheet = ot::month_sheet(m, &trips, &days, &rules);
    let (ms, me) = (m.month_start(), m.month_end());
    let mut orders: Vec<WorkOrder> = store.orders().map_err(e)?.into_iter().filter(|o| o.from.min(o.to) <= me && o.from.max(o.to) >= ms).collect();
    orders.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.number.cmp(&b.number)));
    Ok(json!({
        "title": format!("Общая таблица за {}", ot::month_title(m)),
        "table": sheet_view(&sheet),
        "work_orders": orders_view(&orders),
        "pay_on_screen_only": pay_view(&ot::pay(&sheet, &s.settings)),
        "rules": rules_text(&rules),
    }))
}

fn rules_text(r: &Rules) -> String {
    let t = |m: u32| ot::time_text(m);
    format!(
        "Будни: с {} до {} — коэффициент 1,5, позже — 2. Выходные и праздники — вся смена с коэффициентом 2, без обеда {} мин, если работа захватила время с 12 до 13. Округление до {} мин. Смена за полночь — днём начала.",
        t(r.day_end),
        t(r.day_end + r.first),
        r.lunch,
        r.step
    )
}

fn budget(s: &Snapshot) -> Value {
    json!(s
        .budgets
        .iter()
        .map(|b| json!({
            "id": b.id,
            "title": if b.id == "family" { "Семейный" } else { "Личный" },
            "start": b.start,
            "end": b.end,
            "limit": b.total.map(|t| (t as f64) / 100.0),
            "spent": rub(b.spent),
            "left": b.total.map(|t| ((t - b.spent) as f64) / 100.0),
            "days_left": b.days_left,
            "categories": b.cats.iter().map(|c| json!({ "category": c.cat, "name": cat_name(&c.cat), "limit": c.limit.map(|l| (l as f64) / 100.0), "spent": rub(c.spent) })).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>())
}

fn stats(store: &Store, today: Date, args: &Value) -> ToolResult<Value> {
    let e = |x: crate::error::Error| x.to_string();
    let span = crate::stats::Span::parse(&arg_str(args, "span").unwrap_or_default());
    let any = arg_date(args, "date")?.unwrap_or(today);
    let settings = store.settings().map_err(e)?;
    let st = crate::stats::stats(&store.ops().map_err(e)?, &store.accounts().map_err(e)?, &store.trips().map_err(e)?, book::settings_rate(&settings), span, any, today);
    Ok(json!({
        "from": st.from,
        "to_exclusive": st.to,
        "compared_with": { "from": st.prev_from, "to_exclusive": st.prev_to },
        "period_in_progress": st.partial,
        "spent": rub(st.spent),
        "spent_before": rub(st.prev_spent),
        "income": rub(st.income),
        "categories": st.cats.iter().map(|c| json!({ "category": c.cat, "name": cat_name(&c.cat), "amount": rub(c.amount), "before": rub(c.prev) })).collect::<Vec<_>>(),
        "months": st.months.iter().map(|m| json!({ "month": m.month, "income": rub(m.income), "spent": rub(m.spent), "saved": rub(m.saved) })).collect::<Vec<_>>(),
        "trips_year": {
            "year": st.trips.year,
            "count": st.trips.count,
            "days": st.trips.days,
            "per_diem": rub(st.trips.per_diem),
            "per_diem_left": rub(st.trips.left),
            "debt_to_company": rub(st.trips.debt),
            "with_documents": rub(st.trips.confirmed),
        },
    }))
}

fn debts(s: &Snapshot) -> Value {
    json!({
        "debts": s.debts.iter().map(|d| json!({
            "id": d.debt.id,
            "who": d.debt.who,
            "direction": if d.debt.dir == crate::finance::DebtDir::IOwe { "я должен" } else { "мне должны" },
            "amount": rub(d.debt.amount),
            "repaid": rub(d.repaid),
            "left": rub(d.left),
            "since": d.debt.date,
            "due": d.debt.due,
            "overdue": d.overdue,
            "closed": d.debt.closed,
            "next_payment": d.next,
            "note": d.debt.note,
        })).collect::<Vec<_>>(),
        "regular_payments": s.plans.iter().map(|p| json!({
            "id": p.plan.id,
            "title": p.plan.title,
            "amount_rub": rub(p.amount_rub),
            "account": account_name(s, &p.plan.account),
            "every": p.plan.every,
            "next": p.next,
            "overdue": p.overdue,
            "auto": p.plan.auto,
            "paused": p.plan.paused,
        })).collect::<Vec<_>>(),
        "upcoming": s.upcoming.iter().map(|u| json!({ "title": u.title, "date": u.date, "amount": rub(u.amount), "days_left": u.days, "kind": u.kind, "auto": u.auto })).collect::<Vec<_>>(),
    })
}

fn cats_view(list: &[&str]) -> Value {
    json!(list.iter().map(|c| json!({ "id": c, "name": cat_name(c) })).collect::<Vec<_>>())
}

fn categories(s: &Snapshot) -> Value {
    json!({
        "trip_expense": cats_view(&TRIP_OUT),
        "trip_income": cats_view(&TRIP_IN),
        "personal_expense": cats_view(&OWN_OUT),
        "personal_income": cats_view(&OWN_IN),
        "accounts": s.accounts.iter().filter(|a| !a.account.archived).map(|a| json!({ "id": a.account.id, "name": a.account.name, "kind": a.account.kind, "currency": a.account.currency })).collect::<Vec<_>>(),
        "open_trips": s.trips.iter().filter(|t| t.trip.status != TripStatus::Closed).map(|t| json!({ "id": t.trip.id, "city": t.trip.city, "start": t.trip.start, "end": t.trip.end, "status": t.trip.status.title() })).collect::<Vec<_>>(),
        "notes": "Рабочий расход поездки с документом идёт в авансовый отчёт 1С; без документа — вычитается из суточных, сверх них становится долгом перед компанией. Питание в поездке — за счёт суточных. Материалы и инструменты — статья 03.01.04.05.",
    })
}

// ——— Запись ———

/// Идущая поездка — как в окне ввода: в пути и в датах, потом просто в пути, потом с авансом и в датах.
pub fn active_trip(trips: &[Trip], today: Date) -> Option<&Trip> {
    let within = |t: &&Trip| t.start <= today && today <= t.end;
    trips
        .iter()
        .filter(|t| t.status == TripStatus::InTrip)
        .find(within)
        .or_else(|| trips.iter().find(|t| t.status == TripStatus::InTrip))
        .or_else(|| trips.iter().filter(|t| t.status == TripStatus::AdvanceReceived).find(within))
}

/// Подготовить запись (кроме отчёта по переработкам — его собирает приложение).
/// `rate` — курс валюты к рублю на дату (для счетов не в рублях).
pub fn plan(store: &Store, today: Date, name: &str, args: &Value, rate: &dyn Fn(&str, Date) -> Option<f64>) -> ToolResult<Change> {
    let e = |x: crate::error::Error| x.to_string();
    match name {
        "add_operation" => add_operation(store, today, args, rate),
        "update_operation" => update_operation(store, args, rate),
        "delete_operation" => {
            let id = arg_str(args, "id").ok_or("нужен код операции")?;
            let op = store.ops().map_err(e)?.into_iter().find(|o| o.id == id).ok_or_else(|| format!("операции {id} нет"))?;
            let text = format!("Удалено: {}, {}", op.title, rub_text(op.amount));
            Ok(Change { bodies: vec![EventBody::OpDelete { id: id.clone() }], undo: vec![EventBody::OpPut { op }], text, result: json!({ "deleted": id }) })
        }
        "set_work_day" => set_work_day(store, args),
        "add_work_order" => add_work_order(store, args),
        "create_trip" => create_trip(store, args),
        "overtime_report" => Err("отчёт по переработкам собирает приложение".into()),
        _ if find(name).is_some() => Err(format!("{name} только читает данные")),
        _ => Err(format!("нет инструмента «{name}»")),
    }
}

/// Сумма в рублях: для счёта в валюте — по курсу на дату.
fn to_rub(amount: Kop, cur: &str, date: Date, rate: &dyn Fn(&str, Date) -> Option<f64>) -> ToolResult<(Kop, Option<Kop>, Option<String>)> {
    if cur == "RUB" {
        return Ok((amount, None, None));
    }
    let r = rate(cur, date).filter(|r| *r > 0.0).ok_or_else(|| format!("нет курса {cur} на {date}: укажите курс в настройках «Кармана»"))?;
    Ok(((amount as f64 * r).round() as Kop, Some(amount), Some(cur.to_string())))
}

fn add_operation(store: &Store, today: Date, args: &Value, rate: &dyn Fn(&str, Date) -> Option<f64>) -> ToolResult<Change> {
    let e = |x: crate::error::Error| x.to_string();
    let income = match arg_str(args, "type").as_deref() {
        Some("expense") => false,
        Some("income") => true,
        _ => return Err("type: expense (расход) или income (доход)".into()),
    };
    let amount = arg_kop(args, "amount")?.filter(|a| *a > 0).ok_or("amount: сумма больше нуля")?;
    let cat = arg_str(args, "category").ok_or("category: код категории (см. categories)")?;
    let scope = arg_str(args, "scope").ok_or("scope: trip, personal, family или personal_tagged")?;
    let date = arg_date(args, "date")?.unwrap_or(today);
    let trips = store.trips().map_err(e)?;
    let in_trip = scope == "trip";
    let tagged = scope == "personal_tagged";
    let family = scope == "family";
    if !matches!(scope.as_str(), "trip" | "personal" | "family" | "personal_tagged") {
        return Err("scope: trip, personal, family или personal_tagged".into());
    }
    if income && (family || tagged) {
        return Err("доход записывается как personal (личный) или trip (деньги от компании по поездке)".into());
    }
    let allowed: &[&str] = match (in_trip, income) {
        (true, false) => &TRIP_OUT,
        (true, true) => &TRIP_IN,
        (false, false) => &OWN_OUT,
        (false, true) => &OWN_IN,
    };
    if !allowed.contains(&cat.as_str()) {
        return Err(format!("категория «{cat}» не подходит; можно: {}", allowed.join(", ")));
    }
    let trip_id = if in_trip || tagged {
        let t = match arg_str(args, "trip") {
            Some(id) => trips.iter().find(|t| t.id == id).ok_or_else(|| format!("поездки {id} нет"))?,
            None => active_trip(&trips, today).ok_or("укажите номер поездки: идущей поездки нет")?,
        };
        Some(t.id.clone())
    } else {
        None
    };
    let accounts = store.accounts().map_err(e)?;
    let acc = match arg_str(args, "account") {
        Some(id) => accounts.iter().find(|a| a.id == id && !a.archived).ok_or_else(|| format!("счёта {id} нет (см. accounts)"))?,
        None => {
            let want = if in_trip { "work" } else { "personal" };
            accounts.iter().filter(|a| !a.archived).find(|a| a.kind == want).or_else(|| accounts.iter().find(|a| !a.archived)).ok_or("нет ни одного счёта")?
        }
    };
    let kind = match (in_trip, income) {
        (true, false) if cat == "food" => OpKind::Food,
        (true, false) => OpKind::Work,
        (true, true) if cat == "advance" => OpKind::Advance,
        (true, true) => OpKind::Reimbursement,
        (false, false) => OpKind::Personal,
        (false, true) => OpKind::Income,
    };
    let doc = (kind == OpKind::Work && arg_bool(args, "has_document") == Some(true)).then_some(DocKind::Photo);
    let (rub_amount, orig, cur) = to_rub(amount, &acc.currency, date, rate)?;
    let title = arg_str(args, "title").unwrap_or_else(|| cat_name(&cat).to_string());
    if title.chars().count() > 200 {
        return Err("слишком длинное название".into());
    }
    let id = book::new_id().map_err(e)?;
    let op = Op {
        id: id.clone(),
        date,
        kind,
        cat: cat.clone(),
        title: title.clone(),
        amount: rub_amount,
        account: acc.id.clone(),
        trip: trip_id.clone(),
        doc,
        sent: false,
        family: family && kind == OpKind::Personal,
        debt: None,
        inflow: false,
        orig,
        cur: cur.clone(),
        note: arg_str(args, "note").unwrap_or_default(),
        att: Vec::new(),
        to: None,
        to_amount: None,
        plan: None,
        plan_date: None,
        goal: None,
        extra: Default::default(),
    };
    let shown = match (&orig, &cur) {
        (Some(o), Some(c)) => format!("{} {c} (≈ {})", (*o as f64) / 100.0, rub_text(rub_amount)),
        _ => rub_text(rub_amount),
    };
    let where_ = match (&trip_id, in_trip, family) {
        (Some(t), true, _) => format!("поездка {t}"),
        (Some(t), false, _) => format!("личное, метка поездки {t}"),
        (None, _, true) => "семья".into(),
        _ => "личное".into(),
    };
    let mut text = format!("Записано: {title}, {shown}, {where_}");
    if kind == OpKind::Work && doc.is_none() {
        text.push_str(", без документа — из суточных");
    }
    let result = json!({ "id": id, "date": date, "type": kind_title(&op), "category": cat, "title": title, "amount_rub": rub(rub_amount), "account": acc.name, "trip": trip_id, "document": doc_code(doc) });
    Ok(Change { bodies: vec![EventBody::OpPut { op }], undo: vec![EventBody::OpDelete { id }], text, result })
}

fn update_operation(store: &Store, args: &Value, rate: &dyn Fn(&str, Date) -> Option<f64>) -> ToolResult<Change> {
    let e = |x: crate::error::Error| x.to_string();
    let id = arg_str(args, "id").ok_or("нужен код операции")?;
    let before = store.ops().map_err(e)?.into_iter().find(|o| o.id == id).ok_or_else(|| format!("операции {id} нет"))?;
    if !matches!(before.kind, OpKind::Work | OpKind::Food | OpKind::Personal | OpKind::Income | OpKind::Advance | OpKind::Reimbursement) {
        return Err(format!("«{}» в коннекторе не правится — только в «Кармане»", kind_title(&before)));
    }
    let mut op = before.clone();
    if let Some(d) = arg_date(args, "date")? {
        op.date = d;
    }
    if let Some(c) = arg_str(args, "category") {
        let allowed: &[&str] = match op.kind {
            OpKind::Work | OpKind::Food => &TRIP_OUT,
            OpKind::Advance | OpKind::Reimbursement => &TRIP_IN,
            OpKind::Personal => &OWN_OUT,
            _ => &OWN_IN,
        };
        if !allowed.contains(&c.as_str()) {
            return Err(format!("категория «{c}» не подходит; можно: {}", allowed.join(", ")));
        }
        // Питание и рабочий расход в поездке — разные виды записи.
        if matches!(op.kind, OpKind::Work | OpKind::Food) {
            op.kind = if c == "food" { OpKind::Food } else { OpKind::Work };
            if op.kind == OpKind::Food {
                op.doc = None;
                op.sent = false;
            }
        }
        if matches!(op.kind, OpKind::Advance | OpKind::Reimbursement) {
            op.kind = if c == "advance" { OpKind::Advance } else { OpKind::Reimbursement };
        }
        op.cat = c;
    }
    if let Some(t) = arg_str(args, "title") {
        op.title = t;
    }
    if let Some(n) = args.get("note").and_then(|v| v.as_str()) {
        op.note = n.trim().to_string();
    }
    if let Some(has) = arg_bool(args, "has_document") {
        if op.kind != OpKind::Work {
            return Err("отметка о документе — только у рабочего расхода поездки".into());
        }
        op.doc = if has { op.doc.or(Some(DocKind::Photo)) } else { None };
        if !has {
            op.sent = false;
        }
    }
    let new_amount = arg_kop(args, "amount")?;
    if new_amount == Some(0) {
        return Err("amount: сумма больше нуля".into());
    }
    if new_amount.is_some() || op.date != before.date {
        let cur = op.cur.clone().unwrap_or_else(|| "RUB".into());
        let in_cur = new_amount.or(op.orig).unwrap_or(op.amount);
        let (r, orig, c) = to_rub(in_cur, &cur, op.date, rate)?;
        // Дата поменялась, а сумма в рублях та же — курс не пересчитываем.
        if new_amount.is_some() || cur != "RUB" {
            op.amount = r;
            op.orig = orig;
            op.cur = c;
        }
    }
    if op == before {
        return Err("ничего не поменялось".into());
    }
    let text = format!("Изменено: {}, {}", op.title, rub_text(op.amount));
    let result = json!({ "id": id, "date": op.date, "category": op.cat, "title": op.title, "amount_rub": rub(op.amount), "document": doc_code(op.doc) });
    Ok(Change { bodies: vec![EventBody::OpPut { op }], undo: vec![EventBody::OpPut { op: before }], text, result })
}

fn trip_exists(store: &Store, id: &str) -> ToolResult<Trip> {
    store.trips().map_err(|x| x.to_string())?.into_iter().find(|t| t.id == id).ok_or_else(|| format!("поездки {id} нет"))
}

fn set_work_day(store: &Store, args: &Value) -> ToolResult<Change> {
    let e = |x: crate::error::Error| x.to_string();
    let trip = arg_str(args, "trip").ok_or("нужен номер поездки")?;
    trip_exists(store, &trip)?;
    let date = arg_date(args, "date")?.ok_or("нужна дата")?;
    let norm = |key: &str| -> ToolResult<String> {
        let s = arg_str(args, key).unwrap_or_default();
        if s.is_empty() {
            return Ok(s);
        }
        let s = s.replace(['.', '-'], ":");
        let s = if s.contains(':') { s } else { format!("{s}:00") };
        ot::parse_time(&s).map(ot::time_text).ok_or_else(|| format!("{key}: время в виде 8:00, а пришло «{s}»"))
    };
    let day = WorkDay { id: ot::day_id(&trip, date), trip: trip.clone(), date, start: norm("start")?, end: norm("end")?, comment: arg_str(args, "comment").unwrap_or_default(), extra: Default::default() };
    ot::check_day(&day).map_err(e)?;
    let before = store.work_days().map_err(e)?.into_iter().find(|d| d.id == day.id);
    let undo = match &before {
        Some(b) => vec![EventBody::DayPut { day: b.clone() }],
        None => vec![EventBody::DayDelete { id: day.id.clone() }],
    };
    let clear = day.start.is_empty() && day.comment.is_empty();
    if clear && before.is_none() {
        return Err("день и так пустой".into());
    }
    let text = if clear {
        format!("Очищен день {}: поездка {trip}", date.dotted())
    } else if day.start.is_empty() {
        format!("День {}: {}", date.dotted(), day.comment)
    } else {
        format!("День {}: {}–{}, поездка {trip}", date.dotted(), day.start, day.end)
    };
    let result = json!({ "trip": trip, "date": date, "start": day.start, "end": day.end, "comment": day.comment, "cleared": clear });
    let body = if clear { EventBody::DayDelete { id: day.id.clone() } } else { EventBody::DayPut { day } };
    Ok(Change { bodies: vec![body], undo, text, result })
}

fn add_work_order(store: &Store, args: &Value) -> ToolResult<Change> {
    let e = |x: crate::error::Error| x.to_string();
    let trip = arg_str(args, "trip").ok_or("нужен номер поездки")?;
    trip_exists(store, &trip)?;
    let from = arg_date(args, "from")?.ok_or("from: с какого числа")?;
    let to = arg_date(args, "to")?.ok_or("to: по какое число")?;
    if to < from {
        return Err("дата «по» раньше даты «с»".into());
    }
    let order = WorkOrder { id: book::new_id().map_err(e)?, trip: trip.clone(), number: arg_str(args, "number").unwrap_or_default(), object: arg_str(args, "object").unwrap_or_default(), from, to, att: Vec::new(), note: String::new(), extra: Default::default() };
    ot::check_order(&order).map_err(e)?;
    let name = ot::order_file_name(&order);
    let text = format!("Записан наряд: {name}, поездка {trip} — снимки добавьте в «Кармане»");
    let result = json!({ "id": order.id, "trip": trip, "number": order.number, "from": from, "to": to, "object": order.object, "file_name": name + ".pdf" });
    let id = order.id.clone();
    Ok(Change { bodies: vec![EventBody::OrderPut { order }], undo: vec![EventBody::OrderDelete { id }], text, result })
}

fn create_trip(store: &Store, args: &Value) -> ToolResult<Change> {
    let id = arg_str(args, "id").ok_or("нужен номер поездки")?;
    if store.trips().map_err(|x| x.to_string())?.iter().any(|t| t.id == id) {
        return Err(format!("поездка {id} уже есть"));
    }
    let city = arg_str(args, "city").ok_or("нужен город")?;
    let start = arg_date(args, "start")?.ok_or("start: дата начала")?;
    let end = arg_date(args, "end")?.ok_or("end: дата окончания")?;
    if end < start {
        return Err("дата окончания раньше начала".into());
    }
    if id.chars().count() > 20 || city.chars().count() > 80 {
        return Err("слишком длинный номер или город".into());
    }
    let mut plan = [0 as Kop; 5];
    for (i, key) in ["tickets", "hospitality", "lodging", "per_diem", "transport"].iter().enumerate() {
        plan[i] = arg_kop(args, key)?.unwrap_or(0);
    }
    let trip = Trip {
        id: id.clone(),
        city: city.clone(),
        start,
        end,
        plan_start: start,
        plan_end: end,
        status: TripStatus::Request,
        plan,
        fixed_rate: None,
        own_rate: None,
        req_no: arg_str(args, "request_no").unwrap_or_default(),
        req_date: None,
        goal: arg_str(args, "goal").unwrap_or_default(),
        region: String::new(),
        plan_notes: Default::default(),
        notes: Default::default(),
        deadline: None,
        just: None,
        extra: Map::new(),
    };
    let text = format!("Создана поездка {id} {city}, {}–{}", start.dotted(), end.dotted());
    let result = json!({ "id": id, "city": city, "start": start, "end": end, "status": TripStatus::Request.title(), "plan_total": rub(trip.plan_total()) });
    Ok(Change { bodies: vec![EventBody::TripPut { trip }], undo: vec![EventBody::TripDelete { id }], text, result })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    fn store(dir: &std::path::Path) -> Store {
        let mut s = Store::open(&dir.join("t.db"), &[7u8; 32], "dev-a").unwrap();
        book::ensure_defaults(&mut s).unwrap();
        s.append_all(demo::book_events()).unwrap();
        s
    }

    fn no_rate(_: &str, _: Date) -> Option<f64> {
        None
    }

    #[test]
    fn every_tool_has_a_schema_and_unique_name() {
        let tools = list();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), tools.len());
        for t in &tools {
            assert!(t.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}", t.name);
            assert_eq!(t.input_schema["type"], "object", "{}", t.name);
            assert!(!t.description.is_empty());
        }
        assert!(is_write("add_operation") && !is_write("summary") && !is_write("nope"));
    }

    #[test]
    fn reads_the_demo_book() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let today = demo::today();
        let sum = read(&s, today, "summary", &json!({})).unwrap();
        assert_eq!(sum["mine"], json!(142513.5), "«Мои деньги» как на обзоре");
        assert_eq!(sum["company_money_in_trips"], json!(27300.0));
        assert_eq!(sum["to_return_to_company"], json!(75858.9));
        let ops = read(&s, today, "operations", &json!({ "trip": "573", "from": "2026-10-01", "to": "2026-11-30", "scope": "trip" })).unwrap();
        assert!(ops["found"].as_u64().unwrap() > 0);
        assert!(ops["operations"].as_array().unwrap().iter().all(|o| o["trip"] == "573"));
        let t = read(&s, today, "trip", &json!({ "id": "573" })).unwrap();
        assert_eq!(t["trip"]["city"], "Чекмагуш");
        assert_eq!(t["report_1c"].as_array().unwrap().len(), 5);
        assert_eq!(t["overtime"]["work_orders"].as_array().unwrap().len(), 2);
        let ov = read(&s, today, "overtime", &json!({ "trip": "573" })).unwrap();
        assert_eq!(ov["table"]["totals"]["x1_5"], json!(8.0));
        assert_eq!(ov["table"]["totals"]["x2"], json!(43.5));
        let month = read(&s, today, "overtime", &json!({ "month": "2026-10" })).unwrap();
        assert_eq!(month["title"], "Общая таблица за октябрь 2026");
        assert!(read(&s, today, "trip", &json!({ "id": "999" })).unwrap_err().contains("999"));
        assert!(read(&s, today, "operations", &json!({ "from": "вчера" })).unwrap_err().contains("ГГГГ-ММ-ДД"));
        let cats = read(&s, today, "categories", &json!({})).unwrap();
        assert_eq!(cats["trip_expense"][2]["name"], "Такси и транспорт");
        assert!(read(&s, today, "stats", &json!({ "span": "month" })).unwrap()["months"].as_array().unwrap().len() == 12);
        assert!(read(&s, today, "debts", &json!({})).is_ok() && read(&s, today, "budget", &json!({})).is_ok());
        assert!(read(&s, today, "add_operation", &json!({})).unwrap_err().contains("запись"));
    }

    #[test]
    fn add_operation_follows_the_entry_rules() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(dir.path());
        let today = demo::today();
        // Такси в идущей поездке без документа: рабочий расход на рабочей карте.
        let c = plan(&s, today, "add_operation", &json!({ "type": "expense", "amount": 640, "category": "taxi", "scope": "trip", "title": "Такси до объекта" }), &no_rate).unwrap();
        let EventBody::OpPut { op } = &c.bodies[0] else { panic!() };
        assert_eq!((op.kind, op.amount, op.account.as_str(), op.trip.as_deref(), op.doc), (OpKind::Work, 64000, "acc-work", Some("588"), None));
        assert!(c.text.contains("Такси до объекта") && c.text.contains("поездка 588") && c.text.contains("из суточных"), "{}", c.text);
        assert!(matches!(&c.undo[0], EventBody::OpDelete { id } if id == &op.id));
        s.append_all(c.bodies.clone()).unwrap();
        s.append_all(c.undo.clone()).unwrap();
        assert!(!s.ops().unwrap().iter().any(|o| o.id == op.id), "«Вернуть» убирает запись");
        // Питание — за счёт суточных; с документом — тоже питание, без отметки.
        let c = plan(&s, today, "add_operation", &json!({ "type": "expense", "amount": "1 250,50", "category": "food", "scope": "trip", "trip": "573", "has_document": true }), &no_rate).unwrap();
        let EventBody::OpPut { op } = &c.bodies[0] else { panic!() };
        assert_eq!((op.kind, op.amount, op.doc, op.title.as_str()), (OpKind::Food, 125050, None, "Питание"));
        // Семейный расход — на личной карте.
        let c = plan(&s, today, "add_operation", &json!({ "type": "expense", "amount": 3480, "category": "groceries", "scope": "family", "date": "2026-11-05" }), &no_rate).unwrap();
        let EventBody::OpPut { op } = &c.bodies[0] else { panic!() };
        assert_eq!((op.kind, op.family, op.account.as_str(), op.date), (OpKind::Personal, true, "acc-personal", Date::parse("2026-11-05").unwrap()));
        // Счёт в тенге: нужен курс.
        let args = json!({ "type": "expense", "amount": 5000, "category": "cafe", "scope": "personal", "account": "acc-kzt" });
        assert!(plan(&s, today, "add_operation", &args, &no_rate).unwrap_err().contains("курса KZT"));
        let c = plan(&s, today, "add_operation", &args, &|cur, _| (cur == "KZT").then_some(0.18)).unwrap();
        let EventBody::OpPut { op } = &c.bodies[0] else { panic!() };
        assert_eq!((op.amount, op.orig, op.cur.as_deref()), (90000, Some(500000), Some("KZT")));
        // Ошибки — понятным текстом.
        let bad = |a: Value| plan(&s, today, "add_operation", &a, &no_rate).unwrap_err();
        assert!(bad(json!({ "type": "expense", "amount": 100, "category": "salary", "scope": "trip" })).contains("tickets"));
        assert!(bad(json!({ "type": "expense", "amount": 0, "category": "taxi", "scope": "trip" })).contains("больше нуля"));
        assert!(bad(json!({ "type": "income", "amount": 100, "category": "salary", "scope": "family" })).contains("доход"));
        assert!(bad(json!({ "type": "expense", "amount": 100, "category": "taxi", "scope": "trip", "trip": "777" })).contains("777"));
        assert!(bad(json!({ "type": "expense", "amount": 100, "category": "cafe", "scope": "personal", "account": "acc-nope" })).contains("acc-nope"));
    }

    #[test]
    fn update_and_delete_keep_an_undo() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(dir.path());
        let today = demo::today();
        let op = s.ops().unwrap().into_iter().find(|o| o.kind == OpKind::Work && o.doc.is_none()).unwrap();
        let c = plan(&s, today, "update_operation", &json!({ "id": op.id, "amount": 700, "has_document": true }), &no_rate).unwrap();
        s.append_all(c.bodies).unwrap();
        let now = s.ops().unwrap().into_iter().find(|o| o.id == op.id).unwrap();
        assert_eq!((now.amount, now.doc), (70000, Some(DocKind::Photo)));
        s.append_all(c.undo).unwrap();
        assert_eq!(s.ops().unwrap().into_iter().find(|o| o.id == op.id).unwrap(), op, "«Вернуть» возвращает как было");
        assert!(plan(&s, today, "update_operation", &json!({ "id": op.id }), &no_rate).unwrap_err().contains("ничего"));
        let transfer = s.ops().unwrap().into_iter().find(|o| o.kind == OpKind::Transfer).unwrap();
        assert!(plan(&s, today, "update_operation", &json!({ "id": transfer.id, "title": "x" }), &no_rate).unwrap_err().contains("не правится"));
        let c = plan(&s, today, "delete_operation", &json!({ "id": op.id }), &no_rate).unwrap();
        s.append_all(c.bodies).unwrap();
        assert!(!s.ops().unwrap().iter().any(|o| o.id == op.id));
        s.append_all(c.undo).unwrap();
        assert!(s.ops().unwrap().iter().any(|o| o.id == op.id));
    }

    #[test]
    fn work_days_orders_and_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(dir.path());
        let today = demo::today();
        let c = plan(&s, today, "set_work_day", &json!({ "trip": "573", "date": "2026-10-30", "start": "8", "end": "21.30", "comment": "Испытания" }), &no_rate).unwrap();
        assert!(c.text.contains("30.10.2026: 8:00–21:30"), "{}", c.text);
        s.append_all(c.bodies).unwrap();
        let ov = read(&s, today, "overtime", &json!({ "trip": "573" })).unwrap();
        assert_eq!(ov["table"]["totals"]["x1_5"], json!(10.0), "8 + 2 часа");
        s.append_all(c.undo).unwrap();
        assert_eq!(read(&s, today, "overtime", &json!({ "trip": "573" })).unwrap()["table"]["totals"]["x1_5"], json!(8.0));
        // Правка существующего дня возвращает прежнее время.
        let c = plan(&s, today, "set_work_day", &json!({ "trip": "573", "date": "2026-10-23", "start": "9:00", "end": "2:00" }), &no_rate).unwrap();
        assert!(matches!(&c.undo[0], EventBody::DayPut { day } if day.start == "8:00"));
        assert!(plan(&s, today, "set_work_day", &json!({ "trip": "573", "date": "2026-10-30", "start": "25:00", "end": "1:00" }), &no_rate).unwrap_err().contains("8:00"));
        assert!(plan(&s, today, "set_work_day", &json!({ "trip": "573", "date": "2026-10-30" }), &no_rate).unwrap_err().contains("пустой"));
        let c = plan(&s, today, "add_work_order", &json!({ "trip": "573", "number": "4/31", "from": "2026-10-30", "to": "2026-10-31", "object": "Чекмагуш" }), &no_rate).unwrap();
        assert_eq!(c.result["file_name"], "Наряд 4-31 с 30.10.2026 по 31.10.2026.pdf");
        assert!(plan(&s, today, "add_work_order", &json!({ "trip": "573", "from": "2026-10-31", "to": "2026-10-30" }), &no_rate).unwrap_err().contains("раньше"));
        let c = plan(&s, today, "create_trip", &json!({ "id": "590", "city": "Уфа", "start": "2026-11-12", "end": "2026-11-18", "lodging": 21000, "per_diem": 4900 }), &no_rate).unwrap();
        let EventBody::TripPut { trip } = &c.bodies[0] else { panic!() };
        assert_eq!((trip.status, trip.plan[2], trip.plan[3], trip.plan_total()), (TripStatus::Request, 2_100_000, 490_000, 2_590_000));
        assert!(plan(&s, today, "create_trip", &json!({ "id": "573", "city": "Уфа", "start": "2026-11-12", "end": "2026-11-18" }), &no_rate).unwrap_err().contains("уже есть"));
        s.append_all(c.bodies).unwrap();
        assert!(s.trips().unwrap().iter().any(|t| t.id == "590"));
        s.append_all(c.undo).unwrap();
        assert!(!s.trips().unwrap().iter().any(|t| t.id == "590"), "«Вернуть» убирает поездку");
    }
}
