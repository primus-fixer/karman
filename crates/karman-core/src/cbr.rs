//! Разбор курсов ЦБ РФ: XML с cbr.ru и JSON с зеркала cbr-xml-daily.ru.
//! Сама загрузка — в приложении; здесь только разбор, чтобы его проверяли тесты ядра.

use crate::book::CbrRates;
use crate::Date;
use std::collections::BTreeMap;

fn valid_code(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|c| c.is_ascii_uppercase())
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let i = text.find(open)? + open.len();
    let j = text[i..].find(close)? + i;
    Some(text[i..j].trim())
}

/// Разбирает XML ЦБ. Он в кодировке windows-1251, но нужные поля — латиница и цифры.
pub fn parse_xml(xml: &[u8]) -> Result<CbrRates, String> {
    let text = String::from_utf8_lossy(xml);
    let date = between(&text, "Date=\"", "\"")
        .and_then(|d| {
            let p: Vec<&str> = d.split('.').collect();
            (p.len() == 3).then(|| format!("{}-{}-{}", p[2], p[1], p[0]))
        })
        .filter(|d| Date::parse(d).is_some())
        .ok_or("в ответе ЦБ нет даты")?;
    let mut rates = BTreeMap::new();
    for block in text.split("<Valute").skip(1) {
        let (Some(code), Some(nominal), Some(value)) = (between(block, "<CharCode>", "</CharCode>"), between(block, "<Nominal>", "</Nominal>"), between(block, "<Value>", "</Value>")) else {
            continue;
        };
        let (Ok(n), Ok(v)) = (nominal.parse::<f64>(), value.replace(',', ".").parse::<f64>()) else { continue };
        if n > 0.0 && v > 0.0 && valid_code(code) {
            rates.insert(code.to_string(), v / n);
        }
    }
    if rates.is_empty() {
        return Err("в ответе ЦБ нет курсов".into());
    }
    Ok(CbrRates { date, rates })
}

/// Разбирает курсы с зеркала: {"Date": "2026-11-07T11:30:00+03:00", "Valute": {"USD": {"Nominal": 1, "Value": 81.2}}}.
pub fn parse_json(body: &[u8]) -> Result<CbrRates, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| "ответ зеркала не разобрался")?;
    let date = v["Date"].as_str().filter(|d| d.len() >= 10 && Date::parse(&d[..10]).is_some()).map(|d| d[..10].to_string()).ok_or("в ответе зеркала нет даты")?;
    let mut rates = BTreeMap::new();
    if let Some(map) = v["Valute"].as_object() {
        for (code, x) in map {
            let (Some(n), Some(val)) = (x["Nominal"].as_f64(), x["Value"].as_f64()) else { continue };
            if n > 0.0 && val > 0.0 && valid_code(code) {
                rates.insert(code.clone(), val / n);
            }
        }
    }
    if rates.is_empty() {
        return Err("в ответе зеркала нет курсов".into());
    }
    Ok(CbrRates { date, rates })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cbr_xml() {
        // Кусок настоящего ответа ЦБ (кодировка windows-1251, имена валют здесь не важны).
        let xml = br#"<?xml version="1.0" encoding="windows-1251"?><ValCurs Date="06.11.2026" name="Foreign Currency Market"><Valute ID="R01235"><NumCode>840</NumCode><CharCode>USD</CharCode><Nominal>1</Nominal><Name>X</Name><Value>81,2345</Value><VunitRate>81,2345</VunitRate></Valute><Valute ID="R01335"><NumCode>398</NumCode><CharCode>KZT</CharCode><Nominal>100</Nominal><Name>Y</Name><Value>17,3456</Value><VunitRate>0,173456</VunitRate></Valute></ValCurs>"#;
        let c = parse_xml(xml).unwrap();
        assert_eq!(c.date, "2026-11-06");
        assert!((c.rates["USD"] - 81.2345).abs() < 1e-9);
        assert!((c.rates["KZT"] - 0.173456).abs() < 1e-9, "за 1 тенге, а не за 100");
        assert!(parse_xml(b"<html>error</html>").is_err());
    }

    #[test]
    fn parses_mirror_json() {
        let body = br#"{"Date":"2026-11-07T11:30:00+03:00","PreviousDate":"2026-11-06T11:30:00+03:00","Valute":{"USD":{"ID":"R01235","NumCode":"840","CharCode":"USD","Nominal":1,"Name":"X","Value":81.5,"Previous":81.2},"KZT":{"ID":"R01335","NumCode":"398","CharCode":"KZT","Nominal":100,"Name":"Y","Value":17.4,"Previous":17.3}}}"#;
        let c = parse_json(body).unwrap();
        assert_eq!(c.date, "2026-11-07");
        assert!((c.rates["USD"] - 81.5).abs() < 1e-9);
        assert!((c.rates["KZT"] - 0.174).abs() < 1e-9);
        assert!(parse_json(b"{}").is_err());
        assert!(parse_json(b"not json").is_err());
    }
}
