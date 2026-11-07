//! Ядро «Кармана».
//!
//! Здесь живёт всё, что не зависит от экрана: деньги и даты, расчёт командировок
//! по правилам компании, зашифрованная база, журнал событий для синхронизации
//! и разбор QR-кодов (связь устройств и кассовые чеки).

pub mod book;
pub mod date;
pub mod db;
pub mod demo;
pub mod error;
pub mod money;
pub mod pairing;
pub mod pin;
pub mod receipt;
pub mod sync;
pub mod trip;
pub mod zip;

pub use date::Date;
pub use error::{Error, Result};
pub use money::Kop;
