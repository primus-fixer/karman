//! ИИ в «Кармане»: распознавание наряда-допуска моделью Claude.
//!
//! Ядро готовит запрос к Claude API (снимки или PDF наряда и схему ответа), разбирает ответ
//! и предлагает рабочие дни по правилу таблицы переработок:
//! - будни — с начала рабочего дня (8:00), окончание по наряду, но не раньше конца дня (17:00);
//! - выходные и праздники — по наряду, как есть;
//! - допуск в будни после конца рабочего дня (вечером) — тоже по наряду;
//! - дни до ввода работника в бригаду (и после вывода из неё) не берутся.
//!
//! Сеть, ключ API и счётчик запросов — в приложении. Сохраняет человек, после проверки.

use crate::calendar;
use crate::date::Date;
use crate::overtime::{time_text, Rules};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Версия Claude API (заголовок anthropic-version).
pub const API_VERSION: &str = "2026-10-21";
/// Документы — снимки и PDF нарядов — читает старшая модель.
pub const MODEL_DOCS: &str = "claude-opus-5-5";
/// Короткие задачи: проверка ключа, ввод фразой.
pub const MODEL_SHORT: &str = "claude-haiku-5-5";
/// Вопросы о деньгах и поездках, проверка отчёта, сводка месяца.
pub const MODEL_CHAT: &str = "claude-sonnet-5-5";
/// Ответ с рассуждением модели и JSON наряда умещается с запасом.
const MAX_TOKENS: u32 = 16_000;

/// Названия моделей для экрана.
pub fn model_title(model: &str) -> &'static str {
    match model {
        MODEL_DOCS => "Claude Opus 5.5",
        MODEL_SHORT => "Claude Haiku 5.5",
        MODEL_CHAT => "Claude Sonnet 5.5",
        _ => "Claude",
    }
}

/// Цена за миллион токенов в долларах США: (вход, выход) — по прайсу Anthropic на ноябрь 2026.
pub fn price(model: &str) -> (f64, f64) {
    match model {
        MODEL_SHORT => (0.10, 0.50),
        MODEL_CHAT => (2.0, 10.0),
        _ => (4.0, 20.0),
    }
}

/// Токены одного запроса: обычный вход, выход и кэш подсказки (запись и чтение).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub cache_read: u64,
}

/// Чтение из кэша — доля цены входа: у Opus и Sonnet 5.5 — 5 %, у остальных — 10 %; запись — 125 %.
fn cache_read_share(model: &str) -> f64 {
    if model == MODEL_DOCS || model == MODEL_CHAT {
        0.05
    } else {
        0.10
    }
}

/// Оценка стоимости запроса, в долларах США.
pub fn cost_usd(model: &str, u: Usage) -> f64 {
    let (i, o) = price(model);
    (u.input as f64 * i + u.cache_write as f64 * i * 1.25 + u.cache_read as f64 * i * cache_read_share(model) + u.output as f64 * o) / 1_000_000.0
}

/// Ключ API похож на настоящий: «sk-ant-…», без пробелов.
pub fn check_key(key: &str) -> Result<(), String> {
    let k = key.trim();
    if !k.starts_with("sk-ant-") {
        return Err("Ключ API Claude начинается с «sk-ant-» — скопируйте его из Claude Console целиком".into());
    }
    if k.len() < 24 || k.len() > 400 || !k.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("Ключ API скопирован не целиком или с лишними знаками".into());
    }
    Ok(())
}

/// ID рабочего пространства Claude Console: «wrkspc_…» или пусто.
pub fn check_workspace(id: &str) -> Result<(), String> {
    let t = id.trim();
    if t.is_empty() || (t.starts_with("wrkspc_") && t.len() <= 100 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
        Ok(())
    } else {
        Err("ID рабочего пространства начинается с «wrkspc_» — его видно в Claude Console на странице Workspaces".into())
    }
}

/// «…a1B2» — хвост ключа для экрана.
pub fn key_tail(key: &str) -> String {
    let k = key.trim();
    let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("sk-ant-…{tail}")
}

/// Файл наряда для запроса: снимок (JPEG, PNG, WebP) или PDF.
pub struct Doc<'a> {
    pub mime: &'a str,
    pub data: &'a [u8],
}

const IMAGE_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Можно ли отправить такой файл.
pub fn doc_supported(mime: &str) -> bool {
    mime == "application/pdf" || IMAGE_TYPES.contains(&mime)
}

/// Запрос проверки ключа: одно слово от младшей модели.
pub fn check_request() -> Value {
    json!({
        "model": MODEL_SHORT,
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "Ответь одним словом: готово"}],
    })
}

const ORDER_SYSTEM: &str = "Ты читаешь наряд-допуск для работы в электроустановках (форма из Правил по охране труда при эксплуатации электроустановок): снимки или PDF его листов, иногда нескольких нарядов подряд. Перепиши данные наряда в JSON по схеме.

Правила:
- Ничего не придумывай и не достраивай. Чего не видно или не разобрать, оставь пустой строкой и коротко назови в problems.
- Даты пиши как ГГГГ-ММ-ДД. Год из двух цифр — это 20ГГ: «06.10.26» → «2026-10-06». Время пиши как ЧЧ:ММ по 24 часам: «09-40» → «09:40».
- Фамилии пиши в именительном падеже с инициалами, без группы по электробезопасности: в наряде «Фирсовым К.С. гр.V» → «Фирсов К.С.».
- members — все, кого назначили при выдаче наряда: ответственный руководитель работ, производитель работ, наблюдающий (если назначен) и члены бригады. Допускающего из оперативного персонала и выдавшего наряд не включай.
- admissions — все заполненные строки таблицы «Ежедневный допуск к работе и время её окончания» по порядку, со всех листов: start — дата и время допуска (графа «Дата, время» в части «Бригада получила целевой инструктаж и допущена»), end — дата и время из части «Работа закончена, бригада удалена». Если работа закончилась после полуночи, в end — дата следующего дня. Зачёркнутые пустые строки пропускай.
- changes — строки таблицы «Изменения в составе бригады»: кто введён (added), кто выведен (removed), дата и время.
- start и end — «Работу начать» и «Работу закончить»; extended — «Наряд-допуск продлил по», если заполнено.
- place — подразделение или объект, work — что поручено, коротко.
- object — одна строка для таблицы переработок: населённый пункт или объект и суть работ, до 80 знаков. Например: «Андреевка. Замена устройств РЗА 1СШ 6 кВ».
- Если листов нескольких разных нарядов, бери тот, у которого больше заполненных строк допуска, и напиши о других в problems.
- Если на снимках не наряд-допуск, is_order — false, остальное пусто.";

fn moment_schema(what: &str) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["date", "time"],
        "description": what,
        "properties": {
            "date": {"type": "string", "description": "ГГГГ-ММ-ДД или пустая строка"},
            "time": {"type": "string", "description": "ЧЧ:ММ или пустая строка"}
        }
    })
}

/// Схема ответа: все поля обязательны, пустое — пустой строкой или пустым списком.
pub fn order_schema() -> Value {
    let names = |what: &str| json!({"type": "array", "items": {"type": "string"}, "description": what});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["is_order", "number", "place", "work", "object", "start", "end", "extended", "members", "admissions", "changes", "problems"],
        "properties": {
            "is_order": {"type": "boolean", "description": "На снимках наряд-допуск"},
            "number": {"type": "string", "description": "Номер наряда, как написан: «4/30»"},
            "place": {"type": "string", "description": "Подразделение или объект"},
            "work": {"type": "string", "description": "Что поручено — коротко"},
            "object": {"type": "string", "description": "Для таблицы переработок: место и суть работ, до 80 знаков"},
            "start": moment_schema("Работу начать"),
            "end": moment_schema("Работу закончить"),
            "extended": moment_schema("Наряд-допуск продлил по"),
            "members": names("Бригада при выдаче наряда, в именительном падеже: «Фирсов К.С.»"),
            "admissions": {
                "type": "array",
                "description": "Ежедневный допуск к работе и время её окончания — по строкам",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["place", "start", "end"],
                    "properties": {
                        "place": {"type": "string", "description": "Наименование рабочего места"},
                        "start": moment_schema("Допуск: дата и время"),
                        "end": moment_schema("Работа закончена: дата и время")
                    }
                }
            },
            "changes": {
                "type": "array",
                "description": "Изменения в составе бригады",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["added", "removed", "at"],
                    "properties": {
                        "added": names("Введены в состав бригады"),
                        "removed": names("Выведены из состава бригады"),
                        "at": moment_schema("Дата и время изменения")
                    }
                }
            },
            "problems": {"type": "string", "description": "Что не удалось прочитать или в чём сомнение; пусто, если всё ясно"}
        }
    })
}

/// Листы документа — блоками снимков и PDF; Claude лучше читает, когда задание идёт после них.
fn files_content(docs: &[Doc]) -> Vec<Value> {
    docs.iter()
        .map(|d| {
            let data = B64.encode(d.data);
            if d.mime == "application/pdf" {
                json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": data}})
            } else {
                json!({"type": "image", "source": {"type": "base64", "media_type": d.mime, "data": data}})
            }
        })
        .collect()
}

/// Запрос с ответом строго по схеме (structured outputs).
fn structured(model: &str, max_tokens: u32, system: &str, content: Vec<Value>, schema: Value) -> Value {
    json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": content}],
        "output_config": {"format": {"type": "json_schema", "schema": schema}},
    })
}

fn sheets_text(n: usize, what: &str) -> String {
    match n {
        1 => "Один файл".to_string(),
        n => format!("Файлов: {n} — это листы {what} по порядку"),
    }
}

/// Запрос распознавания наряда: сначала листы, потом задание. Поездка нужна для года и проверки дат.
pub fn order_request(docs: &[Doc], trip_from: Date, trip_to: Date) -> Value {
    let mut content = files_content(docs);
    content.push(json!({
        "type": "text",
        "text": format!("{}. Командировка, в которой выдан наряд: с {} по {}. Перепиши наряд по схеме.", sheets_text(docs.len(), "наряда"), trip_from.dotted(), trip_to.dotted())
    }));
    structured(MODEL_DOCS, MAX_TOKENS, ORDER_SYSTEM, content, order_schema())
}

