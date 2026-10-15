//! A minimal Excel workbook: one sheet of rows, written without a library.
//!
//! The file is a zip of five XML parts, stored without compression. Every
//! text cell is an inline string, never a formula, so a transcript that starts
//! with `=` stays text.

/// A cell of the sheet.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Number(f64),
}

/// The workbook's bytes: a header row, then the rows.
pub fn workbook(sheet_name: &str, header: &[&str], rows: &[Vec<Cell>]) -> Vec<u8> {
    let mut sheet = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    let header: Vec<Cell> = header.iter().map(|h| Cell::Text(h.to_string())).collect();
    for (n, row) in std::iter::once(&header).chain(rows.iter()).enumerate() {
        sheet.push_str(&format!("<row r=\"{}\">", n + 1));
        for cell in row {
            match cell {
                Cell::Text(text) => {
                    sheet.push_str("<c t=\"inlineStr\"><is><t xml:space=\"preserve\">");
                    sheet.push_str(&escape(text));
                    sheet.push_str("</t></is></c>");
                }
                Cell::Number(n) if n.is_finite() => sheet.push_str(&format!("<c><v>{n}</v></c>")),
                Cell::Number(_) => sheet.push_str("<c/>"),
            }
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    let parts: [(&str, String); 5] = [
        (
            "[Content_Types].xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
             <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
             </Types>"
                .to_string(),
        ),
        (
            "_rels/.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
             </Relationships>"
                .to_string(),
        ),
        (
            "xl/workbook.xml",
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
                 xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
                 <sheets><sheet name=\"{}\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>",
                escape(&sheet_name.chars().take(31).collect::<String>())
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
             </Relationships>"
                .to_string(),
        ),
        ("xl/worksheets/sheet1.xml", sheet),
    ];
    zip(&parts)
}

/// Text made safe for XML: the five special characters escaped, and the
/// characters XML 1.0 cannot hold dropped.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || matches!(c as u32, 0xFFFE | 0xFFFF) => {}
            c => out.push(c),
        }
    }
    out
}

/// A zip of the parts, stored: local headers, the data, the central
/// directory and its end record.
fn zip(parts: &[(&str, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in parts {
        let data = data.as_bytes();
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let size = data.len() as u32;
        let name = name.as_bytes();
        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // names are UTF-8
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&0u32.to_le_bytes()); // time and date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        // Its central directory entry.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 8]); // extra, comment, disk
        central.extend_from_slice(&[0u8; 2]); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let directory_at = out.len() as u32;
    let directory_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]); // disk numbers
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&directory_len.to_le_bytes());
    out.extend_from_slice(&directory_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
#[cfg(test)]
mod tests {
    use super::{Cell, escape, workbook};

    /// The stored parts of a zip, read back by walking its local headers.
    fn unzip(data: &[u8]) -> Vec<(String, String)> {
        let mut parts = Vec::new();
        let mut pos = 0;
        while data[pos..pos + 4] == 0x0403_4b50u32.to_le_bytes() {
            let crc = u32::from_le_bytes(data[pos + 14..pos + 18].try_into().unwrap());
            let size = u32::from_le_bytes(data[pos + 18..pos + 22].try_into().unwrap()) as usize;
            let name_len =
                u16::from_le_bytes(data[pos + 26..pos + 28].try_into().unwrap()) as usize;
            let name = String::from_utf8(data[pos + 30..pos + 30 + name_len].to_vec()).unwrap();
            let body = &data[pos + 30 + name_len..pos + 30 + name_len + size];
            assert_eq!(crc32fast::hash(body), crc, "{name}");
            parts.push((name, String::from_utf8(body.to_vec()).unwrap()));
            pos += 30 + name_len + size;
        }
        parts
    }

    #[test]
    fn the_workbook_holds_its_parts_and_its_rows() {
        let rows = vec![
            vec![Cell::Text("=HYPERLINK(\"x\")".into()), Cell::Number(2.5)],
            vec![Cell::Text("a < b & \"c\"".into()), Cell::Number(f64::NAN)],
        ];
        let data = workbook("Calls", &["Text", "Length (s)"], &rows);
        let parts = unzip(&data);
        let names: Vec<&str> = parts.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "[Content_Types].xml",
                "_rels/.rels",
                "xl/workbook.xml",
                "xl/_rels/workbook.xml.rels",
                "xl/worksheets/sheet1.xml"
            ]
        );
        let sheet = &parts[4].1;
        assert!(
            sheet
                .contains("<row r=\"1\"><c t=\"inlineStr\"><is><t xml:space=\"preserve\">Text</t>")
        );
        // A formula's text is a string, never a formula cell.
        assert!(sheet.contains("<t xml:space=\"preserve\">=HYPERLINK(&quot;x&quot;)</t>"));
        assert!(!sheet.contains("<f>"));
        assert!(sheet.contains("<c><v>2.5</v></c>"));
        assert!(sheet.contains("a &lt; b &amp; &quot;c&quot;"));
        assert!(sheet.contains("<row r=\"3\">"));
        // The end record is the last 22 bytes.
        assert_eq!(
            data[data.len() - 22..data.len() - 18],
            0x0605_4b50u32.to_le_bytes()
        );
    }

    #[test]
    fn characters_xml_cannot_hold_are_dropped() {
        assert_eq!(escape("a\u{0}b\u{1b}c\td\n"), "abc\td\n");
        assert_eq!(escape("caf\u{e9} \u{1F4FB}"), "caf\u{e9} \u{1F4FB}");
    }
}
