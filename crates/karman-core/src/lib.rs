//! Ядро «Кармана».
//!
//! Здесь живёт всё, что не зависит от экрана: деньги и даты, расчёт командировок
//! по правилам компании, зашифрованная база, журнал событий для синхронизации
//! и разбор QR-кодов (связь устройств и кассовые чеки).

pub mod ai;
pub mod backup;
pub mod book;
pub mod budget;
pub mod calendar;
pub mod category;
pub mod cbr;
pub mod cloud;
pub mod date;
pub mod db;
pub mod demo;
pub mod error;
pub mod export;
pub mod finance;
pub mod insights;
pub mod lan;
pub mod mcp;
pub mod money;
pub mod overtime;
pub mod pairing;
pub mod pdf;
pub mod receipt;
pub mod savings;
pub mod stats;
pub mod sync;
pub mod tools;
pub mod trip;
pub mod xlsx;
pub mod zip;

/// Как «Карман» называется в сообщениях — для общего модуля kit (копии, коннектор, вход).
pub const NAMES: kit_core::AppNames = kit_core::AppNames { id: "karman", name: "Карман", name_acc: "Карман", name_gen: "Кармана", feminine: false };

pub use date::Date;
pub use error::{Error, Result};
pub use money::Kop;