/// Токены из ответа Claude API.
pub fn usage(v: &Value) -> Usage {
    let u = &v["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    Usage { input: n("input_tokens"), output: n("output_tokens"), cache_write: n("cache_creation_input_tokens"), cache_read: n("cache_read_input_tokens") }
}

/// Текст ответа: блоки «text» по порядку; рассуждения модели («thinking») пропускаются.
pub fn reply_text(v: &Value) -> Result<String, String> {
    match v["stop_reason"].as_str() {
        Some("refusal") => return Err("Claude отказался разбирать эти снимки".into()),
        Some("max_tokens") => return Err("Ответ Claude не уместился — попробуйте отправить меньше листов".into()),
        _ => {}
    }
    let text: String = v["content"]
        .as_array()
        .map(|blocks| blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""))
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err("Claude не прислал ответа — попробуйте ещё раз".into());
    }
    Ok(text)
}

/// Ошибка Claude API — понятным текстом. Сообщение API (по-английски) — в скобках, для поиска причины.
pub fn api_error(status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let kind = v["error"]["type"].as_str().unwrap_or("");
    let msg = v["error"]["message"].as_str().unwrap_or("").trim().to_string();
    let low = msg.to_lowercase();
    let short: String = msg.chars().take(200).collect();
    let tail = if short.is_empty() { String::new() } else { format!(" ({short})") };
    let text = if low.contains("not scoped to a workspace") {
        "Ключ не привязан к рабочему пространству. Создайте ключ для одного пространства или впишите ID пространства (wrkspc_…) в настройках ИИ".to_string()
    } else if low.contains("credit balance") {
        "На счёте Claude Console не хватает средств — пополните баланс в разделе Billing".to_string()
    } else if low.contains("spend limit") {
        "Достигнут лимит расходов, заданный в Claude Console".to_string()
    } else if low.contains("workspace") && (status == 404 || low.contains("valid workspace id")) {
        "Рабочее пространство с таким ID не найдено — проверьте ID в настройках ИИ".to_string()
    } else {
        match (status, kind) {
            (401, _) | (_, "authentication_error") => "Ключ API не подходит: он отозван, истёк или скопирован не целиком. Создайте новый в Claude Console".to_string(),
            (402, _) | (_, "billing_error") => format!("Claude Console: не настроена или не прошла оплата{tail}"),
            (403, _) | (_, "permission_error") => format!("Claude API отклонил запрос: у ключа нет доступа{tail}"),
            (404, _) => format!("Claude API: модель или адрес не найдены{tail}"),
            (413, _) | (_, "request_too_large") => "Файлы наряда слишком большие для одного запроса — уберите лишние листы".to_string(),
            (429, _) | (_, "rate_limit_error") => "Слишком много запросов или исчерпан месячный лимит Claude Console — повторите позже".to_string(),
            (529, _) | (_, "overloaded_error") => "Claude API сейчас перегружен — повторите через несколько минут".to_string(),
            (500..=599, _) => format!("Ошибка на стороне Claude API — повторите позже{tail}"),
            _ => format!("Claude API: ответ {status}{tail}"),
        }
    };
    text
}

// ——— Ответ модели ———

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Moment {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub time: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawAdmission {
    #[serde(default)]
    pub place: String,
    #[serde(default)]
    pub start: Moment,
    #[serde(default)]
    pub end: Moment,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawChange {
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub at: Moment,
}

/// Наряд, как его прочитал Claude.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawOrder {
    #[serde(default = "yes")]
    pub is_order: bool,
    #[serde(default)]
    pub number: String,
    #[serde(default)]
    pub place: String,
    #[serde(default)]
    pub work: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub start: Moment,
    #[serde(default)]
    pub end: Moment,
    #[serde(default)]
    pub extended: Moment,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub admissions: Vec<RawAdmission>,
    #[serde(default)]
    pub changes: Vec<RawChange>,
    #[serde(default)]
    pub problems: String,
}

fn yes() -> bool {
    true
}

/// JSON из ответа; на всякий случай — без обёртки ```json.
pub fn parse_order(text: &str) -> Result<RawOrder, String> {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    serde_json::from_str(t).map_err(|_| "Claude ответил не по форме — попробуйте ещё раз".to_string())
}

/// «2026-10-06», «06.10.26», «06.10.2026» → дата.
pub fn parse_date(s: &str) -> Option<Date> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(d) = Date::parse(t) {
        return Some(d);
    }
    let p: Vec<&str> = t.split(['.', '/']).map(str::trim).collect();
    if p.len() != 3 || p.iter().any(|x| x.is_empty() || !x.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let (d, m, y): (u32, u32, i32) = (p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?);
    let y = match p[2].len() {
        2 => 2000 + y,
        4 => y,
        _ => return None,
    };
    Date::new(y, m, d)
}

/// «09:40», «9:40», «09-40», «09.40», «0940» → минуты от полуночи; «24:00» — 1440.
pub fn parse_clock(s: &str) -> Option<u32> {
    let t: String = s.trim().chars().map(|c| if matches!(c, '-' | '.' | ',' | ' ' | 'ч' | 'h') { ':' } else { c }).collect();
    let t = t.trim_matches(':');
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    let (h, m) = match t.split_once(':') {
        Some(hm) => hm,
        None if t.len() == 4 && digits(t) => (&t[..2], &t[2..]),
        None => (t, "00"),
    };
    if h.len() > 2 || m.len() != 2 || !digits(h) || !digits(m) {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    if h == 24 && m == 0 {
        return Some(24 * 60);
    }
    (h < 24 && m < 60).then_some(h * 60 + m)
}

// ——— Кто есть кто ———

/// Фамилия и инициалы: «Образцов С.В.», «Образцов Сергей Викторович», «С.В. Образцов».
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub surname: String,
    pub initials: Vec<char>,
}

fn norm(s: &str) -> String {
    s.to_lowercase().replace('ё', "е")
}

pub fn person(name: &str) -> Option<Person> {
    let n = norm(name);
    let words: Vec<String> = n.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).map(String::from).collect();
    // Группа по электробезопасности («гр», римские цифры) — не часть имени.
    let words: Vec<String> = words.into_iter().filter(|w| !matches!(w.as_str(), "гр" | "группа" | "i" | "ii" | "iii" | "iv" | "v")).collect();
    let long: Vec<&String> = words.iter().filter(|w| w.chars().count() >= 2).collect();
    let short: Vec<char> = words.iter().filter(|w| w.chars().count() == 1).filter_map(|w| w.chars().next()).collect();
    let surname = (*long.first()?).clone();
    let mut initials: Vec<char> = long.iter().skip(1).filter_map(|w| w.chars().next()).collect();
    initials.extend(short);
    Some(Person { surname, initials })
}

/// Тот же человек: фамилия совпадает (допускается падежное окончание до трёх букв),
/// а если инициалы есть у обоих — совпадает первый.
pub fn same_person(a: &Person, b: &Person) -> bool {
    let (x, y) = (&a.surname, &b.surname);
    let close = |long: &str, short: &str| long.starts_with(short) && short.chars().count() >= 4 && long.chars().count() - short.chars().count() <= 3;
    let surname = x == y || close(x, y) || close(y, x);
    surname && (a.initials.is_empty() || b.initials.is_empty() || a.initials[0] == b.initials[0])
}

// ——— Предложение для таблицы ———

/// Почему день взят так или не взят.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// Будни: с начала рабочего дня, окончание по наряду, но не раньше конца дня.
    Weekday,
    /// Выходной или праздник: по наряду.
    Off,
    /// Допуск в будни после конца рабочего дня: по наряду.
    Evening,
    /// До ввода в бригаду.
    Before,
    /// После вывода из бригады.
    After,
    /// Вне дат поездки.
    Outside,
    /// В наряде не прочитано окончание работы.
    NoEnd,
}

/// День, который Claude нашёл в наряде, и что из него пойдёт в таблицу.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DayPlan {
    pub date: Date,
    /// По наряду: «8:55» – «23:50».
    pub order_start: String,
    pub order_end: String,
    /// В таблицу.
    pub start: String,
    pub end: String,
    pub rule: Rule,
    /// Брать ли день по умолчанию.
    pub take: bool,
}

/// Работник в наряде.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Me {
    /// ФИО указано в настройках переработок.
    pub known: bool,
    /// Нашёлся в наряде.
    pub found: bool,
    /// Как записан в наряде.
    pub name: String,
    /// В бригаде с выдачи наряда.
    pub from_start: bool,
    /// Введён в бригаду: дата и время.
    pub added: Option<Date>,
    pub added_time: String,
    /// Выведен из бригады.
    pub removed: Option<Date>,
    pub removed_time: String,
}

/// Наряд для экрана проверки.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OrderScan {
    /// На снимках не наряд-допуск.
    pub not_order: bool,
    pub number: String,
    pub object: String,
    pub from: Option<Date>,
    pub to: Option<Date>,
    pub me: Me,
    /// Состав бригады одной строкой: для проверки глазами.
    pub brigade: String,
    pub days: Vec<DayPlan>,
    /// Что Claude не смог прочитать.
    pub problems: String,
}

fn clean(s: &str, max: usize) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(max).collect()
}

/// Допуск одного дня: начало и окончание в минутах от полуночи дня начала (окончание может быть больше суток).
fn spans(raw: &RawOrder) -> Vec<(Date, u32, Option<u32>)> {
    let mut out: Vec<(Date, u32, Option<u32>)> = Vec::new();
    for a in &raw.admissions {
        let (Some(day), Some(s)) = (parse_date(&a.start.date), parse_clock(&a.start.time)) else { continue };
        let end = parse_clock(&a.end.time).map(|e| {
            let shift = parse_date(&a.end.date).map(|d| day.days_until(d)).filter(|n| (0..=2).contains(n)).unwrap_or(0) as u32;
            let mut e = e + shift * 1440;
            if e <= s {
                e += 1440;
            }
            e
        });
        // Несколько строк за день (два рабочих места, перерыв) — от первого допуска до последнего окончания.
        match out.iter_mut().find(|x| x.0 == day) {
            Some(x) => {
                x.1 = x.1.min(s);
                x.2 = match (x.2, end) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
            }
            None => out.push((day, s, end)),
        }
    }
    out.sort_by_key(|x| x.0);
    out
}

