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

/// Почему не получилась связь по Wi-Fi — чтобы приложение могло ответить по делу.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanFail {
    /// На том устройстве код не показан, истёк или уже использован.
    NoCode,
    /// Код не подошёл; сколько попыток осталось.
    WrongCode(u8),
    /// Код отменён после пяти неверных попыток.
    Cancelled,
    /// Устройство не узнало нас: связь отвязана или её не было.
    Unknown,
    /// Устройство занято другим обменом.
    Busy,
    /// Ответило не то устройство: оно не знает кода или ключа пары.
    NotPeer,
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
            other => Error::Format(other.to_string()),
        }
    }
}
