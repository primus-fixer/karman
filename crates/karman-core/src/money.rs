//! Деньги хранятся целыми копейками, чтобы суммы сходились до копейки, как в 1С.

/// Сумма в копейках. Отрицательная — уменьшение.
pub type Kop = i64;

const NBSP: char = '\u{a0}';

/// Рубли (можно с копейками) в копейки. Для констант и тестов.
pub fn rub(rubles: f64) -> Kop {
    (rubles * 100.0).round() as Kop
}

/// «66 358,90» или «700» без копеек, если их нет. Минус — настоящий знак «−».
pub fn fmt(k: Kop) -> String {
    let neg = k < 0;
    let a = k.unsigned_abs();
    let int = group(a / 100);
    let frac = a % 100;
    let mut s = String::new();
    if neg {
        s.push('−');
    }
    s.push_str(&int);
    if frac != 0 {
        s.push_str(&format!(",{frac:02}"));
    }
    s
}

/// Всегда с копейками: «122 000,00» — так суммы вводятся в 1С.
pub fn fmt2(k: Kop) -> String {
    let neg = k < 0;
    let a = k.unsigned_abs();
    format!("{}{},{:02}", if neg { "−" } else { "" }, group(a / 100), a % 100)
}

/// «66 358,90 ₽»
pub fn rub_text(k: Kop) -> String {
    format!("{}{NBSP}₽", fmt(k))
}

fn group(mut n: u64) -> String {
    let mut parts = Vec::new();
    loop {
        if n < 1000 {
            parts.push(n.to_string());
            break;
        }
        parts.push(format!("{:03}", n % 1000));
        n /= 1000;
    }
    parts.reverse();
    parts.join(&NBSP.to_string())
}

/// Разбор суммы, набранной человеком: «1 500,50», «1500.5», «700».
pub fn parse(text: &str) -> Option<Kop> {
    let t: String = text.chars().filter(|c| !c.is_whitespace() && *c != NBSP).collect();
    let t = t.replace(',', ".");
    if t.is_empty() {
        return None;
    }
    let (int, frac) = match t.split_once('.') {
        Some((i, f)) => (i, f),
        None => (t.as_str(), ""),
    };
    if int.is_empty() && frac.is_empty() || frac.len() > 2 {
        return None;
    }
    if !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let i: i64 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let f: i64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().ok()? * 10,
        _ => frac.parse().ok()?,
    };
    i.checked_mul(100)?.checked_add(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_1c() {
        assert_eq!(fmt(rub(66358.90)), "66\u{a0}358,90");
        assert_eq!(fmt(rub(700.0)), "700");
        assert_eq!(fmt(-rub(1000.0)), "−1\u{a0}000");
        assert_eq!(fmt2(rub(122000.0)), "122\u{a0}000,00");
        assert_eq!(fmt2(rub(5870.55)), "5\u{a0}870,55");
        assert_eq!(rub_text(rub(9500.0)), "9\u{a0}500\u{a0}₽");
    }

    #[test]
    fn parses_typed_amounts() {
        assert_eq!(parse("1 500,50"), Some(150050));
        assert_eq!(parse("1500.5"), Some(150050));
        assert_eq!(parse("700"), Some(70000));
        assert_eq!(parse(",5"), Some(50));
        assert_eq!(parse("12,345"), None);
        assert_eq!(parse("abc"), None);
        assert_eq!(parse(""), None);
    }
}