/// Промежутки в бригаде: с какого дня (None — с выдачи) по какой (None — до конца наряда).
type Spans = Vec<(Option<Date>, Option<Date>)>;

/// Промежутки, когда работник был в бригаде: с выдачи или со дня ввода, по день вывода включительно.
fn presence(raw: &RawOrder, me: &Person) -> (Me, Spans) {
    let mut info = Me { known: true, ..Me::default() };
    let matches = |n: &str| person(n).is_some_and(|p| same_person(me, &p));
    if let Some(n) = raw.members.iter().find(|n| matches(n)) {
        info.found = true;
        info.from_start = true;
        info.name = clean(n, 60);
    }
    let mut events: Vec<(Date, u32, bool, String)> = Vec::new();
    for c in &raw.changes {
        let Some(d) = parse_date(&c.at.date) else { continue };
        let t = parse_clock(&c.at.time).unwrap_or(0);
        for n in c.added.iter().filter(|n| matches(n)) {
            events.push((d, t, true, n.clone()));
        }
        for n in c.removed.iter().filter(|n| matches(n)) {
            events.push((d, t, false, n.clone()));
        }
    }
    events.sort_by_key(|e| (e.0, e.1));
    let mut spans: Spans = Vec::new();
    let mut open: Option<Option<Date>> = if info.from_start { Some(None) } else { None };
    for (d, t, added, name) in events {
        info.found = true;
        if info.name.is_empty() {
            info.name = clean(&name, 60);
        }
        if added {
            if info.added.is_none() {
                info.added = Some(d);
                info.added_time = time_text(t);
            }
            if open.is_none() {
                open = Some(Some(d));
            }
        } else {
            info.removed = Some(d);
            info.removed_time = time_text(t);
            if let Some(from) = open.take() {
                spans.push((from, Some(d)));
            }
        }
    }
    if let Some(from) = open {
        spans.push((from, None));
    }
    (info, spans)
}

/// Наряд → поля наряда и дни для таблицы переработок. `who` — ФИО из настроек переработок.
pub fn plan(raw: &RawOrder, who: &str, trip_from: Date, trip_to: Date, rules: &Rules) -> OrderScan {
    let days_raw = spans(raw);
    let me_person = person(who);
    let (me, presence) = match &me_person {
        Some(p) => presence(raw, p),
        None => (Me::default(), Vec::new()),
    };
    // ФИО не указано или не нашлось в наряде — дни не отсеиваются: решит человек.
    let filter = me.found;
    let (tf, tt) = (trip_from.min(trip_to), trip_from.max(trip_to));
    let mut days = Vec::new();
    for (date, s, e) in days_raw {
        let order_start = time_text(s);
        let order_end = e.map(time_text).unwrap_or_default();
        let inside = presence.iter().any(|(a, b)| a.is_none_or(|a| date >= a) && b.is_none_or(|b| date <= b));
        let before = presence.iter().all(|(a, _)| a.is_some_and(|a| date < a));
        let off = calendar::is_day_off(date);
        let (start, end, mut rule) = match e {
            None if off || s >= rules.day_end => (String::new(), String::new(), Rule::NoEnd),
            // Окончание не прочитано, но в будни день всё равно до конца рабочего дня.
            None => (time_text(rules.day_start), time_text(rules.day_end), Rule::NoEnd),
            Some(e) if off => (time_text(s), time_text(e), Rule::Off),
            Some(e) if s >= rules.day_end => (time_text(s), time_text(e), Rule::Evening),
            Some(e) => (time_text(rules.day_start), time_text(e.max(rules.day_end)), Rule::Weekday),
        };
        let mut take = !start.is_empty();
        if filter && !inside {
            rule = if before { Rule::Before } else { Rule::After };
            take = false;
        } else if date < tf || date > tt {
            rule = Rule::Outside;
            take = false;
        }
        days.push(DayPlan { date, order_start, order_end, start, end, rule, take });
    }
    let from = parse_date(&raw.start.date).or_else(|| days.first().map(|d| d.date));
    let to = [parse_date(&raw.end.date), parse_date(&raw.extended.date), days.last().map(|d| d.date)].into_iter().flatten().max();
    let to = match (from, to) {
        (Some(f), Some(t)) if t < f => Some(f),
        (_, t) => t,
    };
    let object = match clean(&raw.object, 300) {
        o if !o.is_empty() => o,
        _ => clean(&[raw.place.trim(), raw.work.trim()].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(". "), 300),
    };
    let mut brigade = raw.members.iter().map(|m| clean(m, 60)).filter(|m| !m.is_empty()).collect::<Vec<_>>().join(", ");
    for c in &raw.changes {
        let when = match (parse_date(&c.at.date), parse_clock(&c.at.time)) {
            (Some(d), Some(t)) => format!("{} {}", d.dotted(), time_text(t)),
            (Some(d), None) => d.dotted(),
            _ => String::new(),
        };
        let list = |v: &Vec<String>| v.iter().map(|m| clean(m, 60)).filter(|m| !m.is_empty()).collect::<Vec<_>>().join(", ");
        let (a, r) = (list(&c.added), list(&c.removed));
        if !a.is_empty() {
            brigade.push_str(&format!("; {when} введены: {a}"));
        }
        if !r.is_empty() {
            brigade.push_str(&format!("; {when} выведены: {r}"));
        }
    }
    OrderScan {
        not_order: !raw.is_order,
        number: clean(&raw.number, 40),
        object,
        from,
        to,
        me: Me { known: me_person.is_some(), ..me },
        brigade: brigade.trim_start_matches("; ").to_string(),
        days,
        problems: clean(&raw.problems, 600),
    }
}

// ——— Ввод фразой ———

const PHRASE_SYSTEM: &str = "Ты помогаешь записать операцию в приложение личных и командировочных финансов «Карман». Человек пишет или говорит одну фразу — например «такси 640 до объекта, чек есть» или «зарплата 95 тысяч». Разбери её в поля по схеме, пользуясь списками счетов, поездок и категорий из контекста.

Правила:
- kind: expense — расход, income — доход, transfer — перевод между своими счетами (снял наличные, положил на карту, обмен валюты).
- scope: trip — рабочий расход или доход поездки (аванс, возмещение, всё, что для работы в командировке); personal — личное; family — семейный расход; personal_tagged — личный расход во время поездки (сувениры, личные покупки).
- Если человек не сказал иначе, а сейчас идёт поездка (active в контексте), расход на транспорт, проживание, билеты, питание и материалы — это trip этой поездки; продукты домой, подписки и личное — personal.
- category — только код из списков контекста для этого вида и scope.
- account — код счёта из контекста: для поездки обычно рабочая карта (kind work), для личного — личная (personal); наличные — если сказано «наличными».
- amount — число в валюте счёта; «тыс», «тысяч» — умножь на 1000. Не назвал сумму — 0.
- date — ГГГГ-ММ-ДД; «вчера», «в пятницу» — от сегодняшней даты из контекста; не сказано — пусто.
- title — короткий комментарий как в ленте: «Такси до объекта». Не придумывай лишнего.
- has_document — сказал, что есть чек, билет, квитанция или документ.
- Чего не понял — коротко в unclear, остальное заполни как можно точнее.";

/// Схема ответа для фразы.
pub fn phrase_schema() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["kind", "amount", "currency", "scope", "trip", "category", "account", "to_account", "date", "title", "has_document", "unclear"],
        "properties": {
            "kind": {"type": "string", "enum": ["expense", "income", "transfer"]},
            "amount": {"type": "number", "description": "Сумма в валюте счёта; 0 — если не названа"},
            "currency": s("Валюта, если названа: RUB, KZT, USD, EUR; иначе пусто"),
            "scope": {"type": "string", "enum": ["trip", "personal", "family", "personal_tagged"]},
            "trip": s("Номер поездки из контекста или пусто"),
            "category": s("Код категории из контекста"),
            "account": s("Код счёта из контекста или пусто"),
            "to_account": s("Перевод: код счёта, куда; иначе пусто"),
            "date": s("ГГГГ-ММ-ДД или пусто — сегодня"),
            "title": s("Короткий комментарий"),
            "has_document": {"type": "boolean", "description": "Есть чек или документ"},
            "unclear": s("Что непонятно; пусто, если всё ясно")
        }
    })
}

/// Запрос для фразы: контекст (счета, поездки, категории) и сама фраза — младшей модели.
pub fn phrase_request(text: &str, today: Date, context: &Value) -> Value {
    let content = vec![json!({
        "type": "text",
        "text": format!("Сегодня {} ({}). Контекст «Кармана»:\n{}\n\nФраза человека: «{}»", today, WEEKDAYS_FULL[(today.weekday() - 1) as usize], context, text.trim())
    })];
    structured(MODEL_SHORT, 2000, PHRASE_SYSTEM, content, phrase_schema())
}

const WEEKDAYS_FULL: [&str; 7] = ["понедельник", "вторник", "среда", "четверг", "пятница", "суббота", "воскресенье"];

/// Фраза, как её разобрал Claude.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawPhrase {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub amount: f64,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub trip: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub to_account: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub has_document: bool,
    #[serde(default)]
    pub unclear: String,
}

/// Черновик для окна ввода: те же поля, что в нём. Сохраняет человек.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PhraseDraft {
    /// out, in, move
    pub dir: &'static str,
    /// Сумма в валюте счёта; 0 — не названа.
    pub amount: f64,
    /// own, family, trip:573, tag:573
    pub scope: String,
    pub cat: String,
    pub acc: String,
    pub to: String,
    pub date: Date,
    pub comment: String,
    /// Чек есть (для рабочего расхода поездки).
    pub document: bool,
    pub unclear: String,
}

