//! 对齐的表格。表头随语言变，汉字在终端里占两格，所以按显示的宽度对齐，不按字符数。

use std::fmt;

/// 一张表：一行表头，下面若干行，每一格后面补空格对齐。最后一列不补。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    indent: usize,
}

impl Table {
    pub fn new<S: AsRef<str>>(header: &[S]) -> Self {
        Self { header: header.iter().map(|cell| cell.as_ref().to_owned()).collect(), rows: Vec::new(), indent: 0 }
    }

    /// 每一行前面空几格。
    #[must_use]
    pub fn indented(self, indent: usize) -> Self {
        Self { indent, ..self }
    }

    /// 加一行。列数要和表头一样。
    pub fn row(&mut self, cells: Vec<String>) {
        debug_assert_eq!(cells.len(), self.header.len(), "每一行的列数要和表头一样");
        self.rows.push(cells);
    }

    pub const fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn print(&self) {
        print!("{self}");
    }
}

impl fmt::Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let column_width = |column: usize| {
            let cells = self.rows.iter().filter_map(|row| row.get(column)).map(|cell| width(cell));
            cells.chain([width(&self.header[column])]).max().unwrap_or(0)
        };
        let widths: Vec<usize> = (0..self.header.len()).map(column_width).collect();
        let mut line = |cells: &mut dyn Iterator<Item = &str>| {
            let mut text = " ".repeat(self.indent);
            for (cell, column_width) in cells.zip(&widths) {
                text.push_str(cell);
                text.extend(std::iter::repeat_n(' ', column_width + 2 - width(cell)));
            }
            writeln!(f, "{}", text.trim_end())
        };
        line(&mut self.header.iter().map(String::as_str))?;
        self.rows.iter().try_for_each(|row| line(&mut row.iter().map(String::as_str)))
    }
}

/// 一组"名字 值"的行，名字一列对齐：详情页用。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fields {
    rows: Vec<(String, String)>,
    indent: usize,
}

impl Fields {
    #[must_use]
    pub const fn new(indent: usize) -> Self {
        Self { rows: Vec::new(), indent }
    }

    pub fn add(&mut self, label: &str, value: impl Into<String>) {
        self.rows.push((label.to_owned(), value.into()));
    }

    pub fn print(&self) {
        print!("{self}");
    }
}

impl fmt::Display for Fields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label_width = self.rows.iter().map(|(label, _)| width(label)).max().unwrap_or(0);
        for (label, value) in &self.rows {
            let pad = " ".repeat(label_width + 2 - width(label));
            writeln!(f, "{}{label}{pad}{value}", " ".repeat(self.indent))?;
        }
        Ok(())
    }
}

/// 一段文字在终端里占几格：东亚的宽字符（汉字、全角标点）两格，别的一格。只认常见的几段，够这几个程序用。
#[must_use]
pub fn width(text: &str) -> usize {
    text.chars().map(|ch| if is_wide(ch) { 2 } else { 1 }).sum()
}

const fn is_wide(ch: char) -> bool {
    matches!(ch,
        '\u{1100}'..='\u{115f}'
        | '\u{2e80}'..='\u{a4cf}'
        | '\u{ac00}'..='\u{d7a3}'
        | '\u{f900}'..='\u{faff}'
        | '\u{fe30}'..='\u{fe4f}'
        | '\u{ff00}'..='\u{ff60}'
        | '\u{ffe0}'..='\u{ffe6}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_padded_to_the_widest_cell() {
        let mut table = Table::new(&["ID", "NAME", "TAGS"]);
        table.row(vec!["1".to_owned(), "nas-01".to_owned(), "servers".to_owned()]);
        table.row(vec!["12".to_owned(), "a".to_owned(), "-".to_owned()]);
        assert_eq!(table.to_string(), "ID  NAME    TAGS\n1   nas-01  servers\n12  a       -\n");
    }

    #[test]
    fn field_labels_line_up_by_display_width() {
        let mut fields = Fields::new(2);
        fields.add("公钥", "abc");
        fields.add("虚拟 IP", "10.66.0.1");
        assert_eq!(fields.to_string(), "  公钥     abc\n  虚拟 IP  10.66.0.1\n");
    }

    #[test]
    fn wide_characters_take_two_columns() {
        assert_eq!((width("节点"), width("node"), width("（离线）"), width("ab节")), (4, 4, 8, 4));
        let mut table = Table::new(&["编号", "名字"]).indented(2);
        table.row(vec!["1".to_owned(), "nas".to_owned()]);
        assert_eq!(table.to_string(), "  编号  名字\n  1     nas\n");
    }
}
