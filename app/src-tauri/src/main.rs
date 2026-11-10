// Без лишнего окна консоли в Windows-сборке.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Приложение Claude запускает «Карман» с `--mcp`: это посредник коннектора, без окна.
    if std::env::args().nth(1).as_deref() == Some("--mcp") {
        std::process::exit(karman_app_lib::mcp_relay());
    }
    karman_app_lib::run()
}