fn json_part<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    serde_json::from_str(t).map_err(|_| "Claude ответил не по форме — попробуйте ещё раз".to_string())
}

pub fn parse_phrase(text: &str) -> Result<RawPhrase, String> {
    json_part(text)
}

/// Разобранная фраза → черновик по правилам окна ввода: только существующие счета, поездки и категории.
pub fn phrase_draft(raw: &RawPhrase, s: &crate::book::Snapshot) -> PhraseDraft {
    use crate::tools::{OWN_IN, OWN_OUT, TRIP_IN, TRIP_OUT};
    use crate::trip::TripStatus;
    let dir = match raw.kind.as_str() {
        "income" => "in",
        "transfer" => "move",
        _ => "out",
    };
    let live: Vec<&crate::book::Account> = s.accounts.iter().map(|a| &a.account).filter(|a| !a.archived).collect();
    let open: Vec<&crate::trip::Trip> = s.trips.iter().map(|t| &t.trip).filter(|t| t.status != TripStatus::Closed).collect();
    let active = crate::tools::active_trip(&s.trips.iter().map(|t| t.trip.clone()).collect::<Vec<_>>(), s.today).map(|t| t.id.clone());
    let trip = open.iter().find(|t| t.id == raw.trip.trim()).map(|t| t.id.clone()).or(active);
    let mut scope = match (dir, raw.scope.as_str(), &trip) {
        ("move", _, _) => "own".to_string(),
        (_, "trip", Some(t)) => format!("trip:{t}"),
        ("out", "personal_tagged", Some(t)) => format!("tag:{t}"),
        ("out", "family", _) => "family".to_string(),
        _ => "own".to_string(),
    };
    let list: &[&str] = match (dir, scope.starts_with("trip:")) {
        ("move", _) => &["transfer"],
        ("out", true) => &TRIP_OUT,
        ("in", true) => &TRIP_IN,
        ("in", false) => &OWN_IN,
        _ => &OWN_OUT,
    };
    let cat = if list.contains(&raw.category.as_str()) {
        raw.category.clone()
    } else if dir == "out" && scope.starts_with("trip:") {
        "taxi".to_string()
    } else {
        list[0].to_string()
    };
    // Аванс и возмещение — доходы поездки; если модель отнесла их к личному — поправим.
    if dir == "in" && !scope.starts_with("trip:") && ["advance", "reimb"].contains(&raw.category.as_str()) {
        if let Some(t) = &trip {
            scope = format!("trip:{t}");
        }
    }
    let cat = if dir == "in" && scope.starts_with("trip:") && !TRIP_IN.contains(&cat.as_str()) { "advance".to_string() } else { cat };
    let want_kind = if scope.starts_with("trip:") { "work" } else { "personal" };
    let by_kind = |k: &str| live.iter().find(|a| a.kind == k).or(live.first()).map(|a| a.id.clone()).unwrap_or_default();
    let mut acc = live.iter().find(|a| a.id == raw.account.trim()).map(|a| a.id.clone()).unwrap_or_else(|| by_kind(want_kind));
    // Названа валюта, а у выбранного счёта другая — берём счёт в этой валюте, если он есть.
    let cur = raw.currency.trim().to_uppercase();
    if !cur.is_empty() && live.iter().find(|a| a.id == acc).is_some_and(|a| a.currency != cur) {
        if let Some(a) = live.iter().find(|a| a.currency == cur) {
            acc = a.id.clone();
        }
    }
    let to = if dir == "move" { live.iter().find(|a| a.id == raw.to_account.trim() && a.id != acc).map(|a| a.id.clone()).unwrap_or_default() } else { String::new() };
    let amount = if raw.amount.is_finite() && raw.amount > 0.0 && raw.amount < 1e10 { (raw.amount * 100.0).round() / 100.0 } else { 0.0 };
    let date = parse_date(&raw.date).filter(|d| s.today.days_until(*d).abs() <= 400).unwrap_or(s.today);
    let document = raw.has_document && dir == "out" && scope.starts_with("trip:") && cat != "food";
    PhraseDraft { dir, amount, scope, cat, acc, to, date, comment: clean(&raw.title, 200), document, unclear: clean(&raw.unclear, 300) }
}

// ——— Документ расхода: билет, счёт гостиницы, чек ———

const EXPENSE_SYSTEM: &str = "Ты читаешь документ о расходе в командировке — электронный билет, маршрутную квитанцию, счёт или чек гостиницы, кассовый чек — снимки или PDF. Перепиши данные по схеме.

Правила:
- Ничего не придумывай: чего не видно, оставь пустым (число — 0) и назови в problems.
- kind: ticket — билет (самолёт, поезд, автобус), hotel — проживание в гостинице, receipt — другой кассовый чек, other — не документ о расходе.
- category: tickets — билеты, lodging — проживание, taxi — такси и транспорт, car — аренда авто и топливо, food — питание, tools — материалы и инструменты.
- amount — итог к оплате, число; currency — RUB, KZT, USD, EUR.
- date — дата оплаты или документа, ГГГГ-ММ-ДД.
- Для билета: route — откуда и куда («Уфа — Москва»), travel_date — дата поездки, number — номер билета.
- Для гостиницы: seller — название гостиницы, stay_from и stay_to — даты заезда и выезда, nights — ночей, number — номер счёта.
- title — короткий комментарий для ленты: «Билет Уфа — Москва» или «Гостиница «Башкирия»».";

pub fn expense_schema() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["kind", "category", "amount", "currency", "date", "number", "seller", "route", "travel_date", "stay_from", "stay_to", "nights", "title", "problems"],
        "properties": {
            "kind": {"type": "string", "enum": ["ticket", "hotel", "receipt", "other"]},
            "category": {"type": "string", "enum": ["tickets", "lodging", "taxi", "car", "food", "tools"]},
            "amount": {"type": "number", "description": "Итог к оплате"},
            "currency": s("RUB, KZT, USD, EUR"),
            "date": s("Дата оплаты или документа, ГГГГ-ММ-ДД"),
            "number": s("Номер билета или счёта"),
            "seller": s("Перевозчик, гостиница или продавец"),
            "route": s("Билет: откуда и куда"),
            "travel_date": s("Билет: дата поездки, ГГГГ-ММ-ДД"),
            "stay_from": s("Гостиница: заезд, ГГГГ-ММ-ДД"),
            "stay_to": s("Гостиница: выезд, ГГГГ-ММ-ДД"),
            "nights": {"type": "number", "description": "Гостиница: ночей"},
            "title": s("Короткий комментарий"),
            "problems": s("Что не удалось прочитать; пусто, если всё ясно")
        }
    })
}

/// Запрос распознавания документа расхода. Даты поездки — для года и проверки.
pub fn expense_request(docs: &[Doc], today: Date, trip: Option<(Date, Date)>) -> Value {
    let mut content = files_content(docs);
    let when = trip.map(|(a, b)| format!(" Командировка: с {} по {}.", a.dotted(), b.dotted())).unwrap_or_default();
    content.push(json!({"type": "text", "text": format!("{}. Сегодня {}.{when} Перепиши документ по схеме.", sheets_text(docs.len(), "документа"), today.dotted())}));
    structured(MODEL_DOCS, MAX_TOKENS, EXPENSE_SYSTEM, content, expense_schema())
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawExpense {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub amount: f64,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub number: String,
    #[serde(default)]
    pub seller: String,
    #[serde(default)]
    pub route: String,
    #[serde(default)]
    pub travel_date: String,
    #[serde(default)]
    pub stay_from: String,
    #[serde(default)]
    pub stay_to: String,
    #[serde(default)]
    pub nights: f64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub problems: String,
}

pub fn parse_expense(text: &str) -> Result<RawExpense, String> {
    json_part(text)
}

/// Документ расхода → поля окна ввода.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExpenseDraft {
    pub not_document: bool,
    /// ticket, hotel, receipt
    pub kind: String,
    pub cat: String,
    pub amount: f64,
    pub currency: String,
    pub date: Option<Date>,
    pub comment: String,
    pub problems: String,
}

fn nights_word(n: u32) -> &'static str {
    match (n % 10, n % 100) {
        (1, x) if x != 11 => "ночь",
        (2..=4, x) if !(12..=14).contains(&x) => "ночи",
        _ => "ночей",
    }
}

pub fn expense_draft(raw: &RawExpense) -> ExpenseDraft {
    let ddmm = |s: &str| parse_date(s).map(|d| d.dotted()[..5].to_string());
    let cat = match raw.kind.as_str() {
        "ticket" => "tickets",
        "hotel" => "lodging",
        _ if crate::tools::TRIP_OUT.contains(&raw.category.as_str()) => raw.category.as_str(),
        _ => "taxi",
    }
    .to_string();
    let route = clean(&raw.route, 80);
    let seller = clean(&raw.seller, 80);
    let comment = match raw.kind.as_str() {
        "ticket" if !route.is_empty() => match ddmm(&raw.travel_date) {
            Some(d) => format!("Билет {route}, {d}"),
            None => format!("Билет {route}"),
        },
        "hotel" => {
            let name = if seller.is_empty() { "Проживание".to_string() } else { format!("Проживание: {seller}") };
            let n = if raw.nights.is_finite() && raw.nights >= 1.0 && raw.nights < 400.0 { raw.nights.round() as u32 } else { 0 };
            match (ddmm(&raw.stay_from), ddmm(&raw.stay_to)) {
                (Some(a), Some(b)) if n > 0 => format!("{name}, {a}–{b}, {n} {}", nights_word(n)),
                (Some(a), Some(b)) => format!("{name}, {a}–{b}"),
                _ => name,
            }
        }
        _ => {
            let t = clean(&raw.title, 120);
            if t.is_empty() { seller } else { t }
        }
    };
    let amount = if raw.amount.is_finite() && raw.amount > 0.0 && raw.amount < 1e10 { (raw.amount * 100.0).round() / 100.0 } else { 0.0 };
    ExpenseDraft {
        not_document: raw.kind == "other",
        kind: raw.kind.clone(),
        cat,
        amount,
        currency: clean(&raw.currency, 3).to_uppercase(),
        date: parse_date(&raw.date),
        comment: clean(&comment, 200),
        problems: clean(&raw.problems, 400),
    }
}

