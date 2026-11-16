//! 把结果打印成对齐的表格。

use std::fmt::Display;

/// 一张表：表头是英文大写，免得中文字符的宽度让列对不齐；会有中文的自由文本放在最后一列。
pub struct Table {
    header: Vec<&'static str>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(header: &[&'static str]) -> Self {
        Self { header: header.to_vec(), rows: Vec::new() }
    }

    /// 加一行。列数要和表头一样。
    pub fn row(&mut self, cells: Vec<String>) {
        debug_assert_eq!(cells.len(), self.header.len(), "每一行的列数要和表头一样");
        self.rows.push(cells);
    }

    pub fn print(&self) {
        print!("{self}");
    }
}

impl Display for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let width = |column: usize| {
            let cells = self.rows.iter().filter_map(|row| row.get(column)).map(|cell| cell.chars().count());
            cells.chain([self.header[column].len()]).max().unwrap_or(0)
        };
        let widths: Vec<usize> = (0..self.header.len()).map(width).collect();
        let mut line = |cells: &mut dyn Iterator<Item = &str>| {
            let mut text = String::new();
            for (cell, width) in cells.zip(&widths) {
                text.push_str(cell);
                text.extend(std::iter::repeat_n(' ', width + 2 - cell.chars().count()));
            }
            writeln!(f, "{}", text.trim_end())
        };
        line(&mut self.header.iter().copied())?;
        self.rows.iter().try_for_each(|row| line(&mut row.iter().map(String::as_str)))
    }
}

/// 没有值的单元格。
pub const NONE: &str = "-";

pub fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_owned()
}

/// 可能没有的值；没有时显示一个短横。
pub fn optional<T: Display>(value: Option<T>) -> String {
    value.map_or_else(|| NONE.to_owned(), |value| value.to_string())
}

/// 一组值用逗号连起来；空的显示一个短横。
pub fn list<T: Display>(values: impl IntoIterator<Item = T>) -> String {
    let joined = values.into_iter().map(|value| value.to_string()).collect::<Vec<_>>().join(",");
    if joined.is_empty() { NONE.to_owned() } else { joined }
}

/// 枚举在 JSON 里的写法（`direct_first` 之类），表格里照这个显示，和 `--json` 的输出一致。
pub fn label<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => NONE.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_padded_to_the_widest_cell() {
        let mut table = Table::new(&["ID", "NAME", "TAGS"]);
        table.row(vec!["1".to_owned(), "nas-01".to_owned(), "servers".to_owned()]);
        table.row(vec!["12".to_owned(), "a".to_owned(), NONE.to_owned()]);
        assert_eq!(table.to_string(), "ID  NAME    TAGS\n1   nas-01  servers\n12  a       -\n");
    }

    #[test]
    fn cells_render_the_way_the_json_does() {
        assert_eq!(label(&i3t_model::netmap::RoutingMode::LowestCost), "lowest_cost");
        assert_eq!(list(Vec::<String>::new()), "-");
        assert_eq!(list(["a", "b"]), "a,b");
        assert_eq!(optional(Some(3)), "3");
        assert_eq!(optional(None::<u32>), "-");
    }
}
