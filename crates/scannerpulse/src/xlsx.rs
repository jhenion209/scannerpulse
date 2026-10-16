//! A minimal Excel workbook: one sheet of rows, written without a library.
//!
//! The file is a zip of five XML parts (six when the sheet has links), stored
//! without compression. Every text cell is an inline string, never a formula,
//! so a transcript that starts with `=` stays text; a link is a hyperlink on
//! a cell that shows its address, not a `HYPERLINK` formula.

/// A cell of the sheet.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Number(f64),
    /// An address, shown as text and opened by a click.
    Link(String),
}

/// The longest text a cell holds, in characters (Excel's limit).
const MAX_CHARS: usize = 32_767;

/// The workbook's bytes: a header row, then the rows.
pub fn workbook(sheet_name: &str, header: &[&str], rows: &[Vec<Cell>]) -> Result<Vec<u8>, String> {
    let mut sheet = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheetData>",
    );
    let header: Vec<Cell> = header.iter().map(|h| Cell::Text(h.to_string())).collect();
    // Each link's cell and address, for the sheet's hyperlinks and their part.
    let mut links: Vec<(String, &str)> = Vec::new();
    for (n, row) in std::iter::once(&header).chain(rows.iter()).enumerate() {
        sheet.push_str(&format!("<row r=\"{}\">", n + 1));
        for (column, cell) in row.iter().enumerate() {
            match cell {
                Cell::Link(url) => {
                    let place = format!("{}{}", column_name(column), n + 1);
                    sheet.push_str(&format!(
                        "<c r=\"{place}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                        escape(url)
                    ));
                    links.push((place, url));
                }
                Cell::Text(text) => {
                    sheet.push_str("<c t=\"inlineStr\"><is><t xml:space=\"preserve\">");
                    sheet.push_str(&escape(&text.chars().take(MAX_CHARS).collect::<String>()));
                    sheet.push_str("</t></is></c>");
                }
                Cell::Number(n) if n.is_finite() => sheet.push_str(&format!("<c><v>{n}</v></c>")),
                Cell::Number(_) => sheet.push_str("<c/>"),
            }
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData>");
    let mut targets = String::new();
    if !links.is_empty() {
        sheet.push_str("<hyperlinks>");
        for (i, (place, url)) in links.iter().enumerate() {
            sheet.push_str(&format!(
                "<hyperlink ref=\"{place}\" r:id=\"rId{}\"/>",
                i + 1
            ));
            targets.push_str(&format!(
                "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
                i + 1,
                escape(url)
            ));
        }
        sheet.push_str("</hyperlinks>");
    }
    sheet.push_str("</worksheet>");
    let mut parts: Vec<(&str, String)> = vec![
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
    if !targets.is_empty() {
        parts.push((
            "xl/worksheets/_rels/sheet1.xml.rels",
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{targets}</Relationships>"
            ),
        ));
    }
    zip(&parts)
}

/// A column's letters: 0 is A, 25 is Z, 26 is AA.
fn column_name(mut index: usize) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).expect("ASCII letters")
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
fn zip(parts: &[(&str, String)]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in parts {
        let data = data.as_bytes();
        let crc = crc32fast::hash(data);
        let offset = u32_of(out.len())?;
        let size = u32_of(data.len())?;
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
        // Extra length, comment length, disk number and internal attributes.
        central.extend_from_slice(&[0u8; 8]);
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let directory_at = u32_of(out.len())?;
    let directory_len = u32_of(central.len())?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]); // disk numbers
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&directory_len.to_le_bytes());
    out.extend_from_slice(&directory_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// A zip's 32-bit size or offset; a larger file cannot be stored.
fn u32_of(n: usize) -> Result<u32, String> {
    u32::try_from(n).map_err(|_| "the workbook is larger than 4 GiB".to_string())
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
    fn a_link_shows_its_address_and_opens_it() {
        let rows = vec![vec![
            Cell::Text("a".into()),
            Cell::Link("https://radio.example/audio/7?x=1&y=2".into()),
        ]];
        let data = workbook("Calls", &["Text", "Audio"], &rows).unwrap();
        let parts = unzip(&data);
        let part = |name: &str| {
            parts
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, body)| body.clone())
                .unwrap_or_else(|| panic!("no part {name}"))
        };
        let sheet = part("xl/worksheets/sheet1.xml");
        assert!(
            sheet.contains(
                "<t xml:space=\"preserve\">https://radio.example/audio/7?x=1&amp;y=2</t>"
            )
        );
        assert!(sheet.contains("<hyperlinks><hyperlink ref=\"B2\" r:id=\"rId1\"/></hyperlinks>"));
        assert!(sheet.contains(
            "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\""
        ));
        assert!(!sheet.contains("<f>"), "a link is never a formula");
        let rels = part("xl/worksheets/_rels/sheet1.xml.rels");
        assert!(rels.contains(
            "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"https://radio.example/audio/7?x=1&amp;y=2\" TargetMode=\"External\"/>"
        ));
        assert_eq!(super::column_name(0), "A");
        assert_eq!(super::column_name(25), "Z");
        assert_eq!(super::column_name(26), "AA");
    }

    #[test]
    fn the_workbook_holds_its_parts_and_its_rows() {
        let rows = vec![
            vec![Cell::Text("=HYPERLINK(\"x\")".into()), Cell::Number(2.5)],
            vec![Cell::Text("a < b & \"c\"".into()), Cell::Number(f64::NAN)],
        ];
        let data = workbook("Calls", &["Text", "Length (s)"], &rows).unwrap();
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
        // Excel keeps at most 32,767 characters in a cell.
        let long = workbook("Calls", &["T"], &[vec![Cell::Text("a".repeat(40_000))]]).unwrap();
        let long_parts = unzip(&long);
        let long_sheet = &long_parts[4].1;
        assert!(long_sheet.contains(&format!(">{}</t>", "a".repeat(32_767))));
        assert!(!long_sheet.contains(&"a".repeat(32_768)));
        // The end record is the last 22 bytes.
        assert_eq!(
            data[data.len() - 22..data.len() - 18],
            0x0605_4b50u32.to_le_bytes()
        );
    }

    fn u16_at(data: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(data[at..at + 2].try_into().unwrap())
    }

    fn u32_at(data: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn the_central_directory_reads_back_as_a_zip_reader_reads_it() {
        let rows = vec![vec![Cell::Text("x".into()), Cell::Number(1.0)]];
        let data = workbook("Calls", &["A", "B"], &rows).unwrap();
        let end = data.len() - 22;
        assert_eq!(data[end..end + 4], 0x0605_4b50u32.to_le_bytes());
        let count = u16_at(&data, end + 10) as usize;
        let directory_len = u32_at(&data, end + 12) as usize;
        let directory_at = u32_at(&data, end + 16) as usize;
        assert_eq!(
            directory_at + directory_len,
            end,
            "the directory ends at the end record"
        );
        let mut pos = directory_at;
        for _ in 0..count {
            assert_eq!(
                data[pos..pos + 4],
                0x0201_4b50u32.to_le_bytes(),
                "a central entry"
            );
            let name_len = u16_at(&data, pos + 28) as usize;
            let extra = u16_at(&data, pos + 30) as usize;
            let comment = u16_at(&data, pos + 32) as usize;
            let local = u32_at(&data, pos + 42) as usize;
            let name = &data[pos + 46..pos + 46 + name_len];
            assert_eq!(
                data[local..local + 4],
                0x0403_4b50u32.to_le_bytes(),
                "a local header"
            );
            let local_name_len = u16_at(&data, local + 26) as usize;
            assert_eq!(&data[local + 30..local + 30 + local_name_len], name);
            pos += 46 + name_len + extra + comment;
        }
        assert_eq!(pos, end, "the walk ends at the end record");
    }

    #[test]
    fn characters_xml_cannot_hold_are_dropped() {
        assert_eq!(escape("a\u{0}b\u{1b}c\td\n"), "abc\td\n");
        assert_eq!(escape("caf\u{e9} \u{1F4FB}"), "caf\u{e9} \u{1F4FB}");
    }
}
