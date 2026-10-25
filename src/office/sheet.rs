//! Spreadsheets through calamine: the sheets' names, and a sheet's cells as
//! text, capped so that a huge sheet still opens at once.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use calamine::{
    Data, DataRef, ExcelDateTime, Reader, SheetType, SheetVisible, Sheets, open_workbook_auto,
};
use unicode_width::UnicodeWidthStr;

use super::biff;

/// Rows and columns read from a sheet; the rest is left unread.
pub const MAX_ROWS: usize = 5000;
pub const MAX_COLUMNS: usize = 200;

#[derive(Debug, Default)]
pub struct Grid {
    /// Each row's cells, all rows equally long.
    pub cells: Vec<Vec<Cell>>,
    /// The sheet's row and column of the first cell, from zero.
    pub origin: (u32, u32),
    /// The sheet's used rows and columns, of which `cells` may hold a part.
    pub size: (u32, u32),
    /// Each column's widest text, in columns of a monospace grid.
    pub widths: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub text: String,
    /// Numbers and dates line up on the right.
    pub number: bool,
}

/// The sheets worth a tab, which of them to open on, and that sheet. Chart,
/// dialog and macro sheets have no cells to show, and sheets hidden in Excel
/// stay hidden, unless every worksheet is. The workbook opens where Excel
/// would, on the sheet that was in front when it was saved, and when that
/// one cannot be read, on the next that can.
pub fn open(file: &Path) -> Option<(Vec<String>, usize, Grid)> {
    let Ok(mut book) = open_workbook_auto(file) else {
        return bare(file).map(|grid| (vec!["Sheet1".to_owned()], 0, grid));
    };
    let active = active_sheet(file)
        .and_then(|i| book.sheets_metadata().get(i))
        .map(|s| s.name.clone());
    let worksheets: Vec<(String, bool)> = book
        .sheets_metadata()
        .iter()
        .filter(|s| matches!(s.typ, SheetType::WorkSheet))
        .map(|s| (s.name.clone(), matches!(s.visible, SheetVisible::Visible)))
        .collect();
    let any_visible = worksheets.iter().any(|(_, visible)| *visible);
    let names: Vec<String> = worksheets
        .into_iter()
        .filter(|(_, visible)| *visible || !any_visible)
        .map(|(name, _)| name)
        .collect();
    let start = active
        .and_then(|name| names.iter().position(|n| *n == name))
        .unwrap_or(0);
    for i in (start..names.len()).chain(0..start) {
        if let Some(grid) = read_from(&mut book, &names[i]) {
            return Some((names, i, grid));
        }
    }
    None
}

/// The place, among all the workbook's sheets, of the one in front when
/// it was saved. xlsx records it in its workbook view; xls and ods are not asked.
fn active_sheet(file: &Path) -> Option<usize> {
    let xml = super::Package::open(file)?.text("xl/workbook.xml")?;
    let doc = roxmltree::Document::parse(&xml).ok()?;
    let view = doc
        .descendants()
        .find(|n| n.tag_name().name() == "workbookView")?;
    super::attr(view, "activeTab")?.parse().ok()
}

/// A bare BIFF stream, which holds one sheet.
fn bare(file: &Path) -> Option<Grid> {
    let mut data = Vec::new();
    File::open(file)
        .ok()?
        .take(64 << 20)
        .read_to_end(&mut data)
        .ok()?;
    let cells = biff::cells(&data)?;
    let end = cells.iter().fold((0, 0), |(r, c), ((row, column), _)| {
        (r.max(*row), c.max(*column))
    });
    Some(grid(cells, end))
}

/// The sheet called `name`.
pub fn read(file: &Path, name: &str) -> Option<Grid> {
    read_from(&mut open_workbook_auto(file).ok()?, name)
}

fn read_from(book: &mut Sheets<std::io::BufReader<std::fs::File>>, name: &str) -> Option<Grid> {
    let mut cells: Vec<((u32, u32), Cell)> = Vec::new();
    let end = match book {
        // xlsx streams its cells, so the read stops at the cap.
        Sheets::Xlsx(xlsx) => {
            let mut reader = xlsx.worksheet_cells_reader(name).ok()?;
            let dimensions = reader.dimensions();
            let last_row = dimensions.start.0 as usize + MAX_ROWS;
            while let Ok(Some(cell)) = reader.next_cell() {
                let (row, column) = cell.get_position();
                if row as usize >= last_row {
                    break;
                }
                let value = from_ref(cell.get_value());
                if !value.text.is_empty() {
                    cells.push(((row, column), value));
                }
            }
            dimensions.end
        }
        book => {
            let range = book.worksheet_range(name).ok()?;
            let start = range.start().unwrap_or((0, 0));
            for (r, row) in range.rows().take(MAX_ROWS).enumerate() {
                for (c, data) in row.iter().take(MAX_COLUMNS).enumerate() {
                    let value = from_data(data);
                    if !value.text.is_empty() {
                        cells.push(((start.0 + r as u32, start.1 + c as u32), value));
                    }
                }
            }
            range.end().unwrap_or((0, 0))
        }
    };
    Some(grid(cells, end))
}

