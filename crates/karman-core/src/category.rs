//! Категории личных трат: встроенные и свои.
//!
//! Свою категорию человек заводит сам — название и иконка из набора «Кармана»; она синхронизируется
//! между устройствами, как остальные записи. Любую категорию, кроме «Другое», можно скрыть: она пропадает
//! из выбора, а записанные операции, лимиты бюджета и регулярные платежи остаются с ней.
//!
//! Категории поездки (статьи авансового отчёта 1С) и доходов не меняются.

use crate::book;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Своя категория личных трат.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Category {
    /// «c-» и случайные знаки: по нему на категорию ссылаются операции, бюджет и регулярные платежи.
    pub id: String,
    pub name: String,
    /// Иконка из набора «Кармана» (см. [`ICONS`]).
    pub icon: String,
    /// Поля из более новых версий — хранятся и передаются дальше без потерь.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Встроенные категории личных трат в порядке выбора: код и название. «Другое» — последней.
pub const BUILTIN: [(&str, &str); 19] = [
    ("groceries", "Продукты"),
    ("cafe", "Кафе"),
    ("transport", "Транспорт"),
    ("auto", "Автомобиль"),
    ("home", "Дом"),
    ("telecom", "Связь и интернет"),
    ("subscriptions", "Подписки"),
    ("health", "Здоровье"),
    ("clothes", "Одежда и обувь"),
    ("beauty", "Красота"),
    ("kids", "Дети"),
    ("pets", "Питомцы"),
    ("sport", "Спорт"),
    ("fun", "Развлечения"),
    ("education", "Образование"),
    ("travel", "Отдых и путешествия"),
    ("gifts", "Подарки"),
    ("taxes", "Налоги и сборы"),
    ("other", "Другое"),
];

/// Иконки, из которых выбирают иконку своей категории (те же, что на экранах «Кармана»).
pub const ICONS: [&str; 34] = [
    "groceries", "cafe", "food", "transport", "auto", "taxi", "car", "tickets", "lodging", "home", "telecom", "subscriptions", "health", "clothes", "beauty", "kids", "pets",
    "sport", "fun", "music", "gamepad", "education", "book", "travel", "gifts", "star", "sprout", "bolt", "tools", "taxes", "credit", "salary", "debt", "other",
];

/// Своих категорий — не больше стольких.
pub const MAX_CUSTOM: usize = 50;
/// Название — не длиннее стольких знаков: оно же надпись на кнопке в окне ввода.
pub const MAX_NAME: usize = 24;

/// Общая настройка «категория скрыта»: `cat.hidden.<код>` = 1.
pub fn hidden_key(id: &str) -> String {
    format!("cat.hidden.{id}")
}

/// Категория скрыта из выбора. «Другое» не скрывается: в неё попадает всё, чему нет категории.
pub fn is_hidden(settings: &BTreeMap<String, String>, id: &str) -> bool {
    id != "other" && settings.get(&hidden_key(id)).is_some_and(|v| v == "1")
}

/// Встроенная категория личных трат.
pub fn is_builtin(id: &str) -> bool {
    BUILTIN.iter().any(|(c, _)| *c == id)
}

/// Название категории: своей или встроенной (любой — поездки, дохода); неизвестная — «Другое».
pub fn name(cat: &str, custom: &[Category]) -> String {
    match custom.iter().find(|c| c.id == cat) {
        Some(c) => c.name.clone(),
        None => crate::export::cat_name(cat).to_string(),
    }
}

/// Все категории личных трат по порядку: встроенные, свои по названию, «Другое» — последней.
/// Со скрытыми: ими можно и дальше пометить операцию, если она уже с такой категорией.
pub fn all(custom: &[Category]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = BUILTIN[..BUILTIN.len() - 1].iter().map(|(c, n)| (c.to_string(), n.to_string())).collect();
    let mut own: Vec<&Category> = custom.iter().collect();
    own.sort_by_key(|c| c.name.to_lowercase());
    out.extend(own.into_iter().map(|c| (c.id.clone(), c.name.clone())));
    out.push(("other".into(), "Другое".into()));
    out
}

/// Категории личных трат для выбора: все, кроме скрытых.
pub fn visible(custom: &[Category], settings: &BTreeMap<String, String>) -> Vec<(String, String)> {
    all(custom).into_iter().filter(|(c, _)| !is_hidden(settings, c)).collect()
}

/// Проверяет и приводит в порядок свою категорию перед записью: название, иконка, номер.
/// Новой категории (без номера) номер выдаётся здесь.
pub fn check(c: &mut Category, custom: &[Category]) -> Result<(), String> {
    c.name = c.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let len = c.name.chars().count();
    if len == 0 {
        return Err("Нужно название категории".into());
    }
    if len > MAX_NAME {
        return Err(format!("Название — не длиннее {MAX_NAME} знаков"));
    }
    if c.name.chars().any(char::is_control) {
        return Err("В названии — непечатные знаки".into());
    }
    if !ICONS.contains(&c.icon.as_str()) {
        return Err("Выберите иконку из набора".into());
    }
    if c.id.is_empty() {
        if custom.len() >= MAX_CUSTOM {
            return Err(format!("Своих категорий — не больше {MAX_CUSTOM}"));
        }
        c.id = format!("c-{}", book::new_id().map_err(|e| e.to_string())?);
    } else if !c.id.starts_with("c-") || c.id.len() > 40 || !c.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("Встроенную категорию можно только скрыть".into());
    } else if let Some(old) = custom.iter().find(|o| o.id == c.id) {
        // Поля из более новых версий не теряются, когда категорию правят здесь.
        for (k, v) in &old.extra {
            c.extra.entry(k.clone()).or_insert_with(|| v.clone());
        }
    } else {
        return Err("Такой категории нет — откройте настройки заново".into());
    }
    let low = c.name.to_lowercase();
    let taken = BUILTIN.iter().any(|(_, n)| n.to_lowercase() == low) || custom.iter().any(|o| o.id != c.id && o.name.to_lowercase() == low);
    if taken {
        return Err(format!("Категория «{}» уже есть", c.name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(id: &str, name: &str, icon: &str) -> Category {
        Category { id: id.into(), name: name.into(), icon: icon.into(), extra: Default::default() }
    }

    #[test]
    fn builtins_have_names_and_icons() {
        for (c, n) in BUILTIN {
            assert_eq!(crate::export::cat_name(c), n, "название {c} — одно на экранах и в выгрузке");
            assert!(ICONS.contains(&c), "у встроенной категории {c} — иконка из набора");
        }
        assert_eq!(BUILTIN.last().unwrap().0, "other");
    }

    #[test]
    fn new_category_gets_id_and_tidy_name() {
        let mut c = cat("", "  Ремонт   квартиры ", "tools");
        check(&mut c, &[]).unwrap();
        assert!(c.id.starts_with("c-") && c.id.len() == 18, "{}", c.id);
        assert_eq!(c.name, "Ремонт квартиры");
    }

    #[test]
    fn rejects_bad_categories() {
        let have = [cat("c-1", "Ремонт", "tools")];
        let err = |mut c: Category| check(&mut c, &have).unwrap_err();
        assert_eq!(err(cat("", "", "tools")), "Нужно название категории");
        assert_eq!(err(cat("", &"я".repeat(25), "tools")), "Название — не длиннее 24 знаков");
        assert_eq!(err(cat("", "Дача", "../x")), "Выберите иконку из набора");
        assert_eq!(err(cat("", "ремонт", "home")), "Категория «ремонт» уже есть");
        assert_eq!(err(cat("", "подписки", "star")), "Категория «подписки» уже есть", "со встроенной тоже нельзя");
        assert_eq!(err(cat("groceries", "Еда", "groceries")), "Встроенную категорию можно только скрыть");
        assert_eq!(err(cat("", "Дача\u{7}", "sprout")), "В названии — непечатные знаки");
        assert_eq!(err(cat("c-2", "Дача", "sprout")), "Такой категории нет — откройте настройки заново");
        // Та же категория — переименовать можно, хоть и в то же название; поля новых версий сохраняются.
        let mut newer = cat("c-1", "Ремонт", "tools");
        newer.extra.insert("color".into(), serde_json::json!("teal"));
        let mut same = cat("c-1", "Ремонт", "home");
        assert!(check(&mut same, &[newer]).is_ok());
        assert_eq!(same.extra["color"], "teal");
        let many: Vec<Category> = (0..MAX_CUSTOM).map(|i| cat(&format!("c-{i}"), &format!("К{i}"), "star")).collect();
        assert_eq!(check(&mut cat("", "Ещё одна", "star"), &many).unwrap_err(), "Своих категорий — не больше 50");
    }

    #[test]
    fn lists_custom_after_builtins_and_hides() {
        let custom = [cat("c-b", "Ремонт", "tools"), cat("c-a", "Дача", "sprout")];
        let all = all(&custom);
        let ids: Vec<&str> = all.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(&ids[ids.len() - 3..], ["c-a", "c-b", "other"], "свои — по названию, «Другое» — последней");
        let settings = BTreeMap::from([(hidden_key("cafe"), "1".to_string()), (hidden_key("c-a"), "1".to_string()), (hidden_key("other"), "1".to_string()), (hidden_key("home"), "0".to_string())]);
        let shown: Vec<String> = visible(&custom, &settings).into_iter().map(|(c, _)| c).collect();
        assert!(!shown.contains(&"cafe".to_string()) && !shown.contains(&"c-a".to_string()));
        assert!(shown.contains(&"other".to_string()) && shown.contains(&"home".to_string()), "«Другое» не скрывается; 0 — показана");
        assert_eq!(name("c-a", &custom), "Дача");
        assert_eq!(name("subscriptions", &custom), "Подписки");
        assert_eq!(name("c-удалена", &custom), "Другое");
    }
}
