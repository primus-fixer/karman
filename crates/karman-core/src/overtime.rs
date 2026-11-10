//! Переработки в поездке: наряды, рабочие дни и часы с коэффициентами — как в образцах таблиц
//! переработок:
//! - будни с 17:00 до 19:00 — коэффициент 1,5, после 19:00 — коэффициент 2;
//! - выходные и праздники — вся смена без часа на обед (если смена захватывает 12–13), коэффициент 2;
//! - часы округляются до получаса;
//! - смена, которая кончается после полуночи, считается днём начала.
//!
//! Конец рабочего дня, обед и округление — настройки. Сумма к оплате считается
//! от оклада и премии по норме часов месяца и показывается только на экране.

use crate::calendar;
use crate::date::Date;
use crate::error::{Error, Result};
use crate::money::Kop;
use crate::trip::{Attachment, Trip};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Наряд-допуск: по нему видно, где и когда работали. Снимки или PDF наряда идут в отчёт.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkOrder {
    pub id: String,
    pub trip: String,
    /// Номер наряда: «4/30».
    #[serde(default)]
    pub number: String,
    /// Объект и работы: «Андреевка. Замена РЗиА секции».
    #[serde(default)]
    pub object: String,
    pub from: Date,
    pub to: Date,
    #[serde(default)]
    pub att: Vec<Attachment>,
    #[serde(default)]
    pub note: String,
    /// Поля из более новых версий.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Рабочий день поездки: время начала и конца работ и что делали.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkDay {
    /// «номер поездки:дата» — один день поездки, одна запись.
    pub id: String,
    pub trip: String,
    pub date: Date,
    /// «8:00»; пусто — времени нет.
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
    /// В таблице — «Комментарии».
    #[serde(default)]
    pub comment: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn day_id(trip: &str, date: Date) -> String {
    format!("{trip}:{date}")
}

/// Правила подсчёта: все величины — в минутах.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Rules {
    /// Начало рабочего дня: с него в будни пишется время работы по наряду.
    pub day_start: u32,
    /// Конец рабочего дня: с него в будни идут переработки.
    pub day_end: u32,
    /// Сколько первых часов переработки в будни — с коэффициентом 1,5.
    pub first: u32,
    /// Обед в выходные, если смена захватывает время с 12 до 13.
    pub lunch: u32,
    /// Округление.
    pub step: u32,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules::DEFAULT
    }
}

impl Rules {
    pub const DEFAULT: Rules = Rules { day_start: 8 * 60, day_end: 17 * 60, first: 120, lunch: 60, step: 30 };

    /// Из настроек: «ot.day_start» (8:00), «ot.day_end» (17:00), «ot.lunch» (минуты), «ot.round» (минуты).
    pub fn from_settings(s: &BTreeMap<String, String>) -> Rules {
        let mut r = Rules::DEFAULT;
        if let Some(t) = s.get("ot.day_start").and_then(|v| parse_time(v)).filter(|t| (5 * 60..=12 * 60).contains(t)) {
            r.day_start = t;
        }
        if let Some(t) = s.get("ot.day_end").and_then(|v| parse_time(v)).filter(|t| (12 * 60..=22 * 60).contains(t)) {
            r.day_end = t;
        }
        if let Some(m) = s.get("ot.lunch").and_then(|v| v.parse::<u32>().ok()).filter(|m| *m <= 120) {
            r.lunch = m;
        }
        if let Some(m) = s.get("ot.round").and_then(|v| v.parse::<u32>().ok()).filter(|m| [1, 15, 30, 60].contains(m)) {
            r.step = m;
        }
        r
    }
}

/// «8:00», «08:00», «23:50» → минуты от полуночи.
pub fn parse_time(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Минуты от полуночи → «8:00» (как в таблице).
pub fn time_text(min: u32) -> String {
    format!("{}:{:02}", min / 60 % 24, min % 60)
}

/// Часы переработки за день в минутах, уже округлённые.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Hours {
    /// Будни, с конца рабочего дня два часа: коэффициент 1,5.
    pub x15: u32,
    /// Будни, дальше: коэффициент 2.
    pub x2: u32,
    /// Выходные и праздники: коэффициент 2.
    pub off: u32,
}

/// Округление до шага, половина — вверх (как ОКРУГЛ в Excel).
fn round_to(m: i64, step: u32) -> u32 {
    if m <= 0 {
        return 0;
    }
    let s = step.max(1) as i64;
    ((m + s / 2) / s * s) as u32
}

/// Часы за день: `off` — выходной или праздник; конец не позже начала — смена через полночь.
pub fn day_hours(off: bool, start: u32, end: u32, r: &Rules) -> Hours {
    let s = start as i64;
    let mut e = end as i64;
    if e <= s {
        e += 24 * 60;
    }
    if off {
        let lunch = if s <= 12 * 60 && e >= 13 * 60 { r.lunch as i64 } else { 0 };
        return Hours { off: round_to(e - s - lunch, r.step), ..Hours::default() };
    }
    let (a, b) = (r.day_end as i64, (r.day_end + r.first) as i64);
    Hours { x15: round_to(e.min(b) - s.max(a), r.step), x2: round_to(e - s.max(b), r.step), off: 0 }
}