/// A dense grid from the cells that have text, starting at the first of
/// them; the sheet's own row numbers and column letters stay with them.
fn grid(cells: Vec<((u32, u32), Cell)>, end: (u32, u32)) -> Grid {
    let size = (end.0 + 1, end.1 + 1);
    let Some(top) = cells.iter().map(|((r, _), _)| *r).min() else {
        return Grid {
            size,
            ..Grid::default()
        };
    };
    let left = cells.iter().map(|((_, c), _)| *c).min().unwrap_or(0);
    let bottom = cells.iter().map(|((r, _), _)| *r).max().unwrap_or(top);
    let right = cells.iter().map(|((_, c), _)| *c).max().unwrap_or(left);
    let rows = (bottom - top) as usize + 1;
    let columns = ((right - left) as usize + 1).min(MAX_COLUMNS);
    // Some writers declare a sheet as A1 whatever it holds.
    let size = (size.0.max(bottom + 1), size.1.max(right + 1));
    let mut out = Grid {
        cells: vec![vec![Cell::default(); columns]; rows.min(MAX_ROWS)],
        origin: (top, left),
        size,
        widths: vec![0; columns],
    };
    for ((r, c), cell) in cells {
        let (r, c) = ((r - top) as usize, (c - left) as usize);
        if r < out.cells.len() && c < columns {
            out.widths[c] =
                out.widths[c].max(cell.text.lines().map(|l| l.width()).max().unwrap_or(0));
            out.cells[r][c] = cell;
        }
    }
    out
}

fn from_data(data: &Data) -> Cell {
    match data {
        Data::Int(i) => number(i.to_string()),
        Data::Float(f) => number(general(*f)),
        Data::String(s) => text(s.clone()),
        Data::Bool(b) => text(if *b { "TRUE" } else { "FALSE" }.into()),
        Data::DateTime(d) => number(date(d)),
        Data::DateTimeIso(s) | Data::DurationIso(s) => number(s.clone()),
        Data::Error(e) => text(e.to_string()),
        Data::Empty => Cell::default(),
    }
}

fn from_ref(data: &DataRef) -> Cell {
    match data {
        DataRef::Int(i) => number(i.to_string()),
        DataRef::Float(f) => number(general(*f)),
        DataRef::String(s) => text(s.clone()),
        DataRef::SharedString(s) => text((*s).to_owned()),
        DataRef::Bool(b) => text(if *b { "TRUE" } else { "FALSE" }.into()),
        DataRef::DateTime(d) => number(date(d)),
        DataRef::DateTimeIso(s) | DataRef::DurationIso(s) => number(s.clone()),
        DataRef::Error(e) => text(e.to_string()),
        DataRef::Empty => Cell::default(),
    }
}

fn number(text: String) -> Cell {
    Cell { text, number: true }
}

fn text(text: String) -> Cell {
    Cell {
        text,
        number: false,
    }
}

