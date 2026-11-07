//! Файл Excel (.xlsx) без сторонних библиотек: XLSX — это ZIP с XML-частями.
//! Строки пишутся прямо в ячейки (inlineStr), суммы — числами с двумя знаками,
//! даты — датами Excel. Первая строка закреплена и с фильтром.

use crate::date::Date;
use crate::money::Kop;

pub enum Cell {
    Text(String),
    /// Сумма в копейках — в ячейке рубли с двумя знаками.
    Money(Kop),
    Date(Date),
    Int(i64),
    Empty,
}

pub struct Sheet {
    pub name: String,
    /// Ширина колонок в символах.
    pub widths: Vec<f64>,
    /// Первая строка — заголовки. Пустая строка отделяет итог: фильтр и сортировка его не трогают.
    pub rows: Vec<Vec<Cell>>,
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Управляющие символы XML не допускает — без них файл не откроется.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn col(i: usize) -> String {
    let mut n = i + 1;
    let mut s = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.push(b'A' + r as u8);
        n = (n - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Дата Excel: дней от 30 декабря 1899 года.
fn serial(d: Date) -> i64 {
    Date::from_epoch_days(0).days_until(d) as i64 + 25_569
}

fn sheet_xml(s: &Sheet) -> String {
    let mut x = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>"#);
    if !s.widths.is_empty() {
        x.push_str("<cols>");
        for (i, w) in s.widths.iter().enumerate() {
            x.push_str(&format!(r#"<col min="{n}" max="{n}" width="{w}" customWidth="1"/>"#, n = i + 1));
        }
        x.push_str("</cols>");
    }
    x.push_str("<sheetData>");
    for (r, row) in s.rows.iter().enumerate() {
        x.push_str(&format!(r#"<row r="{}">"#, r + 1));
        for (c, cell) in row.iter().enumerate() {
            let at = format!("{}{}", col(c), r + 1);
            let head = if r == 0 { r#" s="3""# } else { "" };
            match cell {
                Cell::Text(t) => x.push_str(&format!(r#"<c r="{at}" t="inlineStr"{head}><is><t xml:space="preserve">{}</t></is></c>"#, esc(t))),
                Cell::Money(k) => x.push_str(&format!(r#"<c r="{at}" s="1"><v>{}</v></c>"#, *k as f64 / 100.0)),
                Cell::Date(d) => x.push_str(&format!(r#"<c r="{at}" s="2"><v>{}</v></c>"#, serial(*d))),
                Cell::Int(n) => x.push_str(&format!(r#"<c r="{at}"><v>{n}</v></c>"#)),
                Cell::Empty => {}
            }
        }
        x.push_str("</row>");
    }
    x.push_str("</sheetData>");
    // Фильтр — на таблицу до первой пустой строки (ниже — итог).
    let data = s.rows.iter().position(|r| r.iter().all(|c| matches!(c, Cell::Empty))).unwrap_or(s.rows.len());
    let width = s.rows[..data].iter().map(|r| r.len()).max().unwrap_or(0);
    if width > 0 && data > 1 {
        x.push_str(&format!(r#"<autoFilter ref="A1:{}{}"/>"#, col(width - 1), data));
    }
    x.push_str("</worksheet>");
    x
}

const STYLES: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="#,##0.00"/></numFmts><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="4"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"##;

pub fn build(sheets: &[Sheet]) -> std::io::Result<Vec<u8>> {
    let mut types = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#);
    let mut book = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#);
    let mut rels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (i, s) in sheets.iter().enumerate() {
        let n = i + 1;
        types.push_str(&format!(r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#));
        // Имя листа: до 31 символа, без []:*?/\
        let name: String = s.name.chars().filter(|c| !"[]:*?/\\".contains(*c)).take(31).collect();
        book.push_str(&format!(r#"<sheet name="{}" sheetId="{n}" r:id="rId{n}"/>"#, esc(&name)));
        rels.push_str(&format!(r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#));
        files.push((format!("xl/worksheets/sheet{n}.xml"), sheet_xml(s).into_bytes()));
    }
    types.push_str("</Types>");
    book.push_str("</sheets></workbook>");
    rels.push_str(&format!(r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#, sheets.len() + 1));
    let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let mut all = vec![
        ("[Content_Types].xml".to_string(), types.into_bytes()),
        ("_rels/.rels".to_string(), root.as_bytes().to_vec()),
        ("xl/workbook.xml".to_string(), book.into_bytes()),
        ("xl/_rels/workbook.xml.rels".to_string(), rels.into_bytes()),
        ("xl/styles.xml".to_string(), STYLES.as_bytes().to_vec()),
    ];
    all.extend(files);
    crate::zip::build(&all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_dates_and_escaping() {
        assert_eq!((col(0).as_str(), col(25).as_str(), col(26).as_str()), ("A", "Z", "AA"));
        assert_eq!(serial(Date::parse("2026-11-07").unwrap()), 46_333);
        assert_eq!(esc("a<b & \"c\"\u{1}"), "a&lt;b &amp; &quot;c&quot; ");
    }

    #[test]
    fn total_below_an_empty_row_stays_out_of_the_filter() {
        let sheet = Sheet {
            name: "Итог".into(),
            widths: vec![],
            rows: vec![vec![Cell::Text("Категория".into()), Cell::Text("Сумма".into())], vec![Cell::Text("Кафе".into()), Cell::Money(100)], vec![], vec![Cell::Text("Всего".into()), Cell::Money(100)]],
        };
        let xml = sheet_xml(&sheet);
        assert!(xml.contains(r#"<autoFilter ref="A1:B2"/>"#) && xml.contains(r#"<row r="3"></row>"#) && xml.contains("Всего"), "{xml}");
    }

    #[test]
    fn builds_a_zip_excel_can_read() {
        let sheet = Sheet {
            name: "Операции".into(),
            widths: vec![12.0, 30.0, 14.0],
            rows: vec![
                vec![Cell::Text("Дата".into()), Cell::Text("Операция".into()), Cell::Text("Сумма, ₽".into())],
                vec![Cell::Date(Date::parse("2026-11-07").unwrap()), Cell::Text("Кофе & булка".into()), Cell::Money(-350_50)],
            ],
        };
        let xml = sheet_xml(&sheet);
        assert!(xml.contains(r#"<autoFilter ref="A1:C2"/>"#), "{xml}");
        let bytes = build(&[sheet]).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xlsx");
        std::fs::write(&path, &bytes).unwrap();
        // Если есть unzip — архив цел и части на месте.
        if let Ok(out) = std::process::Command::new("unzip").arg("-l").arg(&path).output() {
            let list = String::from_utf8_lossy(&out.stdout);
            for part in ["[Content_Types].xml", "xl/workbook.xml", "xl/worksheets/sheet1.xml", "xl/styles.xml"] {
                assert!(list.contains(part), "нет {part}: {list}");
            }
        }
    }
}
