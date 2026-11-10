//! Выгрузка за период в Excel: листы «Операции», «По категориям», «По месяцам»
//! и «Командировки» за год — то же, что на экране «Статистика».

use crate::book::Account;
use crate::category::{self, Category};
use crate::money::Kop;
use crate::stats::Stats;
use crate::trip::{DocKind, Op, OpKind};
use crate::xlsx::{self, Cell, Sheet};

/// Название встроенной категории — как на экранах. Свои категории — [`crate::category::name`].
pub fn cat_name(cat: &str) -> &'static str {
    match cat {
        "tickets" => "Билеты",
        "lodging" => "Проживание",
        "taxi" => "Такси и транспорт",
        "car" => "Аренда авто и топливо",
        "food" => "Питание",
        "tools" => "Материалы и инструменты",
        "groceries" => "Продукты",
        "cafe" => "Кафе",
        "transport" => "Транспорт",
        "auto" => "Автомобиль",
        "home" => "Дом",
        "telecom" => "Связь и интернет",
        "subscriptions" => "Подписки",
        "health" => "Здоровье",
        "clothes" => "Одежда и обувь",
        "beauty" => "Красота",
        "kids" => "Дети",
        "pets" => "Питомцы",
        "sport" => "Спорт",
        "fun" => "Развлечения",
        "education" => "Образование",
        "travel" => "Отдых и путешествия",
        "gifts" => "Подарки",
        "taxes" => "Налоги и сборы",
        "salary" => "Зарплата",
        "interest" => "Проценты",
        "otherIn" => "Другой доход",
        "advance" => "Аванс",
        "reimb" => "Возмещение",
        "ret" => "Возврат компании",
        "transfer" => "Перевод между счетами",
        "debt" => "Долг",
        _ => "Другое",
    }
}

fn kind_name(o: &Op) -> &'static str {
    match o.kind {
        OpKind::Personal if o.family => "Семейный расход",
        OpKind::Personal => "Личный расход",
        OpKind::Work => "Расход поездки",
        OpKind::Food => "Питание из суточных",
        OpKind::Income => "Доход",
        OpKind::Advance => "Аванс",
        OpKind::Reimbursement => "Возмещение",
        OpKind::Return => "Возврат компании",
        OpKind::Debt if o.inflow => "Долг: деньги пришли",
        OpKind::Debt => "Долг: деньги ушли",
        OpKind::Transfer => "Перевод",
    }
}

const MONTHS: [&str; 12] = ["Январь", "Февраль", "Март", "Апрель", "Май", "Июнь", "Июль", "Август", "Сентябрь", "Октябрь", "Ноябрь", "Декабрь"];

fn text(s: impl Into<String>) -> Cell {
    let s = s.into();
    if s.is_empty() {
        Cell::Empty
    } else {
        Cell::Text(s)
    }
}