// ——— Поездка из приказа или заявки ———

const TRIP_SYSTEM: &str = "Ты читаешь документ о командировке — приказ о направлении в командировку, служебное задание или заявку на командировку с расчётом расходов — снимки или PDF. Перепиши данные по схеме.

Правила:
- Ничего не придумывай: чего нет в документе, оставь пустым (суммы — 0) и назови в problems, если это важно.
- doc_kind: order — приказ или служебное задание, request — заявка с расчётом сумм, other — не документ о командировке.
- city — населённый пункт назначения (без «г.»), region — регион, область или округ, если указан.
- start и end — даты командировки, ГГГГ-ММ-ДД. request_no и request_date — номер и дата документа.
- trip_number — номер командировки, если он указан отдельно от номера документа; иначе пусто.
- goal — цель командировки, как в документе, коротко.
- Суммы по статьям, в рублях числом: tickets — проезд и билеты, hospitality — представительские, lodging — проживание, per_diem — суточные, transport — транспорт и бензин на месте.
- employee — ФИО командируемого, как написано.";

pub fn trip_schema() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    let n = |d: &str| json!({"type": "number", "description": d});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["doc_kind", "trip_number", "request_no", "request_date", "city", "region", "start", "end", "goal", "tickets", "hospitality", "lodging", "per_diem", "transport", "employee", "problems"],
        "properties": {
            "doc_kind": {"type": "string", "enum": ["order", "request", "other"]},
            "trip_number": s("Номер командировки, если указан отдельно"),
            "request_no": s("Номер документа"),
            "request_date": s("Дата документа, ГГГГ-ММ-ДД"),
            "city": s("Населённый пункт назначения"),
            "region": s("Регион, область или округ"),
            "start": s("Начало командировки, ГГГГ-ММ-ДД"),
            "end": s("Окончание командировки, ГГГГ-ММ-ДД"),
            "goal": s("Цель командировки"),
            "tickets": n("Проезд, билеты, рублей"),
            "hospitality": n("Представительские, рублей"),
            "lodging": n("Проживание, рублей"),
            "per_diem": n("Суточные, рублей"),
            "transport": n("Транспорт и бензин на месте, рублей"),
            "employee": s("ФИО командируемого"),
            "problems": s("Что не удалось прочитать; пусто, если всё ясно")
        }
    })
}

pub fn trip_request(docs: &[Doc], today: Date) -> Value {
    let mut content = files_content(docs);
    content.push(json!({"type": "text", "text": format!("{}. Сегодня {}. Перепиши документ о командировке по схеме.", sheets_text(docs.len(), "документа"), today.dotted())}));
    structured(MODEL_DOCS, MAX_TOKENS, TRIP_SYSTEM, content, trip_schema())
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawTripDoc {
    #[serde(default)]
    pub doc_kind: String,
    #[serde(default)]
    pub trip_number: String,
    #[serde(default)]
    pub request_no: String,
    #[serde(default)]
    pub request_date: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub tickets: f64,
    #[serde(default)]
    pub hospitality: f64,
    #[serde(default)]
    pub lodging: f64,
    #[serde(default)]
    pub per_diem: f64,
    #[serde(default)]
    pub transport: f64,
    #[serde(default)]
    pub employee: String,
    #[serde(default)]
    pub problems: String,
}

pub fn parse_trip_doc(text: &str) -> Result<RawTripDoc, String> {
    json_part(text)
}

/// Документ о командировке → поля формы новой поездки. Суммы — в копейках, по статьям 01–05.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TripDraft {
    pub not_document: bool,
    pub id: String,
    pub city: String,
    pub region: String,
    pub req_no: String,
    pub req_date: Option<Date>,
    pub start: Option<Date>,
    pub end: Option<Date>,
    pub goal: String,
    pub plan: [i64; 5],
    pub employee: String,
    pub problems: String,
}

pub fn trip_draft(raw: &RawTripDoc) -> TripDraft {
    let kop = |v: f64| if v.is_finite() && v > 0.0 && v < 1e10 { (v * 100.0).round() as i64 } else { 0 };
    let (start, end) = match (parse_date(&raw.start), parse_date(&raw.end)) {
        (Some(a), Some(b)) if b < a => (Some(b), Some(a)),
        x => x,
    };
    let city = clean(&raw.city, 80);
    let city = city.strip_prefix("г. ").or_else(|| city.strip_prefix("г.")).unwrap_or(&city).trim().to_string();
    TripDraft {
        not_document: raw.doc_kind == "other",
        id: clean(&raw.trip_number, 20),
        city,
        region: clean(&raw.region, 80),
        req_no: clean(&raw.request_no, 40),
        req_date: parse_date(&raw.request_date),
        start,
        end,
        goal: clean(&raw.goal, 500),
        plan: [kop(raw.tickets), kop(raw.hospitality), kop(raw.lodging), kop(raw.per_diem), kop(raw.transport)],
        employee: clean(&raw.employee, 80),
        problems: clean(&raw.problems, 400),
    }
}

// ——— Вопросы о деньгах и поездках ———

/// Инструменты «Кармана» для Claude API — те же, что у коннектора. Список не меняется
/// от настроек: запрет записи проверяется при вызове, а неизменный список нужен для рассуждений модели.
pub fn api_tools() -> Vec<Value> {
    crate::tools::list().into_iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema})).collect()
}

/// Начало разговора: правила «Кармана», сегодняшняя дата и что умеет запись.
pub fn chat_system(today: Date) -> String {
    format!(
        "Ты — Claude внутри приложения «Карман»: отвечаешь человеку о его деньгах и командировках и по его просьбе записываешь операции. Сегодня {} ({}).\n\n{}\n\nОтвечай по-русски, коротко и по делу; числа — как в «Кармане», суммы в рублях. Данные бери только из инструментов «Кармана», не придумывай. Если запись выключена, скажи, что её можно разрешить в «Кармане»: Настройки → ИИ.",
        today,
        WEEKDAYS_FULL[(today.weekday() - 1) as usize],
        crate::mcp::INSTRUCTIONS
    )
}

/// Ход разговора: история сообщений как есть (вместе с блоками рассуждения) и инструменты.
/// Инструменты и правила одинаковы весь разговор — их начало кэшируется: повторные шаги
/// читают его по 5 % цены входа.
pub fn chat_request(system: &str, messages: &[Value]) -> Value {
    json!({
        "model": MODEL_CHAT,
        "max_tokens": 8000,
        "system": [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}],
        "tools": api_tools(),
        "messages": messages,
    })
}

/// Вызовы инструментов из ответа: (id, имя, аргументы).
pub fn tool_calls(v: &Value) -> Vec<(String, String, Value)> {
    v["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "tool_use")
                .map(|b| (b["id"].as_str().unwrap_or_default().to_string(), b["name"].as_str().unwrap_or_default().to_string(), b["input"].clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Текст ответа без рассуждений; пусто — если только вызовы инструментов.
pub fn answer_text(v: &Value) -> String {
    v["content"].as_array().map(|blocks| blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n\n")).unwrap_or_default().trim().to_string()
}

// ——— Проверка отчёта и сводка месяца ———

const REVIEW_SYSTEM: &str = "Ты проверяешь данные приложения «Карман» и коротко объясняешь человеку, что важно. Пиши по-русски, просто и конкретно, с числами из данных. Не придумывай того, чего нет в данных, и не давай финансовых советов сверх очевидного.";

/// Схема находок: итог одной фразой и список пунктов с важностью.
pub fn findings_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "items"],
        "properties": {
            "summary": {"type": "string", "description": "Итог одной-двумя фразами"},
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["level", "text"],
                    "properties": {
                        "level": {"type": "string", "enum": ["problem", "check", "fine"]},
                        "text": {"type": "string", "description": "Пункт: что не так или что проверить и что сделать"}
                    }
                }
            }
        }
    })
}

/// Проверка авансового отчёта поездки перед сдачей: данные поездки и находки «Кармана».
pub fn trip_check_request(trip: &Value, checks: &[String]) -> Value {
    let text = format!(
        "Проверь авансовый отчёт командировки перед сдачей в бухгалтерию. Найди, чего не хватает (документы, чеки, отчёт по статьям), где расход больше заявки, что с суточными и долгом перед компанией, нет ли ошибок в датах и суммах. level: problem — исправить до сдачи, check — стоит проверить, fine — в порядке. Не больше 8 пунктов.\n\nЧто уже нашёл «Карман»:\n{}\n\nДанные поездки (JSON):\n{}",
        if checks.is_empty() { "—".to_string() } else { checks.iter().map(|c| format!("- {c}")).collect::<Vec<_>>().join("\n") },
        trip
    );
    structured(MODEL_CHAT, 6000, REVIEW_SYSTEM, vec![json!({"type": "text", "text": text})], findings_schema())
}

