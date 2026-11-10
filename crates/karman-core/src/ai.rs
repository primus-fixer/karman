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
/// Короткие задачи: проверка ключа.
pub const MODEL_SHORT: &str = "claude-haiku-5-5";
/// Ответ с рассуждением модели и JSON наряда умещается с запасом.
const MAX_TOKENS: u32 = 16_000;

/// Названия моделей для экрана.
pub fn model_title(model: &str) -> &'static str {
    match model {
        MODEL_DOCS => "Claude Opus 5.5",
        MODEL_SHORT => "Claude Haiku 5.5",
        _ => "Claude",
    }
}

/// Цена за миллион токенов в долларах США: (вход, выход) — по прайсу Anthropic на ноябрь 2026.
pub fn price(model: &str) -> (f64, f64) {
    match model {
        MODEL_SHORT => (0.10, 0.50),
        "claude-sonnet-5-5" => (2.0, 10.0),
        _ => (4.0, 20.0),
    }
}

/// Токены одного запроса.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
}

/// Оценка стоимости запроса, в долларах США.
pub fn cost_usd(model: &str, u: Usage) -> f64 {
    let (i, o) = price(model);
    (u.input as f64 * i + u.output as f64 * o) / 1_000_000.0
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

/// Запрос распознавания наряда: сначала листы, потом задание. Поездка нужна для года и проверки дат.
pub fn order_request(docs: &[Doc], trip_from: Date, trip_to: Date) -> Value {
    let mut content: Vec<Value> = docs
        .iter()
        .map(|d| {
            let data = B64.encode(d.data);
            if d.mime == "application/pdf" {
                json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": data}})
            } else {
                json!({"type": "image", "source": {"type": "base64", "media_type": d.mime, "data": data}})
            }
        })
        .collect();
    let sheets = match docs.len() {
        1 => "Один файл".to_string(),
        n => format!("Файлов: {n} — это листы наряда по порядку"),
    };
    content.push(json!({
        "type": "text",
        "text": format!("{sheets}. Командировка, в которой выдан наряд: с {} по {}. Перепиши наряд по схеме.", trip_from.dotted(), trip_to.dotted())
    }));
    json!({
        "model": MODEL_DOCS,
        "max_tokens": MAX_TOKENS,
        "system": ORDER_SYSTEM,
        "messages": [{"role": "user", "content": content}],
        "output_config": {"format": {"type": "json_schema", "schema": order_schema()}},
    })
}

/// Токены из ответа Claude API.
pub fn usage(v: &Value) -> Usage {
    let u = &v["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    Usage { input: n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens"), output: n("output_tokens") }
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

/// Фамилия и инициалы: «Просолов С.В.», «Просолов Семён Викторович», «С.В. Просолов».
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
        let p = person("Просолов Семён Викторович").unwrap();
        assert_eq!(p, Person { surname: "просолов".into(), initials: vec!['с', 'в'] });
        assert_eq!(person("С.В. Просолов").unwrap(), p);
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
        assert_eq!(usage(&reply), Usage { input: 18_000, output: 2_500 });
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
}