/// Файл .xlsx с операциями периода и итогами из статистики.
pub fn workbook(ops: &[Op], accounts: &[Account], custom: &[Category], s: &Stats) -> std::io::Result<Vec<u8>> {
    let name = |id: &str| accounts.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_else(|| "Счёт удалён".into());
    let mut list: Vec<&Op> = ops.iter().filter(|o| o.date >= s.from && o.date <= s.to).collect();
    list.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
    let head = ["Дата", "Операция", "Вид", "Категория", "Сумма, ₽", "Счёт", "Куда", "В валюте", "Валюта", "Поездка", "Метка", "Документ", "Примечание"];
    let mut rows: Vec<Vec<Cell>> = vec![head.iter().map(|h| Cell::Text(h.to_string())).collect()];
    for o in list {
        // Приход — плюс, расход — минус; перевод — без знака, он не меняет «Мои деньги».
        let signed: Kop = if o.kind == OpKind::Transfer || o.is_inflow() { o.amount } else { -o.amount };
        let mark = if o.family {
            "семья".to_string()
        } else if o.kind == OpKind::Personal && o.trip.is_some() {
            format!("метка поездки {}", o.trip.as_deref().unwrap_or(""))
        } else if o.plan.is_some() {
            "по графику".into()
        } else if o.goal.is_some() {
            "цель".into()
        } else {
            String::new()
        };
        let doc = match (o.kind, o.doc) {
            (OpKind::Work, Some(DocKind::Photo)) => "фото чека",
            (OpKind::Work, Some(DocKind::Qr)) => "чек по QR-коду",
            (OpKind::Work, Some(DocKind::File)) => "файл",
            (OpKind::Work, None) => "без чека",
            _ => "",
        };
        rows.push(vec![
            Cell::Date(o.date),
            text(o.title.clone()),
            text(kind_name(o)),
            text(category::name(if o.kind == OpKind::Debt { "debt" } else { &o.cat }, custom)),
            Cell::Money(signed),
            text(name(&o.account)),
            o.to.as_deref().map(|t| text(name(t))).unwrap_or(Cell::Empty),
            o.orig.map(Cell::Money).unwrap_or(Cell::Empty),
            o.cur.clone().map(text).unwrap_or(Cell::Empty),
            o.trip.clone().map(text).unwrap_or(Cell::Empty),
            text(mark),
            text(doc),
            text(o.note.clone()),
        ]);
    }
    let ops_sheet = Sheet { name: "Операции".into(), widths: vec![11.0, 34.0, 20.0, 24.0, 14.0, 18.0, 18.0, 12.0, 8.0, 9.0, 18.0, 15.0, 40.0], rows };

    // Пока период идёт, сравнение — с теми же днями прошлого.
    let prev = if s.partial { format!("{} – {}, ₽", s.prev_from.dotted(), s.prev_to.dotted()) } else { "Прошлый период, ₽".into() };
    let mut cats = vec![vec![Cell::Text("Категория".into()), Cell::Text("Сумма, ₽".into()), Cell::Text(prev), Cell::Text("Разница, ₽".into())]];
    for c in &s.cats {
        cats.push(vec![text(category::name(&c.cat, custom)), Cell::Money(c.amount), Cell::Money(c.prev), Cell::Money(c.amount - c.prev)]);
    }
    cats.push(vec![]);
    cats.push(vec![Cell::Text("Всего".into()), Cell::Money(s.spent), Cell::Money(s.prev_spent), Cell::Money(s.spent - s.prev_spent)]);
    let cats_sheet = Sheet { name: "По категориям".into(), widths: vec![26.0, 14.0, 20.0, 14.0], rows: cats };

    let mut months = vec![["Месяц", "Доходы, ₽", "Расходы, ₽", "Отложено, ₽"].iter().map(|h| Cell::Text(h.to_string())).collect::<Vec<_>>()];
    for m in &s.months {
        let (y, mm) = m.month.split_once('-').unwrap_or(("", "1"));
        let idx = mm.parse::<usize>().unwrap_or(1).clamp(1, 12) - 1;
        months.push(vec![Cell::Text(format!("{} {y}", MONTHS[idx])), Cell::Money(m.income), Cell::Money(m.spent), Cell::Money(m.saved)]);
    }
    let months_sheet = Sheet { name: "По месяцам".into(), widths: vec![16.0, 14.0, 14.0, 14.0], rows: months };

    let t = &s.trips;
    let mut trips = vec![["Поездка", "Город", "Начало", "Конец", "Дней", "Суточные, ₽", "Осталось от суточных, ₽", "Долг перед компанией, ₽", "Расходы с документами, ₽", "Состояние"].iter().map(|h| Cell::Text(h.to_string())).collect::<Vec<_>>()];
    for i in &t.items {
        trips.push(vec![
            text(i.id.clone()),
            text(i.city.clone()),
            Cell::Date(i.start),
            Cell::Date(i.end),
            Cell::Int(i.days as i64),
            Cell::Money(i.per_diem),
            Cell::Money(i.left),
            Cell::Money(i.debt),
            Cell::Money(i.confirmed),
            text(if i.finished { "завершена" } else { "идёт" }),
        ]);
    }
    if !t.items.is_empty() {
        trips.push(vec![]);
        trips.push(vec![Cell::Text(format!("Всего: {}", t.count)), Cell::Empty, Cell::Empty, Cell::Empty, Cell::Int(t.days as i64), Cell::Money(t.per_diem), Cell::Money(t.left), Cell::Money(t.debt), Cell::Money(t.confirmed), Cell::Empty]);
    }
    let trips_sheet = Sheet { name: format!("Командировки {}", t.year), widths: vec![10.0, 18.0, 11.0, 11.0, 7.0, 13.0, 22.0, 22.0, 22.0, 12.0], rows: trips };
    xlsx::build(&[ops_sheet, cats_sheet, months_sheet, trips_sheet])
}

/// Имя файла: «Карман 2026-11», «Карман 2026, IV квартал», «Карман 2026».
pub fn file_name(s: &Stats) -> String {
    let (y, m, _) = s.from.ymd();
    let months = s.from.days_until(s.to.add_days(1));
    if months > 300 {
        format!("Карман {y}")
    } else if months > 80 {
        format!("Карман {y}, {} квартал", ["I", "II", "III", "IV"][((m - 1) / 3) as usize])
    } else {
        format!("Карман {y}-{m:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{stats, Span};

    #[test]
    fn workbook_of_demo_month_opens() {
        let ops = crate::demo::ops();
        let accounts = crate::book::default_accounts();
        let s = stats(&ops, &accounts, &crate::demo::trips(), crate::trip::DEFAULT_RATE, Span::Month, crate::demo::today(), crate::demo::today());
        let bytes = workbook(&ops, &accounts, &[], &s).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        assert!(s.trips.count >= 3, "в демо есть поездки — лист «Командировки» не пустой");
        assert_eq!(file_name(&s), "Карман 2026-11");
        let q = stats(&ops, &accounts, &[], 70000, Span::Quarter, crate::demo::today(), crate::demo::today());
        assert_eq!(file_name(&q), "Карман 2026, IV квартал");
        let y = stats(&ops, &accounts, &[], 70000, Span::Year, crate::demo::today(), crate::demo::today());
        assert_eq!(file_name(&y), "Карман 2026");
    }
}
