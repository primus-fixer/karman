//! QR-код кассового чека (54-ФЗ): `t=20261031T1532&s=600.00&fn=…&i=…&fp=…&n=1`.

use crate::date::Date;
use crate::error::{format_err, Result};
use crate::money::{parse, Kop};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct FiscalReceipt {
    pub date: Date,
    /// «15:32»
    pub time: String,
    pub sum: Kop,
    /// Номер фискального накопителя.
    pub fn_number: String,
    /// Номер фискального документа.
    pub fd: String,
    /// Фискальный признак документа.
    pub fp: String,
    /// 1 — приход, 2 — возврат прихода, 3 — расход, 4 — возврат расхода.
    pub kind: u8,
}

impl FiscalReceipt {
    pub fn kind_title(&self) -> &'static str {
        match self.kind {
            1 => "приход",
            2 => "возврат прихода",
            3 => "расход",
            4 => "возврат расхода",
            _ => "неизвестный тип",
        }
    }
}

pub fn parse_fiscal_qr(text: &str) -> Result<FiscalReceipt> {
    let mut t = None;
    let mut s = None;
    let mut fn_number = None;
    let mut fd = None;
    let mut fp = None;
    let mut kind = 1u8;
    for pair in text.trim().split('&') {
        let (k, v) = pair.split_once('=').ok_or_else(|| format_err("это не QR-код чека"))?;
        match k {
            "t" => t = Some(v.to_string()),
            "s" => s = Some(v.to_string()),
            "fn" => fn_number = Some(v.to_string()),
            "i" => fd = Some(v.to_string()),
            "fp" => fp = Some(v.to_string()),
            "n" => kind = v.parse().map_err(|_| format_err("тип чека не число"))?,
            _ => {}
        }
    }
    let t = t.ok_or_else(|| format_err("в чеке нет даты"))?;
    let (day, clock) = t.split_once('T').ok_or_else(|| format_err("дата чека без времени"))?;
    if day.len() != 8 || clock.len() < 4 || !day.chars().all(|c| c.is_ascii_digit()) || !clock.chars().all(|c| c.is_ascii_digit()) {
        return Err(format_err("дата чека в неизвестном формате"));
    }
    let date = Date::new(day[0..4].parse().unwrap(), day[4..6].parse().unwrap(), day[6..8].parse().unwrap())
        .ok_or_else(|| format_err("в чеке несуществующая дата"))?;
    let time = format!("{}:{}", &clock[0..2], &clock[2..4]);
    let sum = parse(&s.ok_or_else(|| format_err("в чеке нет суммы"))?).ok_or_else(|| format_err("сумма чека не число"))?;
    Ok(FiscalReceipt {
        date,
        time,
        sum,
        fn_number: fn_number.ok_or_else(|| format_err("в чеке нет номера ФН"))?,
        fd: fd.ok_or_else(|| format_err("в чеке нет номера документа"))?,
        fp: fp.ok_or_else(|| format_err("в чеке нет фискального признака"))?,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::rub;

    #[test]
    fn parses_receipt_qr() {
        let r = parse_fiscal_qr("t=20261031T1532&s=600.00&fn=7380440700076549&i=41218&fp=2489764517&n=1").unwrap();
        assert_eq!(r.date.to_string(), "2026-10-31");
        assert_eq!(r.time, "15:32");
        assert_eq!(r.sum, rub(600.0));
        assert_eq!(r.fn_number, "7380440700076549");
        assert_eq!(r.fd, "41218");
        assert_eq!(r.fp, "2489764517");
        assert_eq!(r.kind_title(), "приход");
        let r2 = parse_fiscal_qr("t=20261106T091400&s=1386&fn=1&i=2&fp=3&n=2").unwrap();
        assert_eq!(r2.time, "09:14");
        assert_eq!(r2.sum, rub(1386.0));
        assert_eq!(r2.kind, 2);
    }

    #[test]
    fn rejects_other_qr_codes() {
        assert!(parse_fiscal_qr("karman://pair?v=1").is_err());
        assert!(parse_fiscal_qr("t=2026&s=1").is_err());
        assert!(parse_fiscal_qr("https://example.com").is_err());
    }
}