/// A number as Excel's General format shows it: whole numbers without a
/// point, at most eleven characters, scientific when very large or small.
pub fn general(f: f64) -> String {
    if !f.is_finite() {
        return f.to_string();
    }
    if f == 0. {
        return "0".into();
    }
    if f.fract() == 0. && f.abs() < 1e11 {
        return format!("{}", f as i64);
    }
    let magnitude = f.abs().log10().floor() as i32;
    if !(-5..11).contains(&magnitude) {
        let s = format!("{f:.5E}");
        let (mantissa, exponent) = s.split_once('E').unwrap_or((&s, "0"));
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let exponent: i32 = exponent.parse().unwrap_or(0);
        return format!(
            "{mantissa}E{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        );
    }
    let mut decimals = (9 - magnitude).clamp(0, 15) as usize;
    let mut s = format!("{f:.decimals$}");
    while s.len() > 11 && decimals > 0 {
        decimals -= 1;
        s = format!("{f:.decimals$}");
    }
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// A date, a time of day or both, as ISO writes them; durations in hours.
fn date(d: &ExcelDateTime) -> String {
    let value = d.as_f64();
    if d.is_duration() {
        let seconds = (value * 86_400.).round() as i64;
        return format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        );
    }
    let (year, month, day, hour, minute, second, _) = d.to_ymd_hms_milli();
    let time = if second > 0 {
        format!("{hour:02}:{minute:02}:{second:02}")
    } else {
        format!("{hour:02}:{minute:02}")
    };
    if value < 1. {
        time
    } else if value.fract() == 0. {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{year:04}-{month:02}-{day:02} {time}")
    }
}

/// `A`, `B`, …, `Z`, `AA`, … for column `n` counted from zero.
pub fn column_name(mut n: u32) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::ExcelDateTimeType;
    use std::io::Write;

    /// A workbook as Excel for Mac wrote one: a hidden chart sheet first,
    /// then a worksheet, then another, hidden when `secret` is; `active` is
    /// the sheet in front, among all three.
    fn workbook(dir: &str, secret: bool, active: usize) -> std::path::PathBuf {
        let state = if secret { r#" state="hidden""# } else { "" };
        let dir = std::env::temp_dir().join(format!("silent-{dir}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.xlsx");
        let main = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let sheet = |value: u32| {
            format!(
                r#"<worksheet xmlns="{main}"><sheetData><row r="1"><c r="A1"><v>{value}</v></c></row></sheetData></worksheet>"#
            )
        };
        let parts = [
            (
                "[Content_Types].xml".to_owned(),
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#
                    .to_owned(),
            ),
            (
                "_rels/.rels".to_owned(),
                format!(
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
                ),
            ),
            (
                "xl/workbook.xml".to_owned(),
                format!(
                    r#"<workbook xmlns="{main}" xmlns:r="{rel}"><bookViews><workbookView activeTab="{active}"/></bookViews><sheets><sheet name="Chart1" sheetId="4" state="hidden" r:id="rId1"/><sheet name="Data" sheetId="1" r:id="rId2"/><sheet name="Secret" sheetId="2"{state} r:id="rId3"/></sheets></workbook>"#
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels".to_owned(),
                format!(
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/chartsheet" Target="chartsheets/sheet1.xml"/><Relationship Id="rId2" Type="{rel}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId3" Type="{rel}/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#
                ),
            ),
            (
                "xl/chartsheets/sheet1.xml".to_owned(),
                format!(r#"<chartsheet xmlns="{main}"/>"#),
            ),
            ("xl/worksheets/sheet1.xml".to_owned(), sheet(42)),
            ("xl/worksheets/sheet2.xml".to_owned(), sheet(7)),
        ];
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, xml) in parts {
            zip.start_file(name, options).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn chart_and_hidden_sheets_get_no_tab() {
        // The chart sheet was in front; the first worksheet shown opens instead.
        let (names, current, grid) = open(&workbook("charts", true, 0)).unwrap();
        assert_eq!(names, ["Data"]);
        assert_eq!(current, 0);
        assert_eq!(grid.cells[0][0].text, "42");
    }

    #[test]
    fn workbooks_open_on_the_sheet_in_front() {
        let (names, current, grid) = open(&workbook("active", false, 2)).unwrap();
        assert_eq!(names, ["Data", "Secret"]);
        assert_eq!(current, 1);
        assert_eq!(grid.cells[0][0].text, "7");
    }

    #[test]
    fn numbers_read_as_general_format() {
        assert_eq!(general(42.), "42");
        assert_eq!(general(std::f64::consts::PI), "3.141592654");
        assert_eq!(general(1. / 7.), "0.142857143");
        assert_eq!(general(-1. / 7.), "-0.14285714");
        assert_eq!(general(0.1 + 0.2), "0.3");
        assert_eq!(general(-1234.5), "-1234.5");
        assert_eq!(general(1.5e12), "1.5E+12");
        assert_eq!(general(0.000000123), "1.23E-07");
    }

    #[test]
    fn dates_and_durations_read_plainly() {
        let day = ExcelDateTime::new(46023., ExcelDateTimeType::DateTime, false);
        assert_eq!(date(&day), "2026-01-01");
        let moment = ExcelDateTime::new(46023.5, ExcelDateTimeType::DateTime, false);
        assert_eq!(date(&moment), "2026-01-01 12:00");
        let time = ExcelDateTime::new(0.75, ExcelDateTimeType::DateTime, false);
        assert_eq!(date(&time), "18:00");
        let span = ExcelDateTime::new(1.5, ExcelDateTimeType::TimeDelta, false);
        assert_eq!(date(&span), "36:00:00");
    }

    #[test]
    fn columns_are_named_like_a_spreadsheet() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }
}
