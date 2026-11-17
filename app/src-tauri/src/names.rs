//! Чем «Карман» отличается от других приложений на общем модуле kit: название для сообщений
//! и строки, которыми ключи привязаны к «Карману».
//!
//! **Строки ключей менять нельзя.** С другими строками система не расшифрует ключ базы, пароль
//! облака, ключ API и ключи связи, которые уже лежат на устройствах, — «Карман» не откроет свою базу.

use kit_core::AppNames;
use kit_secure::KeyNames;

pub const APP: AppNames = karman_core::NAMES;

pub const KEYS: KeyNames = KeyNames { dpapi_entropy: b"karman-db-key-v1", dpapi_label: "Karman", keystore_alias: "karman-db-key" };

/// Шифрует секрет средствами системы: DPAPI на Windows, Android Keystore на телефоне;
/// на Linux (разработка) секрет остаётся как есть.
pub fn protect(secret: &[u8]) -> kit_secure::Result<Vec<u8>> {
    kit_secure::protect(&KEYS, secret)
}

pub fn unprotect(blob: &[u8]) -> kit_secure::Result<Vec<u8>> {
    kit_secure::unprotect(&KEYS, blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Строки — те же, что были в «Кармане» до общего модуля (0.15.1). Проверка падает, если их
    /// поменять: тогда на устройствах перестанут открываться база, облако, ключ API и связанные
    /// устройства.
    #[test]
    fn key_names_are_the_same_as_before_kit() {
        assert_eq!(KEYS.dpapi_entropy, b"karman-db-key-v1");
        assert_eq!(KEYS.dpapi_label, "Karman");
        assert_eq!(KEYS.keystore_alias, "karman-db-key");
        // Имена переменных окружения для сквозных проверок — тоже прежние.
        assert_eq!(APP.env("LOCK_AFTER_SECS"), "KARMAN_LOCK_AFTER_SECS");
        assert_eq!(APP.env("FAKE_AUTH"), "KARMAN_FAKE_AUTH");
        assert_eq!(APP.env("FAKE_DEVICE"), "KARMAN_FAKE_DEVICE");
    }

    /// Тексты запросов отпечатка и Windows Hello — как были: «Вход в „Карман“», «Сброс PIN-кода „Кармана“».
    #[test]
    fn names_in_messages() {
        assert_eq!(format!("Вход в «{}»", APP.name_acc), "Вход в «Карман»");
        assert_eq!(format!("Сброс PIN-кода «{}»", APP.name_gen), "Сброс PIN-кода «Кармана»");
        assert_eq!(APP.name, "Карман");
    }

    #[test]
    fn secret_roundtrip() {
        let blob = protect(&[7u8; 32]).unwrap();
        assert_eq!(unprotect(&blob).unwrap(), [7u8; 32]);
    }
}
