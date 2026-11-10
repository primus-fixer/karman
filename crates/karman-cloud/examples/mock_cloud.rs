//! Имитация Яндекс Диска и Google Drive для сквозных проверок «Кармана».
//!
//! `cargo run -p karman-cloud --features mock --example mock_cloud -- 18080`
//! Приложение в отладочной сборке ходит сюда, если задано `KARMAN_CLOUD_MOCK=http://127.0.0.1:18080`.

fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    let mock = karman_cloud::mock::Mock::start_on(port).expect("порт занят");
    // Коды входа «вводятся» сами; отказ, истёкший вход и нехватку места включают через /mock/....
    mock.approve(true);
    println!("{}", mock.base);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
