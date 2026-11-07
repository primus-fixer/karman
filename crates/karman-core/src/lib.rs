//! Ядро «Кармана».
//!
//! Здесь живёт всё, что не зависит от экрана: деньги и даты, расчёт командировок
//! по правилам компании, зашифрованная база, журнал событий для синхронизации
//! и разбор QR-кодов (связь устройств и кассовые чеки).

pub mod book;
pub mod budget;
pub mod cbr;
pub mod date;
pub mod db;
pub mod demo;
pub mod error;
pub mod export;
pub mod finance;
pub mod money;
pub mod pairing;
pub mod pin;
pub mod receipt;
pub mod savings;
pub mod stats;
pub mod sync;
pub mod trip;
pub mod xlsx;
pub mod zip;

pub use date::Date;
pub use error::{Error, Result};
pub use money::Kop;
