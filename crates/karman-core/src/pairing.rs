//! Связь двух устройств: код связи и QR-код.
//!
//! С 0.15 одно устройство показывает код из восьми знаков (и QR-код с тем же кодом и своим адресом),
//! другое вводит код или сканирует QR. Из кода оба получают общий ключ по SPAKE2 ([`crate::lan`]):
//! сам код по сети не передаётся, а подобрать его можно только попытками на самом устройстве — их пять.
//!
//! До 0.15 QR-код нёс сам ключ (`v=1`, [`Pairing`]) — такой QR от прежней версии ещё читается.
//! Адреса в QR — только адреса домашней сети: QR-код чужого с адресом в интернете
//! не уведёт записи на чужой сервер.

use crate::error::{format_err, Error, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};

pub const SCHEME: &str = "karman://pair?";

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Pairing {
    pub device: String,
    pub name: String,
    pub hosts: Vec<String>,
    pub port: u16,
    pub key: [u8; 32],
}

pub use kit_core::lan::{is_local_address, show_code, CODE_ALPHABET, CODE_LEN};

/// Новый код связи: восемь случайных знаков (из общего модуля kit).
pub fn new_code() -> Result<String> {
    Ok(kit_core::lan::new_code()?)
}

/// Код, как его ввёл человек, — к виду «6WVGHEWK» (из общего модуля kit): без пробелов и дефисов,
/// заглавными, русские буквы, похожие на латинские, — латинскими.
pub fn normalize_code(input: &str) -> Result<String> {
    Ok(kit_core::lan::normalize_code(input)?)
}

/// QR-код связи с 0.15: адрес устройства в сети и код связи (из общего модуля kit, `karman://pair?v=2&…`).
pub use kit_core::lan::Invite;

/// Что внутри QR-кода связи.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Qr {
    /// С 0.15: адрес и код связи.
    Invite(Invite),
    /// До 0.15: адрес и одноразовый ключ — обмен один раз, без запоминания.
    Legacy(Pairing),
}

/// Разбирает QR-код связи любой версии: версия 1 (до 0.15) — ключ, иначе — как у всех приложений на kit.
pub fn parse_qr(uri: &str) -> Result<Qr> {
    let t = uri.trim();
    if t.strip_prefix(SCHEME).is_some_and(|q| q.split('&').any(|p| p == "v=1")) {
        return Pairing::from_uri(t).map(Qr::Legacy);
    }
    Ok(Qr::Invite(Invite::parse(&crate::lan::LAN, t)?))
}

pub fn random_key() -> Result<[u8; 32]> {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).map_err(|e| Error::Random(e.to_string()))?;
    Ok(k)
}

impl Pairing {
    pub fn to_uri(&self) -> String {
        format!(
            "{SCHEME}v=1&d={}&n={}&h={}&p={}&k={}",
            self.device,
            B64.encode(self.name.as_bytes()),
            self.hosts.join(","),
            self.port,
            B64.encode(self.key)
        )
    }

