use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("база данных: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("неверный ключ базы или файл повреждён")]
    WrongKey,
    #[error("данные: {0}")]
    Json(#[from] serde_json::Error),
    #[error("сеть: {0}")]
    Io(#[from] std::io::Error),
    #[error("пакет не расшифровался: ключ не подходит или данные изменены")]
    Decrypt,
    #[error("неверный формат: {0}")]
    Format(String),
    #[error("генератор случайных чисел: {0}")]
    Random(String),
    /// Другое устройство не смогло принять записи (старая версия).
    #[error("{0}")]
    Peer(String),
    /// Резервная копия: файл не подходит, повреждён или обрезан.
    #[error("{0}")]
    Backup(String),
    #[error("Пароль не подходит к этой копии")]
    BackupPassword,
    /// Облако: что случилось и что сказать.
    #[error("{1}")]
    Cloud(CloudFail, String),
    #[error("Пароль не подходит: облако зашифровано другим паролем копий")]
    CloudPassword,
    /// Связь по Wi-Fi: что случилось и что сказать.
    #[error("{1}")]
    Lan(LanFail, String),
}

/// Почему не получилась связь по Wi-Fi — из общего модуля kit.
pub use kit_core::lan::LanFail;

/// Что случилось с облаком — от этого зависит подсказка на экране (из общего модуля kit).
pub use kit_core::cloud::CloudFail;

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn format_err(msg: impl Into<String>) -> Error {
    Error::Format(msg.into())
}

/// Ошибки общего модуля kit — теми же словами, что и раньше в «Кармане».
impl From<kit_core::Error> for Error {
    fn from(e: kit_core::Error) -> Self {
        use kit_core::Error as K;
        match e {
            K::Backup(s) => Error::Backup(s),
            K::BackupPassword => Error::BackupPassword,
            K::Random(s) => Error::Random(s),
            K::Io(e) => Error::Io(e),
            K::Json(e) => Error::Json(e),
            K::Format(s) => Error::Format(s),
            K::Store(s) | K::Peer(s) => Error::Peer(s),
            K::Decrypt => Error::Decrypt,
            K::Lan(f, s) => Error::Lan(f, s),
            K::Cloud(f, s) => Error::Cloud(f, s),
            K::CloudPassword => Error::CloudPassword,
            other => Error::Format(other.to_string()),
        }
    }
}
