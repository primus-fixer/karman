//! PDF из снимков: каждый снимок JPEG — отдельная страница A4, снимок вписан в страницу.
//! JPEG кладётся в PDF как есть (DCTDecode) — без пересжатия и сторонних библиотек.

use crate::error::{Error, Result};

/// Размеры и цвет снимка JPEG: ширина, высота, число цветовых каналов.
fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        // Заполнители и маркеры без длины.
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if len < 2 || i + 2 + len > data.len() {
            return None;
        }
        // SOF0–SOF15, кроме DHT (C4), JPG (C8) и DAC (CC): там размеры кадра.
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            let p = &data[i + 4..i + 2 + len];
            if p.len() < 6 {
                return None;
            }
            let h = u16::from_be_bytes([p[1], p[2]]) as u32;
            let w = u16::from_be_bytes([p[3], p[4]]) as u32;
            return (w > 0 && h > 0).then_some((w, h, p[5]));
        }
        i += 2 + len;
    }
    None
}

/// Страница A4 в пунктах: портрет для вертикальных снимков, альбом — для горизонтальных.
const A4: (f64, f64) = (595.28, 841.89);
const MARGIN: f64 = 18.0;

/// Собирает PDF: по странице на снимок.
pub fn from_jpegs(images: &[&[u8]]) -> Result<Vec<u8>> {
    if images.is_empty() {
        return Err(Error::Format("нет снимков для PDF".into()));
    }
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    // 1 — каталог, 2 — страницы; дальше на каждую страницу: страница, содержимое, снимок.
    let n = images.len();
    let mut objects: Vec<Vec<u8>> = Vec::with_capacity(2 + 3 * n);
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 3 * i)).collect();
    objects.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes());
    for (i, img) in images.iter().enumerate() {
        let (w, h, comps) = jpeg_info(img).ok_or_else(|| Error::Format(format!("снимок {} не похож на JPEG", i + 1)))?;
        let color = match comps {
            1 => "/DeviceGray",
            4 => "/DeviceCMYK",
            _ => "/DeviceRGB",
        };
        let (pw, ph) = if w > h { (A4.1, A4.0) } else { A4 };
        let scale = ((pw - 2.0 * MARGIN) / w as f64).min((ph - 2.0 * MARGIN) / h as f64);
        let (dw, dh) = (w as f64 * scale, h as f64 * scale);
        let (x, y) = ((pw - dw) / 2.0, (ph - dh) / 2.0);
        let (content, image) = (4 + 3 * i, 5 + 3 * i);
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im{i} {image} 0 R >> >> /Contents {content} 0 R >>").into_bytes());
        let ops = format!("q {dw:.2} 0 0 {dh:.2} {x:.2} {y:.2} cm /Im{i} Do Q");
        objects.push(format!("<< /Length {} >>\nstream\n{ops}\nendstream", ops.len()).into_bytes());
        let mut im = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {color} /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", img.len()).into_bytes();
        im.extend_from_slice(img);
        im.extend_from_slice(b"\nendstream");
        objects.push(im);
    }
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Маленький «JPEG»: начало файла и кадр SOF0 с размерами — для проверки разметки PDF.
    fn fake_jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut d = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46];
        d.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        d.extend_from_slice(&h.to_be_bytes());
        d.extend_from_slice(&w.to_be_bytes());
        d.extend_from_slice(&[0x03, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        d.extend_from_slice(&[0xFF, 0xD9]);
        d
    }

    #[test]
    fn reads_jpeg_size() {
        assert_eq!(jpeg_info(&fake_jpeg(1600, 1200)), Some((1600, 1200, 3)));
        assert_eq!(jpeg_info(b"%PDF-1.4"), None);
        assert_eq!(jpeg_info(&[0xFF, 0xD8, 0xFF, 0xC0, 0x00]), None);
    }

    #[test]
    fn pages_and_cross_reference_table() {
        let a = fake_jpeg(1200, 1600);
        let b = fake_jpeg(1600, 1200);
        let pdf = from_jpegs(&[&a, &b]).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4") && text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Count 2") && text.contains("/Width 1200 /Height 1600") && text.contains("/MediaBox [0 0 841.89 595.28]"), "второй снимок — альбомная страница");
        // Смещения в таблице указывают на начала объектов (таблица и хвост — ASCII).
        let tail = std::str::from_utf8(&pdf[pdf.len() - 40..]).unwrap();
        let xref_at: usize = tail.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        let table = std::str::from_utf8(&pdf[xref_at..]).unwrap();
        for (k, line) in table.lines().skip(3).take(8).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{} 0 obj", k + 1).as_bytes()), "объект {}", k + 1);
        }
        assert!(from_jpegs(&[]).is_err());
        assert!(matches!(from_jpegs(&[b"not a jpeg"]), Err(Error::Format(m)) if m.contains("JPEG")));
    }
}