/// Строка таблицы — один день.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    pub date: Date,
    /// Выходной или праздник по производственному календарю.
    pub off: bool,
    pub start: String,
    pub end: String,
    pub comment: String,
    /// Поездка, к которой относится день.
    pub trip: Option<String>,
    /// Нет времени — нет и часов.
    pub hours: Option<Hours>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Sheet {
    pub rows: Vec<Row>,
    /// Всего, в минутах: будни с коэффициентом 1,5; будни после двух часов; выходные.
    pub x15: u32,
    pub x2: u32,
    pub off: u32,
}

impl Sheet {
    /// Всего с коэффициентом 2: в будни после двух часов и в выходные.
    pub fn double(&self) -> u32 {
        self.x2 + self.off
    }
}

fn times(w: &WorkDay) -> Option<(u32, u32)> {
    Some((parse_time(&w.start)?, parse_time(&w.end)?))
}

/// Таблица по дням с `from` по `to`: дни без записей — пустые строки.
pub fn build(from: Date, to: Date, days: &[&WorkDay], r: &Rules) -> Sheet {
    let mut by_date: BTreeMap<Date, Vec<&WorkDay>> = BTreeMap::new();
    for d in days {
        by_date.entry(d.date).or_default().push(d);
    }
    let mut sheet = Sheet::default();
    let mut d = from;
    // Больше года подряд таблица не бывает — защита от ошибки в датах поездки.
    let last = to.min(from.add_days(400));
    while d <= last {
        let off = calendar::is_day_off(d);
        let mut row = Row { date: d, off, start: String::new(), end: String::new(), comment: String::new(), trip: None, hours: None };
        if let Some(list) = by_date.get(&d) {
            // Записи из двух поездок за один день: время — из той, где оно есть; комментарии вместе.
            let main = list.iter().find(|w| times(w).is_some()).or(list.first());
            if let Some(w) = main {
                row.trip = Some(w.trip.clone());
                if let Some((s, e)) = times(w) {
                    let h = day_hours(off, s, e, r);
                    sheet.x15 += h.x15;
                    sheet.x2 += h.x2;
                    sheet.off += h.off;
                    row.start = time_text(s);
                    row.end = time_text(e);
                    row.hours = Some(h);
                }
            }
            let mut seen = BTreeSet::new();
            row.comment = list.iter().map(|w| w.comment.trim()).filter(|c| !c.is_empty() && seen.insert(c.to_string())).collect::<Vec<_>>().join("; ");
        }
        sheet.rows.push(row);
        d = d.add_days(1);
    }
    sheet
}

/// Таблица поездки: все её дни (и дни с записями, если даты поездки потом поменяли).
pub fn trip_sheet(t: &Trip, days: &[WorkDay], r: &Rules) -> Sheet {
    let mine: Vec<&WorkDay> = days.iter().filter(|d| d.trip == t.id).collect();
    let (a, b) = (t.start.min(t.end), t.start.max(t.end));
    let from = mine.iter().map(|d| d.date).min().map_or(a, |m| m.min(a));
    let to = mine.iter().map(|d| d.date).max().map_or(b, |m| m.max(b));
    build(from, to, &mine, r)
}

/// Общая таблица за месяц: от первого до последнего дня поездок месяца, вместе с днями между ними.
pub fn month_sheet(month: Date, trips: &[Trip], days: &[WorkDay], r: &Rules) -> Sheet {
    let (ms, me) = (month.month_start(), month.month_end());
    let mine: Vec<&WorkDay> = days.iter().filter(|d| d.date >= ms && d.date <= me).collect();
    let spans = trips.iter().filter(|t| t.start.min(t.end) <= me && t.start.max(t.end) >= ms).map(|t| (t.start.min(t.end).max(ms), t.start.max(t.end).min(me)));
    let starts = spans.clone().map(|s| s.0).chain(mine.iter().map(|d| d.date));
    let ends = spans.map(|s| s.1).chain(mine.iter().map(|d| d.date));
    match (starts.min(), ends.max()) {
        (Some(from), Some(to)) => build(from, to, &mine, r),
        _ => Sheet::default(),
    }
}

/// «16», «97,5».
pub fn hours_text(min: u32) -> String {
    let whole = min / 60;
    let rest = min % 60;
    if rest == 0 {
        whole.to_string()
    } else {
        let frac = format!("{:.2}", rest as f64 / 60.0);
        format!("{whole},{}", frac.trim_start_matches("0.").trim_end_matches('0'))
    }
}

/// «час», «часа», «часов»; дробное — «часа».
pub fn hours_word(min: u32) -> &'static str {
    if min % 60 != 0 {
        return "часа";
    }
    let n = min / 60;
    match (n % 10, n % 100) {
        (1, x) if x != 11 => "час",
        (2..=4, x) if !(12..=14).contains(&x) => "часа",
        _ => "часов",
    }
}

/// Итоговая строка таблицы — как в образцах.
pub fn summary(s: &Sheet) -> String {
    format!(
        "Работы с коэффициентом 1,5 - {} {}, Работы в выходные дни и более 2 часов с коэффициентом 2 - {} {}",
        hours_text(s.x15),
        hours_word(s.x15),
        hours_text(s.double()),
        hours_word(s.double())
    )
}