/// Сводка месяца: цифры месяца, подписки, необычные траты и прогноз — посчитаны «Карманом».
pub fn month_request(facts: &Value) -> Value {
    let text = format!(
        "Сделай сводку месяца по личным деньгам человека: что главное, куда ушло больше обычного, какие подписки и регулярные траты стоит проверить, что ждать к концу месяца по прогнозу. level: problem — требует внимания, check — стоит посмотреть, fine — хорошо. Не больше 7 пунктов. Цифры — только из данных.\n\nДанные (JSON):\n{}",
        facts
    );
    structured(MODEL_CHAT, 6000, REVIEW_SYSTEM, vec![json!({"type": "text", "text": text})], findings_schema())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Findings {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub items: Vec<Finding>,
}

pub fn parse_findings(text: &str) -> Result<Findings, String> {
    let mut f: Findings = json_part(text)?;
    f.summary = clean(&f.summary, 600);
    f.items.retain(|i| !i.text.trim().is_empty());
    f.items.truncate(10);
    for i in &mut f.items {
        i.text = clean(&i.text, 500);
        if !["problem", "check", "fine"].contains(&i.level.as_str()) {
            i.level = "check".into();
        }
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overtime::{day_hours, parse_time, Hours};

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn m(date: &str, time: &str) -> Moment {
        Moment { date: date.into(), time: time.into() }
    }

    fn adm(start: (&str, &str), end: (&str, &str)) -> RawAdmission {
        RawAdmission { place: "ЗРУ-6кВ".into(), start: m(start.0, start.1), end: m(end.0, end.1) }
    }

    /// Наряд № 4/30 (Андреевка, 6–12 октября 2026): допуск по дням и ввод двоих в бригаду 9 октября.
    fn sample() -> RawOrder {
        RawOrder {
            is_order: true,
            number: "4/30".into(),
            place: "НПС «Андреевка-ПП»".into(),
            work: "Замена устройств РЗА в шкафах №1–10".into(),
            object: "Андреевка. Замена устройств РЗА 1СШ 6 кВ".into(),
            start: m("2026-10-06", "09:30"),
            end: m("2026-10-12", "18:00"),
            extended: m("", ""),
            members: vec!["Митрофанов П.А.".into(), "Фирсов К.С.".into(), "Иванов Д.Н.".into()],
            admissions: vec![
                adm(("2026-10-06", "10:34"), ("2026-10-06", "23:50")),
                adm(("2026-10-07", "09:10"), ("2026-10-07", "23:50")),
                adm(("2026-10-08", "13:00"), ("2026-10-08", "23:50")),
                adm(("2026-10-09", "08:55"), ("2026-10-09", "23:50")),
                adm(("2026-10-10", "08:45"), ("2026-10-10", "23:50")),
                adm(("2026-10-11", "08:45"), ("2026-10-11", "23:50")),
                adm(("2026-10-12", "09:40"), ("2026-10-12", "16:00")),
            ],
            changes: vec![RawChange { added: vec!["Образцов С.В.".into(), "Петров М.А.".into()], removed: vec![], at: m("2026-10-09", "16:20") }],
            problems: String::new(),
        }
    }

    fn hours(p: &DayPlan) -> Hours {
        day_hours(calendar::is_day_off(p.date), parse_time(&p.start).unwrap(), parse_time(&p.end).unwrap(), &Rules::DEFAULT)
    }

    /// Введён в бригаду 9 октября: дни с 9 по 12 — как в таблице переработок за октябрь.
    #[test]
    fn added_to_brigade_like_the_table() {
        let s = plan(&sample(), "Образцов С. В.", d("2026-10-09"), d("2026-10-15"), &Rules::DEFAULT);
        assert!(s.me.known && s.me.found && !s.me.from_start);
        assert_eq!((s.me.added, s.me.added_time.as_str(), s.me.name.as_str()), (Some(d("2026-10-09")), "16:20", "Образцов С.В."));
        let taken: Vec<(String, &str, &str, Rule)> = s.days.iter().filter(|x| x.take).map(|x| (x.date.to_string(), x.start.as_str(), x.end.as_str(), x.rule)).collect();
        assert_eq!(
            taken,
            [
                ("2026-10-09".to_string(), "8:00", "23:50", Rule::Weekday),
                ("2026-10-10".to_string(), "8:45", "23:50", Rule::Off),
                ("2026-10-11".to_string(), "8:45", "23:50", Rule::Off),
                ("2026-10-12".to_string(), "8:00", "17:00", Rule::Weekday),
            ]
        );
        let skipped: Vec<(String, Rule)> = s.days.iter().filter(|x| !x.take).map(|x| (x.date.to_string(), x.rule)).collect();
        assert_eq!(skipped, [("2026-10-06".to_string(), Rule::Before), ("2026-10-07".to_string(), Rule::Before), ("2026-10-08".to_string(), Rule::Before)]);
        // Часы — как в таблице: 2 и 5 в пятницу, по 14 в выходные, в понедельник переработки нет.
        let h: Vec<Hours> = s.days.iter().filter(|x| x.take).map(hours).collect();
        assert_eq!(h, [Hours { x15: 120, x2: 300, off: 0 }, Hours { off: 840, ..Hours::default() }, Hours { off: 840, ..Hours::default() }, Hours::default()]);
        assert_eq!((s.number.as_str(), s.from, s.to), ("4/30", Some(d("2026-10-06")), Some(d("2026-10-12"))));
        assert_eq!(s.brigade, "Митрофанов П.А., Фирсов К.С., Иванов Д.Н.; 09.10.2026 16:20 введены: Образцов С.В., Петров М.А.");
        assert_eq!(s.days[0].order_start, "10:34");
    }

    /// В бригаде с выдачи: все дни наряда, как во второй таблице (с 6 октября).
    #[test]
    fn whole_order_for_a_member() {
        let s = plan(&sample(), "Фирсов Константин Сергеевич", d("2026-10-05"), d("2026-10-16"), &Rules::DEFAULT);
        assert!(s.me.found && s.me.from_start && s.me.added.is_none());
        let times: Vec<(&str, &str)> = s.days.iter().map(|x| (x.start.as_str(), x.end.as_str())).collect();
        assert_eq!(times, [("8:00", "23:50"), ("8:00", "23:50"), ("8:00", "23:50"), ("8:00", "23:50"), ("8:45", "23:50"), ("8:45", "23:50"), ("8:00", "17:00")]);
        assert!(s.days.iter().all(|x| x.take));
    }

    #[test]
    fn unknown_or_absent_worker_keeps_all_days_inside_the_trip() {
        let s = plan(&sample(), "", d("2026-10-09"), d("2026-10-15"), &Rules::DEFAULT);
        assert!(!s.me.known && !s.me.found);
        assert_eq!(s.days.iter().filter(|x| x.take).count(), 4, "без ФИО — все дни поездки");
        assert!(s.days.iter().filter(|x| !x.take).all(|x| x.rule == Rule::Outside));
        let s = plan(&sample(), "Сидоров А.А.", d("2026-10-01"), d("2026-10-31"), &Rules::DEFAULT);
        assert!(s.me.known && !s.me.found);
        assert!(s.days.iter().all(|x| x.take), "не нашёлся в наряде — решает человек");
    }

    /// Вечерний допуск в будни, выходной за полночь и работа после полуночи в будни — как в таблице.
    #[test]
    fn evening_night_and_weekend_shifts() {
        let mut o = sample();
        o.changes.clear();
        o.admissions = vec![
            adm(("2026-10-13", "08:20"), ("2026-10-14", "01:10")),
            adm(("2026-10-24", "15:00"), ("2026-10-25", "01:00")),
            adm(("2026-10-29", "21:45"), ("2026-10-30", "02:15")),
            // Окончание без даты, но меньше начала — следующий день.
            adm(("2026-10-27", "08:00"), ("", "01:30")),
            // Две строки за день — от первого допуска до последнего окончания.
            adm(("2026-10-28", "09:00"), ("2026-10-28", "12:00")),
            adm(("2026-10-28", "13:30"), ("2026-10-28", "20:00")),
        ];
        let s = plan(&o, "Иванов Д. Н.", d("2026-10-01"), d("2026-10-31"), &Rules::DEFAULT);
        let got: Vec<(String, &str, &str, Rule)> = s.days.iter().map(|x| (x.date.to_string(), x.start.as_str(), x.end.as_str(), x.rule)).collect();
        assert_eq!(
            got,
            [
                ("2026-10-13".to_string(), "8:00", "1:10", Rule::Weekday),
                ("2026-10-24".to_string(), "15:00", "1:00", Rule::Off),
                ("2026-10-27".to_string(), "8:00", "1:30", Rule::Weekday),
                ("2026-10-28".to_string(), "8:00", "20:00", Rule::Weekday),
                ("2026-10-29".to_string(), "21:45", "2:15", Rule::Evening),
            ]
        );
        let h: Vec<Hours> = s.days.iter().map(hours).collect();
        assert_eq!(h[0], Hours { x15: 120, x2: 360, off: 0 }, "13.10: 2 и 6");
        assert_eq!(h[1], Hours { off: 600, ..Hours::default() }, "24.10: 10");
        assert_eq!(h[2], Hours { x15: 120, x2: 390, off: 0 }, "27.10 до 1:30: 2 и 6,5");
        assert_eq!(h[4], Hours { x15: 0, x2: 270, off: 0 }, "29.10: 4,5");
        assert_eq!(s.days[3].order_start, "9:00");
    }

    #[test]
    fn removed_from_brigade_and_missing_end() {
        let mut o = sample();
        o.changes = vec![RawChange { added: vec!["Петров А.Б.".into()], removed: vec!["Фирсов К.С.".into()], at: m("2026-10-10", "12:00") }];
        o.admissions[6].end = m("", "");
        let s = plan(&o, "Фирсов К.С.", d("2026-10-01"), d("2026-10-31"), &Rules::DEFAULT);
        assert_eq!((s.me.removed, s.me.removed_time.as_str()), (Some(d("2026-10-10")), "12:00"));
        let rules: Vec<(u32, Rule, bool)> = s.days.iter().map(|x| (x.date.ymd().2, x.rule, x.take)).collect();
        assert_eq!(rules[4], (10, Rule::Off, true), "день вывода — ещё в бригаде");
        assert_eq!(rules[5], (11, Rule::After, false));
        assert_eq!(rules[6], (12, Rule::After, false));
        // Окончание не прочитано: в будни — до конца рабочего дня, в выходной — не берём.
        let s = plan(&o, "Митрофанов П.А.", d("2026-10-01"), d("2026-10-31"), &Rules::DEFAULT);
        let last = s.days.last().unwrap();
        assert_eq!((last.start.as_str(), last.end.as_str(), last.rule, last.take, last.order_end.as_str()), ("8:00", "17:00", Rule::NoEnd, true, ""));
        o.admissions[5].end = m("", "");
        let s = plan(&o, "Митрофанов П.А.", d("2026-10-01"), d("2026-10-31"), &Rules::DEFAULT);
        assert_eq!((s.days[5].rule, s.days[5].take, s.days[5].start.as_str()), (Rule::NoEnd, false, ""));
    }

    #[test]
    fn own_day_start_and_end() {
        let mut r = Rules::DEFAULT;
        r.day_start = 9 * 60;
        r.day_end = 18 * 60;
        let s = plan(&sample(), "Фирсов К.С.", d("2026-10-01"), d("2026-10-31"), &r);
        assert_eq!((s.days[0].start.as_str(), s.days[6].end.as_str()), ("9:00", "18:00"));
    }

    #[test]
    fn names_dates_and_times() {
        let p = person("Образцов Сергей Викторович").unwrap();
        assert_eq!(p, Person { surname: "образцов".into(), initials: vec!['с', 'в'] });
        assert_eq!(person("С.В. Образцов").unwrap(), p);
        assert_eq!(person("Фирсовым К.С. гр.V").unwrap(), Person { surname: "фирсовым".into(), initials: vec!['к', 'с'] });
        assert!(person("  ").is_none() && person("С. В.").is_none());
        let a = person("Фирсов К.С.").unwrap();
        assert!(same_person(&a, &person("Фирсовым К.С. гр.V").unwrap()), "творительный падеж");
        assert!(same_person(&a, &person("Фирсов").unwrap()), "без инициалов");
        assert!(!same_person(&a, &person("Фирсова А.С.").unwrap()), "другие инициалы");
        assert!(!same_person(&person("Ким А.А.").unwrap(), &person("Кимов А.А.").unwrap()), "короткие фамилии — только целиком");
        assert!(same_person(&person("Семёнов И.").unwrap(), &person("Семенов И.И.").unwrap()), "ё и е");
        assert_eq!(parse_date("06.10.26"), Some(d("2026-10-06")));
        assert_eq!(parse_date("6.10.2026"), Some(d("2026-10-06")));
        assert_eq!(parse_date("2026-10-06"), Some(d("2026-10-06")));
        for bad in ["", "06.10", "32.10.26", "06.10.226", "вчера"] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
        assert_eq!([parse_clock("09:40"), parse_clock("9-40"), parse_clock("09.40"), parse_clock("0940"), parse_clock("8"), parse_clock("00:15"), parse_clock("24:00")], [Some(580), Some(580), Some(580), Some(580), Some(480), Some(15), Some(1440)]);
        for bad in ["", "25:00", "9:7", "утро"] {
            assert_eq!(parse_clock(bad), None, "{bad}");
        }
    }

    #[test]
    fn request_reply_and_errors() {
        let docs = [Doc { mime: "image/jpeg", data: b"\xFF\xD8jpeg" }, Doc { mime: "application/pdf", data: b"%PDF-1.7" }];
        let r = order_request(&docs, d("2026-10-09"), d("2026-10-15"));
        assert_eq!((r["model"].as_str(), r["max_tokens"].as_u64()), (Some(MODEL_DOCS), Some(16_000)));
        let c = r["messages"][0]["content"].as_array().unwrap();
        assert_eq!((c[0]["type"].as_str(), c[0]["source"]["media_type"].as_str(), c[0]["source"]["data"].as_str()), (Some("image"), Some("image/jpeg"), Some("/9hqcGVn")));
        assert_eq!((c[1]["type"].as_str(), c[1]["source"]["media_type"].as_str()), (Some("document"), Some("application/pdf")));
        assert!(c[2]["text"].as_str().unwrap().contains("с 09.10.2026 по 15.10.2026"), "задание — после листов");
        assert!(r.get("temperature").is_none() && r.get("tool_choice").is_none(), "версия API 2026-10-21 и Opus 5.5 их не принимают");
        let f = &r["output_config"]["format"];
        assert_eq!(f["type"], "json_schema");
        // Каждый объект схемы закрыт и все его поля обязательны — так требует Claude API.
        fn closed(v: &Value) -> bool {
            match v {
                Value::Object(o) => {
                    let ok = if o.get("type").and_then(Value::as_str) == Some("object") {
                        let props: Vec<&String> = o["properties"].as_object().unwrap().keys().collect();
                        let req: Vec<&str> = o["required"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
                        o["additionalProperties"] == false && props.len() == req.len() && props.iter().all(|p| req.contains(&p.as_str()))
                    } else {
                        true
                    };
                    ok && o.values().all(closed)
                }
                Value::Array(a) => a.iter().all(closed),
                _ => true,
            }
        }
        assert!(closed(&f["schema"]));
        let schema = f["schema"].to_string();
        for banned in ["minLength", "maxLength", "minimum", "maximum", "pattern", "format\":"] {
            assert!(!schema.contains(banned), "{banned} в схеме не поддерживается");
        }
        // Ответ: рассуждение пропускается, JSON — из текста.
        let reply = json!({
            "type": "message", "stop_reason": "end_turn",
            "content": [{"type": "thinking", "thinking": "", "signature": "x"}, {"type": "text", "text": "{\"is_order\": true, \"number\": \"4/30\", \"place\": \"\", \"work\": \"\", \"object\": \"\", \"start\": {\"date\": \"\", \"time\": \"\"}, \"end\": {\"date\": \"\", \"time\": \"\"}, \"extended\": {\"date\": \"\", \"time\": \"\"}, \"members\": [], \"admissions\": [], \"changes\": [], \"problems\": \"\"}"}],
            "usage": {"input_tokens": 18_000, "output_tokens": 2_500, "cache_read_input_tokens": 0}
        });
        let raw = parse_order(&reply_text(&reply).unwrap()).unwrap();
        assert_eq!(raw.number, "4/30");
        assert_eq!(usage(&reply), Usage { input: 18_000, output: 2_500, ..Usage::default() });
        let cached = Usage { input: 1_000, output: 500, cache_write: 8_000, cache_read: 0 };
        assert!((cost_usd(MODEL_CHAT, cached) - (1_000.0 * 2.0 + 8_000.0 * 2.5 + 500.0 * 10.0) / 1e6).abs() < 1e-12);
        let read = Usage { input: 1_000, output: 500, cache_write: 0, cache_read: 8_000 };
        assert!((cost_usd(MODEL_CHAT, read) - (1_000.0 * 2.0 + 8_000.0 * 0.1 + 500.0 * 10.0) / 1e6).abs() < 1e-12, "чтение кэша — 5 % цены входа");
        assert!((cost_usd(MODEL_DOCS, usage(&reply)) - 0.122).abs() < 1e-9);
        assert!(parse_order("```json\n{\"number\": \"7\"}\n```").is_ok_and(|o| o.number == "7" && o.is_order));
        assert!(parse_order("не JSON").is_err());
        assert!(reply_text(&json!({"stop_reason": "refusal", "content": []})).unwrap_err().contains("отказался"));
        assert!(reply_text(&json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "{"}]})).unwrap_err().contains("не уместился"));
        assert!(reply_text(&json!({"stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": "…"}]})).is_err());
        // Ошибки API — по-русски.
        let e = |status: u16, kind: &str, msg: &str| api_error(status, &json!({"type": "error", "error": {"type": kind, "message": msg}}).to_string());
        assert!(e(401, "authentication_error", "invalid x-api-key").starts_with("Ключ API не подходит"));
        assert!(e(400, "invalid_request_error", "Your credit balance is too low to access the Claude API.").contains("не хватает средств"));
        assert!(e(400, "invalid_request_error", "This API key is not scoped to a workspace, so this request must include the anthropic-workspace-id header").contains("wrkspc_"));
        assert!(e(404, "not_found_error", "Workspace `wrkspc_x` not found.").contains("пространство"));
        assert!(e(429, "rate_limit_error", "").contains("повторите позже"));
        assert!(e(529, "overloaded_error", "Overloaded").contains("перегружен"));
        assert!(e(403, "permission_error", "Request not allowed").contains("(Request not allowed)"));
        assert!(api_error(502, "<html>").contains("на стороне Claude API"));
        // Ключ и пространство.
        assert!(check_key("sk-ant-api03-abcdefghijklmnopqrstuvwxyz").is_ok());
        assert!(check_key("sk-proj-123").is_err() && check_key("sk-ant-api03 abc def ghi jkl mno pqr").is_err() && check_key("sk-ant-1").is_err());
        assert_eq!(key_tail("sk-ant-api03-abcdefghijklmnopqrstuvwxyzA1b2"), "sk-ant-…A1b2");
        assert!(check_workspace("").is_ok() && check_workspace("wrkspc_01JwQvzr7rXLA5AGx3HKfFUJ").is_ok() && check_workspace("ws-1").is_err());
        assert!(doc_supported("image/jpeg") && doc_supported("application/pdf") && !doc_supported("image/heic"));
    }

    /// Все схемы ответа закрыты и без неподдерживаемых ключевых слов.
    fn closed(v: &Value) -> bool {
        match v {
            Value::Object(o) => {
                let ok = if o.get("type").and_then(Value::as_str) == Some("object") {
                    let props: Vec<&String> = o["properties"].as_object().unwrap().keys().collect();
                    let req: Vec<&str> = o["required"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
                    o["additionalProperties"] == false && props.len() == req.len() && props.iter().all(|p| req.contains(&p.as_str()))
                } else {
                    true
                };
                ok && o.values().all(closed)
            }
            Value::Array(a) => a.iter().all(closed),
            _ => true,
        }
    }

    #[test]
    fn every_schema_is_closed() {
        for (name, s) in [("phrase", phrase_schema()), ("expense", expense_schema()), ("trip", trip_schema()), ("findings", findings_schema()), ("order", order_schema())] {
            assert!(closed(&s), "{name}");
            let t = s.to_string();
            for banned in ["minLength", "maxLength", "minimum", "maximum", "pattern", "format\":"] {
                assert!(!t.contains(banned), "{name}: {banned}");
            }
        }
    }

    fn demo_snapshot() -> crate::book::Snapshot {
        let dir = tempfile::tempdir().unwrap();
        let mut st = crate::db::Store::open(&dir.path().join("t.db"), &[7u8; 32], "dev").unwrap();
        crate::book::ensure_defaults(&mut st).unwrap();
        st.append_all(crate::demo::book_events()).unwrap();
        crate::book::snapshot(&st, crate::demo::today()).unwrap()
    }

    #[test]
    fn phrase_becomes_an_entry_draft_by_the_entry_rules() {
        let s = demo_snapshot();
        let active = crate::tools::active_trip(&s.trips.iter().map(|t| t.trip.clone()).collect::<Vec<_>>(), s.today).map(|t| t.id.clone()).expect("в демо идёт поездка");
        let raw = |v: Value| -> RawPhrase { serde_json::from_value(v).unwrap() };
        // Такси в поездке с чеком — на рабочую карту, с документом.
        let d = phrase_draft(&raw(json!({"kind": "expense", "amount": 640, "scope": "trip", "trip": "", "category": "taxi", "account": "", "date": "", "title": "Такси до объекта", "has_document": true})), &s);
        assert_eq!((d.dir, d.amount, d.scope.clone(), d.cat.as_str(), d.document, d.date), ("out", 640.0, format!("trip:{active}"), "taxi", true, s.today));
        assert_eq!(s.accounts.iter().find(|a| a.account.id == d.acc).unwrap().account.kind, "work");
        // Неизвестная категория и чужой счёт — по умолчанию; питание без документа.
        let d = phrase_draft(&raw(json!({"kind": "expense", "amount": 450, "scope": "trip", "category": "food", "account": "нет-такого", "has_document": true, "title": "Обед"})), &s);
        assert!(!d.document && d.cat == "food", "питание — из суточных, без чека");
        let d = phrase_draft(&raw(json!({"kind": "expense", "amount": 1200.456, "scope": "personal", "category": "spaceships", "date": "2026-11-05", "title": "Продукты"})), &s);
        assert_eq!((d.scope.as_str(), d.cat.as_str(), d.amount, d.date.to_string()), ("own", "groceries", 1200.46, "2026-11-05".to_string()));
        assert_eq!(s.accounts.iter().find(|a| a.account.id == d.acc).unwrap().account.kind, "personal");
        // Зарплата — личный доход; аванс, отнесённый к личному, — в поездку.
        let d = phrase_draft(&raw(json!({"kind": "income", "amount": 95000, "scope": "personal", "category": "salary"})), &s);
        assert_eq!((d.dir, d.scope.as_str(), d.cat.as_str()), ("in", "own", "salary"));
        let d = phrase_draft(&raw(json!({"kind": "income", "amount": 30000, "scope": "personal", "category": "advance"})), &s);
        assert_eq!((d.scope.clone(), d.cat.as_str()), (format!("trip:{active}"), "advance"));
        // Семья и метка поездки; перевод — между двумя разными счетами.
        assert_eq!(phrase_draft(&raw(json!({"kind": "expense", "amount": 3000, "scope": "family", "category": "groceries"})), &s).scope, "family");
        assert_eq!(phrase_draft(&raw(json!({"kind": "expense", "amount": 900, "scope": "personal_tagged", "category": "gifts"})), &s).scope, format!("tag:{active}"));
        let cash = s.accounts.iter().find(|a| a.account.kind == "cash").unwrap().account.id.clone();
        let card = s.accounts.iter().find(|a| a.account.kind == "personal").unwrap().account.id.clone();
        let d = phrase_draft(&raw(json!({"kind": "transfer", "amount": 5000, "account": card, "to_account": cash, "category": "x"})), &s);
        assert_eq!((d.dir, d.cat.as_str(), d.acc.clone(), d.to.clone()), ("move", "transfer", card.clone(), cash));
        assert_eq!(phrase_draft(&raw(json!({"kind": "transfer", "amount": 5000, "account": card, "to_account": card})), &s).to, "", "куда — не тот же счёт");
        // Названа валюта — счёт в этой валюте, если он есть.
        if let Some(kzt) = s.accounts.iter().find(|a| a.account.currency == "KZT") {
            let d = phrase_draft(&raw(json!({"kind": "expense", "amount": 2500, "currency": "kzt", "scope": "personal", "category": "cafe"})), &s);
            assert_eq!(d.acc, kzt.account.id);
        }
        // Мусор в сумме и дате — ноль и сегодня.
        let d = phrase_draft(&raw(json!({"kind": "expense", "amount": -5, "date": "когда-то", "scope": "personal", "category": "cafe"})), &s);
        assert_eq!((d.amount, d.date), (0.0, s.today));
        let r = phrase_request("такси 640 до объекта, чек есть", s.today, &json!({"accounts": []}));
        assert_eq!(r["model"], MODEL_SHORT);
        assert!(r["messages"][0]["content"][0]["text"].as_str().unwrap().contains("«такси 640 до объекта, чек есть»"));
        assert!(parse_phrase("{\"kind\": \"expense\", \"amount\": 1}").is_ok() && parse_phrase("не JSON").is_err());
    }

    #[test]
    fn tickets_hotels_and_trip_documents() {
        let raw = |v: Value| -> RawExpense { serde_json::from_value(v).unwrap() };
        let t = expense_draft(&raw(json!({"kind": "ticket", "category": "taxi", "amount": 12850.0, "currency": "rub", "date": "2026-10-01", "route": "Уфа — Москва", "travel_date": "2026-10-06", "number": "555 1234567890"})));
        assert_eq!((t.cat.as_str(), t.amount, t.currency.as_str(), t.comment.as_str(), t.date.unwrap().to_string()), ("tickets", 12850.0, "RUB", "Билет Уфа — Москва, 06.10", "2026-10-01".to_string()));
        let h = expense_draft(&raw(json!({"kind": "hotel", "category": "lodging", "amount": 21000, "date": "12.10.2026", "seller": "Гостиница «Башкирия»", "stay_from": "2026-10-06", "stay_to": "2026-10-12", "nights": 6})));
        assert_eq!((h.cat.as_str(), h.comment.as_str()), ("lodging", "Проживание: Гостиница «Башкирия», 06.10–12.10, 6 ночей"));
        let r = expense_draft(&raw(json!({"kind": "receipt", "category": "tools", "amount": 1530.5, "title": "Изолента, хомуты"})));
        assert_eq!((r.cat.as_str(), r.comment.as_str(), r.not_document), ("tools", "Изолента, хомуты", false));
        assert!(expense_draft(&raw(json!({"kind": "other"}))).not_document);
        assert_eq!(expense_draft(&raw(json!({"kind": "receipt", "category": "food??", "amount": 300}))).cat, "taxi");
        assert_eq!(nights_word(1), "ночь");
        let raw = |v: Value| -> RawTripDoc { serde_json::from_value(v).unwrap() };
        let d = trip_draft(&raw(json!({"doc_kind": "request", "request_no": "КМ-000573", "request_date": "2026-10-15", "city": "г. Чекмагуш", "region": "Республика Башкортостан", "start": "2026-10-31", "end": "2026-10-21", "goal": "Наладка РЗА", "tickets": 15000, "lodging": 21000.5, "per_diem": 7700, "transport": -3})));
        assert_eq!((d.city.as_str(), d.req_no.as_str(), d.start.unwrap().to_string(), d.end.unwrap().to_string()), ("Чекмагуш", "КМ-000573", "2026-10-21".to_string(), "2026-10-31".to_string()));
        assert_eq!(d.plan, [1_500_000, 0, 2_100_050, 770_000, 0]);
        assert!(trip_draft(&raw(json!({"doc_kind": "other"}))).not_document);
        let docs = [Doc { mime: "application/pdf", data: b"%PDF" }];
        let q = trip_request(&docs, Date::parse("2026-11-10").unwrap());
        assert_eq!((q["model"].as_str(), q["messages"][0]["content"][0]["type"].as_str()), (Some(MODEL_DOCS), Some("document")));
        let q = expense_request(&docs, Date::parse("2026-11-10").unwrap(), Some((Date::parse("2026-10-21").unwrap(), Date::parse("2026-10-31").unwrap())));
        assert!(q["messages"][0]["content"][1]["text"].as_str().unwrap().contains("с 21.10.2026 по 31.10.2026"));
    }

    #[test]
    fn chat_and_review_requests() {
        let today = Date::parse("2026-11-10").unwrap();
        let sys = chat_system(today);
        assert!(sys.contains("2026-11-10 (вторник)") && sys.contains("order_days"));
        let r = chat_request(&sys, &[json!({"role": "user", "content": "Сколько у меня денег?"})]);
        assert_eq!(r["model"], MODEL_CHAT);
        assert_eq!((r["system"][0]["text"].as_str(), r["system"][0]["cache_control"]["type"].as_str()), (Some(sys.as_str()), Some("ephemeral")));
        let tools = r["tools"].as_array().unwrap();
        assert_eq!(tools.len(), crate::tools::list().len(), "все инструменты — и чтение, и запись: список не меняется от настроек");
        assert!(tools.iter().all(|t| t["input_schema"]["type"] == "object" && t.get("strict").is_none()));
        let reply = json!({"content": [{"type": "thinking", "thinking": "", "signature": "s"}, {"type": "text", "text": "Сейчас посмотрю."}, {"type": "tool_use", "id": "tu_1", "name": "summary", "input": {}}], "stop_reason": "tool_use"});
        assert_eq!(tool_calls(&reply), vec![("tu_1".to_string(), "summary".to_string(), json!({}))]);
        assert_eq!(answer_text(&reply), "Сейчас посмотрю.");
        let q = trip_check_request(&json!({"id": "573"}), &["Нет документа: такси 640 ₽".to_string()]);
        assert!(q["messages"][0]["content"][0]["text"].as_str().unwrap().contains("- Нет документа: такси 640 ₽"));
        assert_eq!(month_request(&json!({"spent": 1}))["model"], MODEL_CHAT);
        let f = parse_findings("{\"summary\": \" Всё  хорошо \", \"items\": [{\"level\": \"warning\", \"text\": \"Чек\"}, {\"level\": \"fine\", \"text\": \"  \"}]}").unwrap();
        assert_eq!((f.summary.as_str(), f.items.len(), f.items[0].level.as_str()), ("Всё хорошо", 1, "check"));
    }
}
