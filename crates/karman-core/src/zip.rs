//! Простой ZIP без сжатия: архив чеков для офиса. Снимки уже сжаты, поэтому
//! хранение «как есть» почти не проигрывает в размере. Имена файлов — в UTF-8
//! (флаг 11), чтобы Проводник Windows показывал русские названия.

use std::io::Write;

/// Время 1 января 2026, 12:00 в формате DOS — дата у всех файлов одна.
const DOS_TIME: u16 = 12 << 11;
const DOS_DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1;
const UTF8: u16 = 1 << 11;

pub fn build(files: &[(String, Vec<u8>)]) -> std::io::Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let size = data.len() as u32;
        let name = name.as_bytes();
        // Локальный заголовок.
        out.write_all(&0x0403_4b50u32.to_le_bytes())?;
        out.write_all(&20u16.to_le_bytes())?; // версия для распаковки
        out.write_all(&UTF8.to_le_bytes())?;
        out.write_all(&0u16.to_le_bytes())?; // без сжатия
        out.write_all(&DOS_TIME.to_le_bytes())?;
        out.write_all(&DOS_DATE.to_le_bytes())?;
        out.write_all(&crc.to_le_bytes())?;
        out.write_all(&size.to_le_bytes())?;
        out.write_all(&size.to_le_bytes())?;
        out.write_all(&(name.len() as u16).to_le_bytes())?;
        out.write_all(&0u16.to_le_bytes())?;
        out.write_all(name)?;
        out.write_all(data)?;
        // Запись в оглавлении.
        central.write_all(&0x0201_4b50u32.to_le_bytes())?;
        central.write_all(&20u16.to_le_bytes())?; // кем создан
        central.write_all(&20u16.to_le_bytes())?;
        central.write_all(&UTF8.to_le_bytes())?;
        central.write_all(&0u16.to_le_bytes())?;
        central.write_all(&DOS_TIME.to_le_bytes())?;
        central.write_all(&DOS_DATE.to_le_bytes())?;
        central.write_all(&crc.to_le_bytes())?;
        central.write_all(&size.to_le_bytes())?;
        central.write_all(&size.to_le_bytes())?;
        central.write_all(&(name.len() as u16).to_le_bytes())?;
        central.write_all(&[0u8; 12])?; // доп. поле, комментарий, диск, атрибуты
        central.write_all(&offset.to_le_bytes())?;
        central.write_all(name)?;
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.write_all(&0x0605_4b50u32.to_le_bytes())?;
    out.write_all(&[0u8; 4])?;
    out.write_all(&(files.len() as u16).to_le_bytes())?;
    out.write_all(&(files.len() as u16).to_le_bytes())?;
    out.write_all(&(central.len() as u32).to_le_bytes())?;
    out.write_all(&start.to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?;
    Ok(out)
}

/// Имя файла без символов, которые Windows не разрешает.
pub fn safe_name(s: &str) -> String {
    let t: String = s.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(80).collect();
    if t.is_empty() { "файл".into() } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_opens_with_unzip() {
        let files = vec![("2026-11-05 Такси 640,00.jpg".to_string(), vec![0xFF, 0xD8, 0xFF, 1, 2, 3]), ("билет.pdf".to_string(), b"%PDF-1.7".to_vec())];
        let zip = build(&files).unwrap();
        assert_eq!(&zip[..4], b"PK\x03\x04");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.zip");
        std::fs::write(&path, &zip).unwrap();
        // Если в системе есть unzip — проверяем им, иначе хотя бы структуру.
        if let Ok(out) = std::process::Command::new("unzip").arg("-t").arg(&path).output() {
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{text}");
            assert!(text.contains("No errors"), "{text}");
        }
        assert_eq!(safe_name("Такси: «вокзал» / объект?"), "Такси «вокзал» объект");
    }
}