/// Сумма к оплате за месяц — только для экрана.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pay {
    /// «2026-10»
    pub month: String,
    /// Норма часов месяца, в минутах.
    pub norm: u32,
    /// Норма по производственному календарю (иначе — оценка).
    pub exact: bool,
    /// Своя норма из настроек.
    pub custom: bool,
    /// Стоимость часа: (оклад + премия) / норма.
    pub rate: Kop,
    /// Переработки этого месяца, минуты.
    pub x15: u32,
    pub x2: u32,
    /// К оплате до налога.
    pub sum: Kop,
}

fn kop_setting(s: &BTreeMap<String, String>, key: &str) -> Kop {
    s.get(key).and_then(|v| v.parse::<Kop>().ok()).filter(|v| *v > 0).unwrap_or(0)
}

/// Своя норма часов месяца из настроек «ot.norm.2026-10», в минутах.
fn custom_norm(s: &BTreeMap<String, String>, month: &str) -> Option<u32> {
    let v = s.get(&format!("ot.norm.{month}"))?.replace(',', ".");
    let h: f64 = v.trim().parse().ok()?;
    (h > 0.0 && h < 400.0).then(|| (h * 60.0).round() as u32)
}

/// Сумма за переработки по месяцам: стоимость часа — (оклад + премия) / норма часов месяца.
pub fn pay(sheet: &Sheet, s: &BTreeMap<String, String>) -> Vec<Pay> {
    let base = kop_setting(s, "ot.salary") + kop_setting(s, "ot.bonus");
    if base <= 0 {
        return Vec::new();
    }
    let mut months: BTreeMap<(i32, u32), (u32, u32)> = BTreeMap::new();
    for row in &sheet.rows {
        if let Some(h) = row.hours {
            let (y, m, _) = row.date.ymd();
            let e = months.entry((y, m)).or_default();
            e.0 += h.x15;
            e.1 += h.x2 + h.off;
        }
    }
    months
        .into_iter()
        .filter(|(_, (a, b))| a + b > 0)
        .filter_map(|((y, m), (x15, x2))| {
            let month = format!("{y}-{m:02}");
            let (cal, exact) = calendar::norm_minutes(y, m);
            let custom = custom_norm(s, &month);
            let norm = custom.unwrap_or(cal);
            if norm == 0 {
                return None;
            }
            let per_hour = base as f64 * 60.0 / norm as f64;
            let sum = (x15 as f64 / 60.0 * 1.5 + x2 as f64 / 60.0 * 2.0) * per_hour;
            Some(Pay { month, norm, exact, custom: custom.is_some(), rate: per_hour.round() as Kop, x15, x2, sum: sum.round() as Kop })
        })
        .collect()
}

/// Переработки поездки для экрана.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TripOt {
    pub orders: Vec<WorkOrder>,
    /// Имена файлов нарядов в отчёте — в том же порядке, что и наряды.
    pub names: Vec<String>,
    pub sheet: Sheet,
    pub summary: String,
    pub pay: Vec<Pay>,
    /// Правила, по которым посчитаны часы: для подписей на экране.
    pub rules: Rules,
}

pub fn trip_ot(t: &Trip, orders: &[WorkOrder], days: &[WorkDay], settings: &BTreeMap<String, String>) -> TripOt {
    let r = Rules::from_settings(settings);
    let sheet = trip_sheet(t, days, &r);
    let mut mine: Vec<WorkOrder> = orders.iter().filter(|o| o.trip == t.id).cloned().collect();
    mine.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.number.cmp(&b.number)));
    TripOt { summary: summary(&sheet), pay: pay(&sheet, settings), names: mine.iter().map(order_file_name).collect(), orders: mine, sheet, rules: r }
}

/// Имя файла наряда: «Наряд 4-30 с 06.10.2026 по 12.10.2026».
pub fn order_file_name(o: &WorkOrder) -> String {
    let num = o.number.trim().replace(['/', '\\'], "-");
    let (a, b) = (o.from.min(o.to), o.from.max(o.to));
    let dates = if a == b { format!("за {}", a.dotted()) } else { format!("с {} по {}", a.dotted(), b.dotted()) };
    let base = if num.is_empty() { format!("Наряд {dates}") } else { format!("Наряд {num} {dates}") };
    crate::zip::safe_name(&base)
}

// ——— Таблица Excel в оформлении образца ———

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").chars().map(|c| if (c as u32) < 0x20 && c != '\t' && c != '\n' { ' ' } else { c }).collect()
}

fn serial(d: Date) -> i64 {
    Date::from_epoch_days(0).days_until(d) as i64 + 25_569
}

