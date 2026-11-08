//! Связь двух устройств по QR-коду.
//!
//! Одно устройство показывает QR со своим адресом в сети и одноразовым ключом,
//! другое сканирует его и подключается. Все пакеты шифруются этим ключом.
//! Адреса в QR — только адреса домашней сети: QR-код чужого с адресом в интернете
//! не уведёт записи на чужой сервер.

use crate::error::{format_err, Error, Result};
use std::net::IpAddr;
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

/// Адрес домашней сети: частные диапазоны IPv4 (и точки доступа телефона), 100.64/10,
/// link-local и свой компьютер; для IPv6 — link-local и ULA. Имена хостов не подходят.
pub fn is_local_address(host: &str) -> bool {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            let [a, b, ..] = v4.octets();
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        Ok(IpAddr::V6(v6)) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xffc0) == 0xfe80 || (first & 0xfe00) == 0xfc00
        }
        Err(_) => false,
    }
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
    fn rejects_foreign_codes() {
        assert!(Pairing::from_uri("t=20261031T1532&s=600.00").is_err());
        assert!(Pairing::from_uri("karman://pair?v=2&d=x").is_err());
        assert!(Pairing::from_uri("karman://pair?v=1&d=x&h=&p=1&k=AAAA").is_err());
    }
}