    pub fn from_uri(uri: &str) -> Result<Pairing> {
        let q = uri.trim().strip_prefix(SCHEME).ok_or_else(|| format_err("это не QR-код «Кармана» для связи"))?;
        let mut device = None;
        let mut name = None;
        let mut hosts = None;
        let mut port = None;
        let mut key = None;
        for part in q.split('&') {
            let (k, v) = part.split_once('=').ok_or_else(|| format_err("испорченный QR-код связи"))?;
            match k {
                "v" if v != "1" => return Err(format_err("QR-код от более новой версии приложения")),
                "d" => device = Some(v.to_string()),
                "n" => {
                    let raw = B64.decode(v).map_err(|_| format_err("имя устройства не читается"))?;
                    name = Some(String::from_utf8(raw).map_err(|_| format_err("имя устройства не читается"))?);
                }
                "h" => hosts = Some(v.split(',').filter(|h| !h.is_empty()).map(String::from).collect::<Vec<_>>()),
                "p" => port = Some(v.parse::<u16>().map_err(|_| format_err("порт не число"))?),
                "k" => {
                    let raw = B64.decode(v).map_err(|_| format_err("ключ не читается"))?;
                    let arr: [u8; 32] = raw.try_into().map_err(|_| format_err("ключ неверной длины"))?;
                    key = Some(arr);
                }
                _ => {}
            }
        }
        let hosts = hosts.ok_or_else(|| format_err("в QR нет адреса"))?;
        if hosts.is_empty() {
            return Err(format_err("в QR нет адреса"));
        }
        let hosts: Vec<String> = hosts.into_iter().filter(|h| is_local_address(h)).collect();
        if hosts.is_empty() {
            return Err(format_err("в QR-коде адрес не из домашней сети: связь — только между устройствами в одной сети Wi-Fi"));
        }
        Ok(Pairing {
            device: device.ok_or_else(|| format_err("в QR нет устройства"))?,
            name: name.unwrap_or_default(),
            hosts,
            port: port.ok_or_else(|| format_err("в QR нет порта"))?,
            key: key.ok_or_else(|| format_err("в QR нет ключа"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let p = Pairing {
            device: "a1b2".into(),
            name: "Ноутбук".into(),
            hosts: vec!["192.168.1.20".into(), "10.0.0.5".into()],
            port: 47_500,
            key: [7u8; 32],
        };
        let uri = p.to_uri();
        assert!(uri.starts_with("karman://pair?v=1&d=a1b2&"));
        assert_eq!(Pairing::from_uri(&uri).unwrap(), p);
    }

    #[test]
    fn only_home_network_addresses() {
        for ok in ["192.168.1.20", "10.0.0.5", "172.20.10.2", "192.168.43.1", "100.101.1.2", "169.254.3.4", "127.0.0.1", "fe80::1", "fd12:3456::1", "::1"] {
            assert!(is_local_address(ok), "{ok}");
        }
        for bad in ["8.8.8.8", "93.184.216.34", "172.32.0.1", "100.128.0.1", "evil.example.com", "localhost", "2001:db8::1", ""] {
            assert!(!is_local_address(bad), "{bad}");
        }
        // Адреса из интернета отбрасываются; если других нет — QR не принимается.
        let mut p = Pairing { device: "a".into(), name: "".into(), hosts: vec!["8.8.8.8".into(), "192.168.1.20".into()], port: 47_500, key: [1u8; 32] };
        assert_eq!(Pairing::from_uri(&p.to_uri()).unwrap().hosts, ["192.168.1.20"]);
        p.hosts = vec!["evil.example.com".into(), "93.184.216.34".into()];
        assert!(matches!(Pairing::from_uri(&p.to_uri()), Err(Error::Format(m)) if m.contains("не из домашней сети")));
    }

    #[test]
    fn codes_are_random_readable_and_forgiving() {
        let a = new_code().unwrap();
        let b = new_code().unwrap();
        assert_eq!(a.len(), CODE_LEN);
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| CODE_ALPHABET.contains(&c)), "{a}");
        assert_eq!(show_code("6WVGHEWK"), "6WVG-HEWK");
        assert_eq!(normalize_code(" 6wvg-hewk ").unwrap(), "6WVGHEWK");
        // С русской раскладки: «Н», «Е», «В», «К», «Р», «С», «Т», «М», «Х», «А» — как латинские.
        assert_eq!(normalize_code("НЕВК–РСТМ").unwrap(), "HEBKPCTM");
        assert_eq!(normalize_code("ха3у ксам").unwrap(), "XA3YKCAM");
        assert!(normalize_code("6WVG-HEW").unwrap_err().to_string().contains("восемь знаков"));
        assert!(normalize_code("6WVG-HEW0").unwrap_err().to_string().contains("«0»"), "нуля в коде не бывает");
        assert!(normalize_code("6WVG-HEWЖ").unwrap_err().to_string().contains("«Ж»"));
    }

    #[test]
    fn invite_qr_roundtrip_and_old_qr() {
        let inv = Invite { device: "a1b2".into(), name: "Ноутбук".into(), hosts: vec!["192.168.1.20".into(), "8.8.8.8".into()], port: 47_500, code: "6WVGHEWK".into() };
        let Qr::Invite(back) = parse_qr(&inv.to_uri(&crate::lan::LAN)).unwrap() else { panic!() };
        assert_eq!(back, Invite { hosts: vec!["192.168.1.20".into()], ..inv.clone() }, "адрес из интернета отброшен");
        let only_internet = Invite { hosts: vec!["8.8.8.8".into()], ..inv.clone() };
        assert!(parse_qr(&only_internet.to_uri(&crate::lan::LAN)).unwrap_err().to_string().contains("не из домашней сети"));
        let bad_code = inv.to_uri(&crate::lan::LAN).replace("c=6WVGHEWK", "c=123");
        assert!(parse_qr(&bad_code).is_err());
        let old = Pairing { device: "a".into(), name: "Ноутбук".into(), hosts: vec!["192.168.1.20".into()], port: 47_500, key: [3u8; 32] };
        assert!(matches!(parse_qr(&old.to_uri()).unwrap(), Qr::Legacy(p) if p == old), "QR прежней версии ещё читается");
        assert!(parse_qr("karman://pair?v=3&d=x").unwrap_err().to_string().contains("более новой версии"));
        assert!(parse_qr("https://example.com").is_err());
    }

    #[test]
    fn rejects_foreign_codes() {
        assert!(Pairing::from_uri("t=20261031T1532&s=600.00").is_err());
        assert!(Pairing::from_uri("karman://pair?v=2&d=x").is_err());
        assert!(Pairing::from_uri("karman://pair?v=1&d=x&h=&p=1&k=AAAA").is_err());
    }
}