/// Стили образца: Arimo 10, тонкие рамки, зелёная заливка выходных, «ДД.ММ.ГГГГ» и «Ч:ММ».
/// Номера стилей: 1 — заголовок по центру, 2 — заголовок «Комментарии», 3 — дата, 4 — время,
/// 5 — часы, 6 — комментарий, 7–9 — дата, время и часы выходного дня, 10 — «Итого переработок:», 11 — итог.
const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="2"><numFmt numFmtId="164" formatCode="dd\.mm\.yyyy"/><numFmt numFmtId="165" formatCode="[$-F400]h:mm"/></numFmts><fonts count="1"><font><sz val="10"/><color rgb="FF000000"/><name val="Arimo"/><family val="2"/></font></fonts><fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FF92D050"/><bgColor indexed="64"/></patternFill></fill></fills><borders count="2"><border><left/><right/><top/><bottom/><diagonal/></border><border><left style="thin"><color indexed="64"/></left><right style="thin"><color indexed="64"/></right><top style="thin"><color indexed="64"/></top><bottom style="thin"><color indexed="64"/></bottom><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="12"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0" applyBorder="1" applyAlignment="1"><alignment horizontal="center"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0" applyBorder="1"/><xf numFmtId="164" fontId="0" fillId="0" borderId="1" xfId="0" applyNumberFormat="1" applyBorder="1" applyAlignment="1"><alignment horizontal="center"/></xf><xf numFmtId="165" fontId="0" fillId="0" borderId="1" xfId="0" applyNumberFormat="1" applyBorder="1" applyAlignment="1"><alignment horizontal="center"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0" applyBorder="1" applyAlignment="1"><alignment horizontal="center" vertical="center"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="1" xfId="0" applyBorder="1"/><xf numFmtId="164" fontId="0" fillId="2" borderId="1" xfId="0" applyNumberFormat="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="center"/></xf><xf numFmtId="165" fontId="0" fillId="2" borderId="1" xfId="0" applyNumberFormat="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="center"/></xf><xf numFmtId="0" fontId="0" fillId="2" borderId="1" xfId="0" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="center" vertical="center"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0" applyAlignment="1"><alignment horizontal="center"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs><cellStyles count="1"><cellStyle name="Обычный" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#;

