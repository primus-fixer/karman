//! Переработки: наряды и рабочие дни поездки, общая таблица за месяц и отчёт —
//! таблица Excel и папка с нарядами.

use crate::{day, err, snap, AppState, Res};
use karman_core::book::{self, Snapshot};
use karman_core::overtime::{self as ot, Pay, Rules, Sheet, WorkDay, WorkOrder};
use karman_core::sync::EventBody;
use karman_core::Date;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

/// Наряд поездки: новый или исправленный.
#[tauri::command]
pub fn order_save(state: State<AppState>, today: String, mut order: WorkOrder) -> Res<Snapshot> {
    if order.to < order.from {
        return Err("Дата окончания наряда раньше начала".into());
    }
    ot::check_order(&order).map_err(err)?;
    if order.id.is_empty() {
        order.id = book::new_id().map_err(err)?;
    }
    let mut st = state.store()?;
    st.append(EventBody::OrderPut { order }).map_err(err)?;
    snap(&st, &today)
}

#[tauri::command]
pub fn order_delete(state: State<AppState>, today: String, id: String) -> Res<Snapshot> {
    let mut st = state.store()?;
    st.append(EventBody::OrderDelete { id }).map_err(err)?;
    snap(&st, &today)
}

/// Рабочий день поездки. Пустые время и комментарий — день очищается.
#[tauri::command]
pub fn day_save(state: State<AppState>, today: String, mut day: WorkDay) -> Res<Snapshot> {
    day.start = day.start.trim().to_string();
    day.end = day.end.trim().to_string();
    day.comment = day.comment.trim().to_string();
    ot::check_day(&day).map_err(err)?;
    day.id = ot::day_id(&day.trip, day.date);
    let mut st = state.store()?;
    let body = if day.start.is_empty() && day.comment.is_empty() { EventBody::DayDelete { id: day.id } } else { EventBody::DayPut { day } };
    st.append(body).map_err(err)?;
    snap(&st, &today)
}

/// Общая таблица переработок за месяц — из всех поездок месяца.
#[derive(Serialize)]
pub struct MonthView {
    /// «2026-10»
    month: String,
    /// «октябрь 2026»
    title: String,
    sheet: Sheet,
    summary: String,
    pay: Vec<Pay>,
    orders: Vec<WorkOrder>,
    /// Имена файлов нарядов в отчёте — в том же порядке, что и наряды.
    names: Vec<String>,
    rules: Rules,
    /// Месяцы, в которых есть поездки: для выбора.
    months: Vec<String>,
}

fn month_of(s: &str) -> Res<Date> {
    day(&format!("{s}-01")).map_err(|_| format!("неверный месяц «{s}»"))
}

fn in_month(o: &WorkOrder, m: Date) -> bool {
    o.from.min(o.to) <= m.month_end() && o.from.max(o.to) >= m.month_start()
}

#[tauri::command]
pub fn overtime_month(state: State<AppState>, month: String) -> Res<MonthView> {
    let m = month_of(&month)?;
    let st = state.store()?;
    let settings = st.settings().map_err(err)?;
    let trips = st.trips().map_err(err)?;
    let rules = Rules::from_settings(&settings);
    let sheet = ot::month_sheet(m, &trips, &st.work_days().map_err(err)?, &rules);
    let mut months: Vec<String> = trips
        .iter()
        .flat_map(|t| {
            let (mut a, b) = (t.start.min(t.end).month_start(), t.start.max(t.end).month_start());
            let mut out = Vec::new();
            while a <= b && out.len() < 24 {
                let (y, mo, _) = a.ymd();
                out.push(format!("{y}-{mo:02}"));
                a = a.add_months(1);
            }
            out
        })
        .collect();
    months.sort();
    months.dedup();
    months.reverse();
    let mut orders: Vec<WorkOrder> = st.orders().map_err(err)?.into_iter().filter(|o| in_month(o, m)).collect();
    orders.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.number.cmp(&b.number)));
    Ok(MonthView {
        month,
        title: ot::month_title(m),
        summary: ot::summary(&sheet),
        pay: ot::pay(&sheet, &settings),
        names: orders.iter().map(ot::order_file_name).collect(),
        orders,
        sheet,
        rules,
        months,
    })
}

#[derive(Serialize)]
pub struct Exported {
    /// Папка отчёта в «Загрузках».
    pub path: String,
    /// Сколько файлов нарядов в папке «Наряды».
    pub files: usize,
    /// Что не вошло и почему.
    pub notes: Vec<String>,
}

/// Папка без повторов в «Загрузках».
fn unique_dir(base: &std::path::Path, name: &str) -> std::path::PathBuf {
    let mut path = base.join(name);
    let mut n = 2;
    while path.exists() {
        path = base.join(format!("{name} ({n})"));
        n += 1;
    }
    path
}

/// Отчёт по переработкам: по поездке (`trip`) или общий за месяц (`month`) —
/// папка в «Загрузках»: таблица Excel и «Наряды». Как и другие выгрузки, собирается на компьютере.
#[tauri::command]
pub fn overtime_export(app: AppHandle, state: State<AppState>, trip: Option<String>, month: Option<String>) -> Res<Exported> {
    export_report(&app, &state, trip, month)
}

/// Отчёт по переработкам в «Загрузки» — для кнопки на экране и для Claude.
pub fn export_report(app: &AppHandle, state: &AppState, trip: Option<String>, month: Option<String>) -> Res<Exported> {
    if cfg!(mobile) {
        return Err("Отчёт по переработкам собирается на компьютере: свяжите устройства по Wi-Fi и выгрузите там".into());
    }
    let (files, notes, base) = {
        let st = state.store()?;
        let settings = st.settings().map_err(err)?;
        let rules = Rules::from_settings(&settings);
        let trips = st.trips().map_err(err)?;
        let days = st.work_days().map_err(err)?;
        let all = st.orders().map_err(err)?;
        let who = settings.get("ot.name").cloned().unwrap_or_default();
        let (sheet, orders, what) = match (&trip, &month) {
            (Some(id), _) => {
                let t = trips.iter().find(|t| &t.id == id).ok_or("Поездка не найдена")?;
                (ot::trip_sheet(t, &days, &rules), all.into_iter().filter(|o| &o.trip == id).collect::<Vec<_>>(), format!("{} {}", t.id, t.city))
            }
            (None, Some(m)) => {
                let m = month_of(m)?;
                (ot::month_sheet(m, &trips, &days, &rules), all.into_iter().filter(|o| in_month(o, m)).collect(), ot::month_title(m))
            }
            _ => return Err("Нужна поездка или месяц".into()),
        };
        if sheet.rows.is_empty() {
            return Err("В этом месяце нет поездок — отчёт пустой".into());
        }
        let base = karman_core::zip::safe_name(&ot::report_name(&who, &what));
        let mut blob = |id: &str| st.blob(id);
        let (files, notes) = ot::report_files(&base, &sheet, &rules, &orders, &mut blob).map_err(err)?;
        (files, notes, base)
    };
    let count = files.len() - 1;
    let downloads = app.path().download_dir().or_else(|_| app.path().home_dir().map(|h| h.join("Downloads"))).map_err(|_| "не нашлась папка «Загрузки»".to_string())?;
    let dir = unique_dir(&downloads, &base);
    for (name, data) in &files {
        let path = name.split('/').fold(dir.clone(), |p, part| p.join(part));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(err)?;
        }
        std::fs::write(&path, data).map_err(err)?;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    }
    Ok(Exported { path: dir.display().to_string(), files: count, notes })
}
