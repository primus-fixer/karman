// Без лишнего окна консоли в Windows-сборке.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    karman_app_lib::run()
}
