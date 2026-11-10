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
}

/// Что случилось с облаком — от этого зависит подсказка на экране.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudFail {
    /// Вход устарел или доступ отозван — войти заново.
    Auth,
    /// На диске нет места.
    Space,
    /// Нет связи с облаком.
    Offline,
    /// Облако ответило ошибкой — обычно временной.
    Server,
    /// Данные «Кармана» в облаке удалены.
    Gone,
    /// Облако подключили заново, с другим ключом.
    Replaced,
    /// Записи этого устройства в облаке расходятся с базой.
    Conflict,
    /// Файл записан более новой версией «Кармана».
    Newer,
    /// Файл или ответ облака не разобрать.
    Format,
    /// Обмен остановлен.
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn format_err(msg: impl Into<String>) -> Error {
    Error::Format(msg.into())
}
