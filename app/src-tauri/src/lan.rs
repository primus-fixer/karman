//! Связь по Wi-Fi (0.15): связанные устройства, код связи, поиск в сети и обмен в фоне.
//!
//! С 0.16 всё это — общее, из kit (`kit_tauri::wifi`), то же, что у «Полки»: команды экрана, фоновый
//! обмен и хранение связанных устройств. Своё у «Кармана» — настройки связи (`karman_core::lan::LAN`),
//! база, строки ключей для хранилища системы и разовый обмен по QR-коду прежней версии (до 0.15).

use crate::{err, AppState, Res};
use karman_core::db::Store;
use karman_core::pairing::{self, Qr};
use kit_tauri::wifi::PairResult;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

kit_tauri::wifi_commands!(Store; no_qr);

/// Связь по Wi-Fi в приложении — состояние для `app.manage`.
pub type Wifi = kit_tauri::wifi::Wifi<Store>;

/// Фон: открыть порты, когда нужно, находить связанные устройства и обмениваться.
pub fn start(app: AppHandle) {
    kit_tauri::wifi::start::<_, Store>(app);
}

/// «Карман» заблокирован — код больше не действует. Обмен со связанными продолжается, как с облаком.
pub fn on_lock(app: &AppHandle) {
    kit_tauri::wifi::on_lock::<_, Store>(app);
}

#[derive(Clone, Serialize)]
struct LanEvent {
    received: usize,
    paired: Option<String>,
}

/// Ошибка ядра — предложением: без «неверный формат:», с большой буквы.
fn sentence(e: &karman_core::Error) -> String {
    let m = match e {
        karman_core::Error::Format(m) => m.clone(),
        e => e.to_string(),
    };
    let mut c = m.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => m,
    }
}

/// Что сказать после обмена по QR-коду прежней версии: там записи новых видов не видны.
fn legacy_hint(peer_app: &str) -> String {
    let mine = env!("CARGO_PKG_VERSION");
    if peer_app.is_empty() {
        return format!("На другом устройстве «Карман» старше 0.5: переводы и новые записи там не видны, пока его не обновите до {mine}.");
    }
    kit_tauri::wifi::version_hint(mine, peer_app)
}

/// QR-код с другого устройства: с 0.15 — адрес и код (как у всех приложений на kit);
/// QR прежней версии — разовый обмен по ключу из QR.
#[tauri::command]
pub async fn lan_pair_qr(app: AppHandle, uri: String) -> Res<PairResult> {
    let state = app.state::<AppState>();
    state.unlocked()?;
    let Qr::Legacy(p) = pairing::parse_qr(&uri).map_err(|e| sentence(&e))? else {
        return kit_tauri::wifi::pair_qr::<_, Store>(app.clone(), uri).await;
    };
    if p.device == state.me.id {
        return Err("Это QR-код этого же устройства. Отсканируйте его другим устройством".into());
    }
    let store = state.store.clone();
    let name = state.me.name.clone();
    let rep = tauri::async_runtime::spawn_blocking(move || karman_core::sync::sync_with(&p, &store, &name)).await.map_err(err)?.map_err(|e| match e {
        karman_core::Error::Peer(m) => m,
        e => format!("Не удалось связаться: {e}. Устройства должны быть в одной сети Wi-Fi"),
    })?;
    let mut hint = legacy_hint(&rep.peer_app);
    if !hint.is_empty() {
        hint.push(' ');
    }
    hint.push_str("Это QR-код прежней версии: обмен прошёл один раз, устройства друг друга не запомнили.");
    let _ = app.emit("lan", LanEvent { received: rep.received + rep.files_received, paired: None });
    Ok(PairResult { peer_name: rep.peer_name, sent: rep.sent, received: rep.received, files_sent: rep.files_sent, files_received: rep.files_received, hint, remembered: false })
}
