//! Производственный календарь России: выходные и праздники, сокращённые дни и норма часов.
//!
//! 2026 год — постановление Правительства № 1466 от 24.09.2025, 2027 год — № 1187 от 17.09.2026.
//! Для других лет — оценка: праздники по ст. 112 ТК РФ и перенос праздника с выходного
//! на ближайший будний день, без переносов из постановлений; норму часов тогда можно поправить.

use crate::date::Date;

/// Календарь года: нерабочие будни (праздники и перенесённые выходные), рабочие выходные
/// и сокращённые на час предпраздничные дни — (месяц, число).
struct Year {
    year: i32,
    off: &'static [(u32, u32)],
    work: &'static [(u32, u32)],
    short: &'static [(u32, u32)],
}

const YEARS: &[Year] = &[
    Year {
        year: 2026,
        off: &[(1, 1), (1, 2), (1, 5), (1, 6), (1, 7), (1, 8), (1, 9), (2, 23), (3, 9), (5, 1), (5, 11), (6, 12), (11, 4), (12, 31)],
        work: &[],
        short: &[(4, 30), (5, 8), (6, 11), (11, 3)],
    },
    Year {
        year: 2027,
        off: &[(1, 1), (1, 4), (1, 5), (1, 6), (1, 7), (1, 8), (2, 22), (2, 23), (3, 8), (5, 3), (5, 10), (6, 14), (11, 4), (11, 5), (12, 31)],
        work: &[(2, 20)],
        short: &[(2, 20), (4, 30), (6, 11), (11, 3)],
    },
];

/// Праздники по ст. 112 ТК РФ, кроме новогодних (1–8 января).
const HOLIDAYS: [(u32, u32); 6] = [(2, 23), (3, 8), (5, 1), (5, 9), (6, 12), (11, 4)];

fn year_of(y: i32) -> Option<&'static Year> {
    YEARS.iter().find(|c| c.year == y)
}

fn has(list: &[(u32, u32)], m: u32, d: u32) -> bool {
    list.contains(&(m, d))
}

/// Календарь года известен точно (есть постановление о переносах).
pub fn exact(year: i32) -> bool {
    year_of(year).is_some()
}

/// Оценка для лет без постановления: праздник, выпавший на субботу или воскресенье, переносится на понедельник.
fn estimated_off(d: Date) -> bool {
    let (_, m, day) = d.ymd();
    if (m == 1 && day <= 8) || HOLIDAYS.contains(&(m, day)) {
        return true;
    }
    d.weekday() == 1
        && (1..=2).any(|back| {
            let (_, pm, pd) = d.add_days(-back).ymd();
            HOLIDAYS.contains(&(pm, pd))
        })
}

/// Нерабочий день: выходной (если не рабочий по переносу), праздник или перенесённый выходной.
pub fn is_day_off(d: Date) -> bool {
    let (y, m, day) = d.ymd();
    let weekend = d.weekday() >= 6;
    match year_of(y) {
        Some(c) => (weekend && !has(c.work, m, day)) || has(c.off, m, day),
        None => weekend || estimated_off(d),
    }
}

/// Сокращённый на час предпраздничный день.
pub fn is_short(d: Date) -> bool {
    let (y, m, day) = d.ymd();
    year_of(y).is_some_and(|c| has(c.short, m, day))
}

/// Норма рабочего времени за месяц при 40-часовой неделе, в минутах; `true` — по календарю точно.
pub fn norm_minutes(year: i32, month: u32) -> (u32, bool) {
    let Some(first) = Date::new(year, month, 1) else { return (0, false) };
    let last = first.month_end();
    let mut minutes = 0;
    let mut d = first;
    while d <= last {
        if !is_day_off(d) {
            minutes += if is_short(d) { 7 * 60 } else { 8 * 60 };
        }
        d = d.add_days(1);
    }
    (minutes, exact(year))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    #[test]
    fn norms_match_the_official_calendars() {
        // Рабочих часов по месяцам: производственные календари 2026 и 2027 годов.
        let y2026 = [120, 152, 168, 175, 151, 167, 184, 168, 176, 176, 159, 176];
        let y2027 = [120, 151, 176, 175, 152, 167, 176, 176, 176, 168, 159, 176];
        for (year, table) in [(2026, y2026), (2027, y2027)] {
            for (i, hours) in table.iter().enumerate() {
                assert_eq!(norm_minutes(year, i as u32 + 1), (hours * 60, true), "{year}-{:02}", i + 1);
            }
            assert_eq!(table.iter().sum::<u32>(), 1972);
        }
    }

    #[test]
    fn days_off_holidays_and_transfers() {
        assert!(is_day_off(d("2026-10-10")) && is_day_off(d("2026-10-11")), "суббота и воскресенье");
        assert!(!is_day_off(d("2026-10-09")), "пятница — рабочая");
        assert!(is_day_off(d("2026-01-09")), "перенос с 3 января");
        assert!(is_day_off(d("2026-12-31")), "перенос с 4 января");
        assert!(is_day_off(d("2026-03-09")), "8 марта в воскресенье — выходной в понедельник");
        assert!(is_day_off(d("2026-11-04")));
        assert!(!is_day_off(d("2027-02-20")), "рабочая суббота 2027 года");
        assert!(is_short(d("2027-02-20")) && is_short(d("2026-11-03")) && !is_short(d("2026-11-02")));
        assert!(is_day_off(d("2027-11-05")), "перенос с 2 января 2027 года");
        // Без постановления — оценка по ТК: 4 ноября 2028 года — суббота, выходной переносится на понедельник.
        assert!(is_day_off(d("2028-05-09")) && is_day_off(d("2028-01-03")));
        assert!(is_day_off(d("2028-11-06")) && !is_day_off(d("2028-11-07")));
        assert!(!exact(2028));
        assert_eq!(d("2026-11-08").weekday(), 7);
        assert_eq!(d("2026-11-09").weekday(), 1);
    }
}
