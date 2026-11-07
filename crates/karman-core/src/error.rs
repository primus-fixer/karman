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
    #[error("PIN-код: {0}")]
    Pin(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn format_err(msg: impl Into<String>) -> Error {
    Error::Format(msg.into())
}