fn text_cell(at: &str, style: u8, t: &str) -> String {
    format!(r#"<c r="{at}" s="{style}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#, esc(t))
}

fn num_cell(at: &str, style: u8, v: &str) -> String {
    format!(r#"<c r="{at}" s="{style}"><v>{v}</v></c>"#)
}

/// Часы числом («4.5» — Excel покажет «4,5») или прочерк.
fn hours_cell(at: &str, style: u8, min: u32) -> String {
    if min == 0 {
        return text_cell(at, style, "-");
    }
    let v = if min % 60 == 0 { (min / 60).to_string() } else { format!("{}", min as f64 / 60.0) };
    num_cell(at, style, &v)
}

fn time_cell(at: &str, style: u8, t: &str) -> String {
    match parse_time(t) {
        Some(m) => num_cell(at, style, &format!("{}", m as f64 / 1440.0)),
        None => text_cell(at, style, "-"),
    }
}

fn sheet_xml(s: &Sheet, r: &Rules) -> String {
    let last = s.rows.len() + 1;
    let total = last + 2;
    let mut x = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
    x.push_str(&format!(r#"<dimension ref="B1:H{total}"/><sheetViews><sheetView tabSelected="1" workbookViewId="0"/></sheetViews><sheetFormatPr defaultColWidth="13.453125" defaultRowHeight="14.25" customHeight="1"/>"#));
    x.push_str(r#"<cols><col min="3" max="3" width="20" customWidth="1"/><col min="4" max="4" width="22.453125" customWidth="1"/><col min="5" max="5" width="25.08984375" customWidth="1"/><col min="6" max="6" width="22" customWidth="1"/><col min="7" max="7" width="17" customWidth="1"/><col min="8" max="8" width="48.90625" customWidth="1"/></cols><sheetData>"#);
    let row = |n: usize, cells: &str| format!(r#"<row r="{n}" spans="2:8" ht="14.25" customHeight="1">{cells}</row>"#);
    let (from, to) = (time_text(r.day_end), time_text(r.day_end + r.first));
    let head = [
        text_cell("B1", 1, "Дата"),
        text_cell("C1", 1, "Время начала работы"),
        text_cell("D1", 1, "Время окончания работы"),
        text_cell("E1", 1, &format!("Переработки с {from} до {to}")),
        text_cell("F1", 1, &format!("Переработки с {to}")),
        text_cell("G1", 1, "Работа в выходные"),
        text_cell("H1", 2, "Комментарии"),
    ]
    .concat();
    x.push_str(&row(1, &head));
    for (i, d) in s.rows.iter().enumerate() {
        let n = i + 2;
        let (sd, st, sh) = if d.off { (7, 8, 9) } else { (3, 4, 5) };
        let h = d.hours.unwrap_or_default();
        let mut cells = num_cell(&format!("B{n}"), sd, &serial(d.date).to_string());
        cells.push_str(&time_cell(&format!("C{n}"), st, &d.start));
        cells.push_str(&time_cell(&format!("D{n}"), st, &d.end));
        cells.push_str(&hours_cell(&format!("E{n}"), sh, h.x15));
        cells.push_str(&hours_cell(&format!("F{n}"), sh, h.x2));
        cells.push_str(&hours_cell(&format!("G{n}"), sh, h.off));
        if d.comment.is_empty() {
            cells.push_str(&format!(r#"<c r="H{n}" s="6"/>"#));
        } else {
            cells.push_str(&text_cell(&format!("H{n}"), 6, &d.comment));
        }
        x.push_str(&row(n, &cells));
    }
    let foot = [
        text_cell(&format!("B{total}"), 10, "Итого переработок:"),
        format!(r#"<c r="C{total}" s="10"/><c r="D{total}" s="10"/>"#),
        text_cell(&format!("E{total}"), 11, &summary(s)),
    ]
    .concat();
    x.push_str(&row(total, &foot));
    x.push_str(&format!(r#"</sheetData><mergeCells count="1"><mergeCell ref="B{total}:D{total}"/></mergeCells><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup paperSize="9" orientation="portrait"/></worksheet>"#));
    x
}

/// Таблица переработок Excel — как в образце (октябрь 2026).
pub fn xlsx(s: &Sheet, r: &Rules) -> std::io::Result<Vec<u8>> {
    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#;
    let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let book = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Лист1" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    crate::zip::build(&[
        ("[Content_Types].xml".into(), types.as_bytes().to_vec()),
        ("_rels/.rels".into(), root.as_bytes().to_vec()),
        ("xl/workbook.xml".into(), book.as_bytes().to_vec()),
        ("xl/_rels/workbook.xml.rels".into(), rels.as_bytes().to_vec()),
        ("xl/styles.xml".into(), STYLES.as_bytes().to_vec()),
        ("xl/worksheets/sheet1.xml".into(), sheet_xml(s, r).into_bytes()),
    ])
}

/// Файлы отчёта: таблица «<base>.xlsx» и наряды в папке «Наряды» — каждый одним PDF.
/// Снимки наряда собираются в один PDF; PDF-файлы наряда кладутся как есть (второй — с «(2)» и т. д.).
/// Возвращает файлы и что не вошло (наряд без снимков, снимок ещё не пришёл с другого устройства).
pub fn report_files(
    base: &str,
    sheet: &Sheet,
    rules: &Rules,
    orders: &[WorkOrder],
    blob: &mut dyn FnMut(&str) -> Result<Option<(String, Vec<u8>)>>,
) -> Result<(Vec<(String, Vec<u8>)>, Vec<String>)> {
    let mut files = vec![(format!("{}.xlsx", crate::zip::safe_name(base)), xlsx(sheet, rules)?)];
    let mut notes = Vec::new();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut unique = |name: String| -> String {
        let mut n = 1;
        let mut candidate = name.clone();
        while !used.insert(candidate.to_lowercase()) {
            n += 1;
            candidate = format!("{name} ({n})");
        }
        candidate
    };
    let mut sorted: Vec<&WorkOrder> = orders.iter().collect();
    sorted.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.number.cmp(&b.number)));
    for o in sorted {
        let name = order_file_name(o);
        let (mut jpegs, mut pdfs) = (Vec::new(), Vec::new());
        for a in &o.att {
            match blob(&a.id)? {
                Some((mime, data)) if mime == "application/pdf" => pdfs.push(data),
                Some((_, data)) => jpegs.push(data),
                None => notes.push(format!("{name}: снимок ещё не пришёл с другого устройства")),
            }
        }
        if jpegs.is_empty() && pdfs.is_empty() {
            if o.att.is_empty() {
                notes.push(format!("{name}: нет снимков наряда"));
            }
            continue;
        }
        let mut parts: Vec<Vec<u8>> = Vec::new();
        if !jpegs.is_empty() {
            let refs: Vec<&[u8]> = jpegs.iter().map(|j| j.as_slice()).collect();
            match crate::pdf::from_jpegs(&refs) {
                Ok(pdf) => parts.push(pdf),
                Err(e) => notes.push(format!("{name}: {e}")),
            }
        }
        parts.extend(pdfs);
        for part in parts {
            files.push((format!("Наряды/{}.pdf", unique(name.clone())), part));
        }
    }
    Ok((files, notes))
}

/// Имя отчёта: «Переработки Иванов октябрь 2026» или «Переработки 573 Чекмагуш».
pub fn report_name(who: &str, what: &str) -> String {
    let who = who.trim();
    if who.is_empty() {
        format!("Переработки {what}")
    } else {
        format!("Переработки {who} {what}")
    }
}

const MONTHS: [&str; 12] = ["январь", "февраль", "март", "апрель", "май", "июнь", "июль", "август", "сентябрь", "октябрь", "ноябрь", "декабрь"];

/// «октябрь 2026».
pub fn month_title(d: Date) -> String {
    let (y, m, _) = d.ymd();
    format!("{} {y}", MONTHS[(m - 1) as usize])
}

/// Проверка наряда перед записью.
pub fn check_order(o: &WorkOrder) -> Result<()> {
    if o.trip.trim().is_empty() {
        return Err(Error::Format("наряд без поездки".into()));
    }
    if o.from.days_until(o.to).abs() > 366 {
        return Err(Error::Format("наряд длиннее года — проверьте даты".into()));
    }
    if o.number.chars().count() > 40 || o.object.chars().count() > 300 {
        return Err(Error::Format("слишком длинный номер или объект наряда".into()));
    }
    Ok(())
}

/// Проверка рабочего дня перед записью: время — «Ч:ММ» или пусто.
pub fn check_day(d: &WorkDay) -> Result<()> {
    for t in [&d.start, &d.end] {
        if !t.trim().is_empty() && parse_time(t).is_none() {
            return Err(Error::Format(format!("время «{t}» — в виде 8:00")));
        }
    }
    if d.start.trim().is_empty() != d.end.trim().is_empty() {
        return Err(Error::Format("укажите и начало, и окончание работы".into()));
    }
    if d.comment.chars().count() > 500 {
        return Err(Error::Format("слишком длинный комментарий".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn day(trip: &str, date: &str, start: &str, end: &str, comment: &str) -> WorkDay {
        let date = d(date);
        WorkDay { id: day_id(trip, date), trip: trip.into(), date, start: start.into(), end: end.into(), comment: comment.into(), extra: Default::default() }
    }

    /// Образец — таблица за октябрь 2026 года: время и комментарии из неё.
    fn sample() -> Vec<WorkDay> {
        let a = "Андреевка. Замена РЗиА секции";
        let c1 = "2СШ яч. №12,13,14,15";
        let c2 = "2СШ яч. №9,10,11,12,13,14,15";
        vec![
            day("561", "2026-10-09", "8:00", "23:50", a),
            day("561", "2026-10-10", "8:45", "23:50", a),
            day("561", "2026-10-11", "8:45", "23:50", a),
            day("561", "2026-10-12", "8:00", "17:00", a),
            day("561", "2026-10-13", "8:00", "1:10", "Утром перевоз оборудования и комплектующих с Андреевки. С обеда Чекмагуш 1СШ"),
            day("561", "2026-10-14", "8:00", "1:00", "Чекмагуш 1СШ"),
            day("561", "2026-10-15", "8:00", "3:55", "Чекмагуш 1СШ"),
            day("573", "2026-10-22", "", "", "Чекмагуш. организационные и подготовительные работы"),
            day("573", "2026-10-23", "8:00", "2:00", c1),
            day("573", "2026-10-24", "15:00", "1:00", c1),
            day("573", "2026-10-25", "9:00", "23:00", c1),
            day("573", "2026-10-26", "8:00", "23:00", c2),
            day("573", "2026-10-27", "8:00", "23:00", c2),
            day("573", "2026-10-28", "8:00", "20:00", c2),
            day("573", "2026-10-29", "21:45", "2:15", c2),
        ]
    }

    fn trip(id: &str, start: &str, end: &str) -> Trip {
        let mut t = crate::demo::trips().into_iter().next().unwrap();
        t.id = id.into();
        t.start = d(start);
        t.end = d(end);
        t
    }

    /// Часы по дням ровно как в образце: (будни 17–19, будни после 19, выходные).
    #[test]
    fn reproduces_the_sample_table() {
        let r = Rules::DEFAULT;
        let trips = [trip("561", "2026-10-09", "2026-10-15"), trip("573", "2026-10-22", "2026-10-29")];
        let s = month_sheet(d("2026-10-01"), &trips, &sample(), &r);
        assert_eq!(s.rows.first().unwrap().date, d("2026-10-09"));
        assert_eq!(s.rows.last().unwrap().date, d("2026-10-29"));
        assert_eq!(s.rows.len(), 21, "с 9 по 29 октября, дни между поездками — тоже");
        let want: &[(&str, (u32, u32, u32))] = &[
            ("2026-10-09", (2, 5, 0)),
            ("2026-10-10", (0, 0, 14)),
            ("2026-10-11", (0, 0, 14)),
            ("2026-10-12", (0, 0, 0)),
            ("2026-10-13", (2, 6, 0)),
            ("2026-10-14", (2, 6, 0)),
            ("2026-10-15", (2, 9, 0)),
            ("2026-10-23", (2, 7, 0)),
            ("2026-10-24", (0, 0, 10)),
            ("2026-10-25", (0, 0, 13)),
            ("2026-10-26", (2, 4, 0)),
            ("2026-10-27", (2, 4, 0)),
            ("2026-10-28", (2, 1, 0)),
        ];
        for (date, (a, b, c)) in want {
            let row = s.rows.iter().find(|x| x.date == d(date)).unwrap();
            assert_eq!(row.hours.unwrap(), Hours { x15: a * 60, x2: b * 60, off: c * 60 }, "{date}");
        }
        let late = s.rows.iter().find(|x| x.date == d("2026-10-29")).unwrap().hours.unwrap();
        assert_eq!(late, Hours { x15: 0, x2: 270, off: 0 }, "с 21:45 до 2:15 — 4,5 часа после 19:00");
        assert_eq!((s.x15, s.double()), (16 * 60, 97 * 60 + 30));
        assert_eq!(summary(&s), "Работы с коэффициентом 1,5 - 16 часов, Работы в выходные дни и более 2 часов с коэффициентом 2 - 97,5 часа");
        let gap = s.rows.iter().find(|x| x.date == d("2026-10-22")).unwrap();
        assert_eq!((gap.hours, gap.comment.as_str()), (None, "Чекмагуш. организационные и подготовительные работы"));
        assert!(s.rows.iter().find(|x| x.date == d("2026-10-17")).unwrap().off);
    }

    #[test]
    fn pay_for_october_like_we_counted() {
        let trips = [trip("561", "2026-10-09", "2026-10-15"), trip("573", "2026-10-22", "2026-10-29")];
        let s = month_sheet(d("2026-10-01"), &trips, &sample(), &Rules::DEFAULT);
        let mut set = BTreeMap::new();
        assert!(pay(&s, &set).is_empty(), "без оклада суммы нет");
        set.insert("ot.salary".to_string(), "9000000".to_string());
        set.insert("ot.bonus".to_string(), "3000000".to_string());
        let p = pay(&s, &set);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].month.as_str(), p[0].norm, p[0].exact, p[0].rate), ("2026-10", 176 * 60, true, 68_182));
        assert_eq!(p[0].sum, 14_931_818, "149 318,18 ₽ — как в расчёте по образцу");
        set.insert("ot.norm.2026-10".to_string(), "168".to_string());
        let p = pay(&s, &set);
        assert!(p[0].custom && p[0].norm == 168 * 60);
    }

    #[test]
    fn rules_and_rounding() {
        let r = Rules::DEFAULT;
        // 8:55 после 19:00 — 9 часов; 4:50 — 5; 1:14 — 1; 1:15 — 1,5.
        assert_eq!(day_hours(false, 8 * 60, 3 * 60 + 55, &r).x2, 9 * 60);
        assert_eq!(day_hours(false, 8 * 60, 23 * 60 + 50, &r).x2, 5 * 60);
        assert_eq!(day_hours(false, 8 * 60, 20 * 60 + 14, &r).x2, 60);
        assert_eq!(day_hours(false, 8 * 60, 20 * 60 + 15, &r).x2, 90);
        // Начало после 17:00 — часть с коэффициентом 1,5 тоже считается.
        assert_eq!(day_hours(false, 18 * 60, 21 * 60, &r), Hours { x15: 60, x2: 120, off: 0 });
        // Выходной: обед вычитается, только если смена захватывает 12–13.
        assert_eq!(day_hours(true, 13 * 60, 20 * 60, &r).off, 7 * 60);
        assert_eq!(day_hours(true, 9 * 60, 18 * 60, &r).off, 8 * 60);
        // Свои правила: рабочий день до 18:00, без обеда, округление до четверти часа.
        let mut s = BTreeMap::new();
        s.insert("ot.day_end".to_string(), "18:00".to_string());
        s.insert("ot.lunch".to_string(), "0".to_string());
        s.insert("ot.round".to_string(), "15".to_string());
        let own = Rules::from_settings(&s);
        assert_eq!(own, Rules { day_start: 8 * 60, day_end: 18 * 60, first: 120, lunch: 0, step: 15 });
        s.insert("ot.day_start".to_string(), "9:00".to_string());
        assert_eq!(Rules::from_settings(&s).day_start, 9 * 60);
        s.insert("ot.day_start".to_string(), "14:00".to_string());
        assert_eq!(Rules::from_settings(&s).day_start, 8 * 60, "начало дня после полудня не принимается");
        assert_eq!(day_hours(false, 8 * 60, 20 * 60 + 40, &own), Hours { x15: 120, x2: 45, off: 0 });
        assert_eq!(day_hours(true, 9 * 60, 18 * 60, &own).off, 9 * 60);
        s.insert("ot.round".to_string(), "7".to_string());
        assert_eq!(Rules::from_settings(&s).step, 30, "неподходящее округление не принимается — до получаса");
    }

    #[test]
    fn times_words_and_names() {
        assert_eq!(parse_time("8:00"), Some(480));
        assert_eq!(parse_time("08:05"), Some(485));
        assert_eq!(parse_time("23:59"), Some(1439));
        for bad in ["24:00", "8:60", "8", "8:5", "", "8:00:00", "a:bc", "+8:00"] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
        assert_eq!(time_text(485), "8:05");
        assert_eq!((hours_text(16 * 60), hours_text(97 * 60 + 30), hours_text(15)), ("16".to_string(), "97,5".to_string(), "0,25".to_string()));
        assert_eq!([60, 120, 300, 21 * 60, 22 * 60, 25 * 60, 11 * 60, 12 * 60, 90].map(hours_word), ["час", "часа", "часов", "час", "часа", "часов", "часов", "часов", "часа"]);
        let o = WorkOrder { id: "o1".into(), trip: "561".into(), number: "4/30".into(), object: String::new(), from: d("2026-10-06"), to: d("2026-10-12"), att: vec![], note: String::new(), extra: Default::default() };
        assert_eq!(order_file_name(&o), "Наряд 4-30 с 06.10.2026 по 12.10.2026");
        assert_eq!(order_file_name(&WorkOrder { number: String::new(), to: d("2026-10-06"), ..o.clone() }), "Наряд за 06.10.2026");
        assert_eq!(report_name("Иванов", &month_title(d("2026-10-15"))), "Переработки Иванов октябрь 2026");
        assert!(check_day(&day("561", "2026-10-09", "8:00", "", "")).is_err());
        assert!(check_day(&day("561", "2026-10-09", "25:00", "1:00", "")).is_err());
        assert!(check_day(&day("561", "2026-10-09", "", "", "Как у Фирсова")).is_ok());
    }

    #[test]
    fn trip_sheet_covers_trip_days_and_moved_records() {
        let t = trip("573", "2026-10-21", "2026-10-31");
        let mut days = sample();
        days.push(day("573", "2026-11-02", "8:00", "20:00", "дополнительный день"));
        let s = trip_sheet(&t, &days, &Rules::DEFAULT);
        assert_eq!((s.rows.first().unwrap().date, s.rows.last().unwrap().date), (d("2026-10-21"), d("2026-11-02")));
        assert!(s.rows.iter().all(|r| r.trip.as_deref().is_none_or(|x| x == "573")), "дни другой поездки не попадают");
    }

    #[test]
    fn excel_looks_like_the_sample() {
        let trips = [trip("561", "2026-10-09", "2026-10-15"), trip("573", "2026-10-22", "2026-10-29")];
        let s = month_sheet(d("2026-10-01"), &trips, &sample(), &Rules::DEFAULT);
        let x = sheet_xml(&s, &Rules::DEFAULT);
        // Заголовки, дата и время числами в формате образца, прочерки, зелёные выходные, итог через строку.
        assert!(x.contains(">Переработки с 17:00 до 19:00<") && x.contains(">Переработки с 19:00<") && x.contains(">Комментарии<"));
        assert!(x.contains(r#"<c r="B2" s="3"><v>46304</v></c>"#), "9 октября 2026 — 46304");
        assert!(x.contains(r#"<c r="C2" s="4"><v>0.3333333333333333</v></c>"#));
        assert!(x.contains(r#"<c r="B3" s="7"><v>46305</v></c>"#) && x.contains(r#"<c r="G3" s="9"><v>14</v></c>"#), "10 октября — выходной, зелёный");
        assert!(x.contains(r#"<c r="F22" s="5"><v>4.5</v></c>"#));
        assert!(x.contains(r#"<c r="E3" s="9" t="inlineStr"><is><t xml:space="preserve">-</t></is></c>"#));
        assert!(x.contains(r#"<mergeCell ref="B24:D24"/>"#) && x.contains(">Итого переработок:<") && x.contains("97,5 часа"));
        let bytes = xlsx(&s, &Rules::DEFAULT).unwrap();
        assert_eq!(&bytes[..2], b"PK");
    }

    #[test]
    fn report_files_name_orders_and_report_gaps() {
        let s = Sheet::default();
        let jpeg = {
            let mut j = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x20, 0x03, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1];
            j.extend_from_slice(&[0xFF, 0xD9]);
            j
        };
        let att = |id: &str, mime: &str| Attachment { id: id.into(), mime: mime.into(), name: String::new(), size: 0 };
        let o = |id: &str, number: &str, from: &str, to: &str, a: Vec<Attachment>| WorkOrder { id: id.into(), trip: "573".into(), number: number.into(), object: String::new(), from: d(from), to: d(to), att: a, note: String::new(), extra: Default::default() };
        let orders = vec![
            o("a", "3-2/23", "2026-10-23", "2026-10-26", vec![att("p1", "image/jpeg"), att("p2", "image/jpeg")]),
            o("b", "3-2/25", "2026-10-26", "2026-10-29", vec![att("f1", "application/pdf"), att("p3", "image/jpeg"), att("gone", "image/jpeg")]),
            o("c", "3-2/23", "2026-10-23", "2026-10-26", vec![att("f2", "application/pdf")]),
            o("d", "7", "2026-10-30", "2026-10-30", vec![]),
        ];
        let mut blob = |id: &str| -> Result<Option<(String, Vec<u8>)>> {
            Ok(match id {
                "f1" | "f2" => Some(("application/pdf".into(), b"%PDF-1.7 file".to_vec())),
                "gone" => None,
                _ => Some(("image/jpeg".into(), jpeg.clone())),
            })
        };
        let (files, notes) = report_files("Переработки 573 Чекмагуш", &s, &Rules::DEFAULT, &orders, &mut blob).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(
            names,
            [
                "Переработки 573 Чекмагуш.xlsx",
                "Наряды/Наряд 3-2-23 с 23.10.2026 по 26.10.2026.pdf",
                "Наряды/Наряд 3-2-23 с 23.10.2026 по 26.10.2026 (2).pdf",
                "Наряды/Наряд 3-2-25 с 26.10.2026 по 29.10.2026.pdf",
                "Наряды/Наряд 3-2-25 с 26.10.2026 по 29.10.2026 (2).pdf",
            ]
        );
        assert!(files[1].1.starts_with(b"%PDF-1.4") && String::from_utf8_lossy(&files[1].1).contains("/Count 2"), "два снимка — две страницы");
        assert_eq!(files[4].1, b"%PDF-1.7 file", "PDF наряда — как есть");
        assert!(notes.iter().any(|n| n.contains("не пришёл")) && notes.iter().any(|n| n.contains("Наряд 7 за 30.10.2026: нет снимков")), "{notes:?}");
    }
}
