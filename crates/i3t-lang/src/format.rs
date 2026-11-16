//! 数、字节数、时长、时间、并列的几样东西按语言的写法。程序里写这些都经过这里，不各写各的。

use std::fmt::Display;

use crate::Lang;

/// 没有值的地方显示的短横。两种语言一样。
pub const NONE: &str = "-";

impl Lang {
    /// 字节数，按 1000 进位（和网络流量的计法、命令行上 `--quota 500G` 的写法一样）：`512 B`、`1.2 KB`、`3.4 GB`。
    #[must_use]
    pub fn bytes(self, count: u64) -> String {
        const UNITS: [(u64, &str); 4] = [(1_000_000_000_000, "TB"), (1_000_000_000, "GB"), (1_000_000, "MB"), (1_000, "KB")];
        match UNITS.iter().find(|(scale, _)| count >= *scale) {
            Some((scale, unit)) => format!("{}.{} {unit}", count / scale, count % scale * 10 / scale),
            None => format!("{count} B"),
        }
    }

    /// 一段时间，用最大的两个单位：中文 `3 小时 12 分`，英文 `3h 12m`。
    #[must_use]
    pub fn duration(self, secs: u64) -> String {
        let (days, hours, minutes, seconds) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60, secs % 60);
        match (self, days, hours, minutes) {
            (Self::ZhCn, 0, 0, 0) => format!("{seconds} 秒"),
            (Self::ZhCn, 0, 0, _) => format!("{minutes} 分 {seconds} 秒"),
            (Self::ZhCn, 0, _, _) => format!("{hours} 小时 {minutes} 分"),
            (Self::ZhCn, _, _, _) => format!("{days} 天 {hours} 小时"),
            (Self::English, 0, 0, 0) => format!("{seconds}s"),
            (Self::English, 0, 0, _) => format!("{minutes}m {seconds}s"),
            (Self::English, 0, _, _) => format!("{hours}h {minutes}m"),
            (Self::English, _, _, _) => format!("{days}d {hours}h"),
        }
    }

    /// 多久之前：中文 `3 分 12 秒前`，英文 `3m 12s ago`。
    #[must_use]
    pub fn ago(self, secs: u64) -> String {
        match self {
            Self::ZhCn => format!("{}前", self.duration(secs)),
            Self::English => format!("{} ago", self.duration(secs)),
        }
    }

    /// Unix 秒写成 UTC 的日期和时间：`2026-11-15 23:59:59Z`。两种语言一样，不带时区换算，不会读错。
    #[must_use]
    pub fn time(self, at: u64) -> String {
        const SECS_PER_DAY: u64 = 86400;
        let (days, secs) = (at / SECS_PER_DAY, at % SECS_PER_DAY);
        // 公历的日期换算（Howard Hinnant 的 civil_from_days），以 400 年为一个周期，从 0000-03-01 算起。
        let shifted = days + 719_468;
        let (era, day_of_era) = (shifted / 146_097, shifted % 146_097);
        let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
        let year = year_of_era + era * 400 + u64::from(month <= 2);
        format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
    }

    /// 微秒写成带一位小数的毫秒数（不带单位）：`41.2`。
    #[must_use]
    pub fn millis(self, micros: u64) -> String {
        format!("{}.{}", micros / 1000, micros % 1000 / 100)
    }

    /// 千分之几写成带一位小数的百分数（不带百分号）：`2.5`。
    #[must_use]
    pub fn percent(self, permille: u16) -> String {
        format!("{}.{}", permille / 10, permille % 10)
    }

    /// 比特每秒写成带一位小数的 Mbit/s 数（不带单位）。
    #[must_use]
    pub fn mbps(self, bits_per_second: u64) -> String {
        format!("{}.{}", bits_per_second / 1_000_000, bits_per_second % 1_000_000 / 100_000)
    }

    /// 并列的几样东西：中文用顿号，英文用逗号加空格。一样都没有时是短横。
    #[must_use]
    pub fn list<T: Display>(self, items: impl IntoIterator<Item = T>) -> String {
        let separator = match self {
            Self::ZhCn => "、",
            Self::English => ", ",
        };
        let joined = items.into_iter().map(|item| item.to_string()).collect::<Vec<_>>().join(separator);
        if joined.is_empty() { NONE.to_owned() } else { joined }
    }

    /// 两句话接在一起：中文直接相连，英文中间空一格。每一句自己带着句末的标点。
    #[must_use]
    pub fn sentences(self, first: &str, second: &str) -> String {
        match self {
            Self::ZhCn => format!("{first}{second}"),
            Self::English => format!("{first} {second}"),
        }
    }

    /// 一样东西后面跟着括号里的补充：中文 `laptop（离线）`，英文 `laptop (offline)`。
    #[must_use]
    pub fn paren(self, main: &str, aside: &str) -> String {
        match self {
            Self::ZhCn => format!("{main}（{aside}）"),
            Self::English => format!("{main} ({aside})"),
        }
    }

    /// 一个名字后面跟着它的说明：中文 `原因：…`，英文 `Reason: …`。
    #[must_use]
    pub fn labelled(self, label: &str, value: &str) -> String {
        match self {
            Self::ZhCn => format!("{label}：{value}"),
            Self::English => format!("{label}: {value}"),
        }
    }

    /// 一串层层包着的错误写成一行，外层在前：中文 `打开数据目录 /x：权限不够`，英文 `Opening /x: permission denied`。
    /// 相邻两层说的是同一句话时只留一次（库的错误常把来源的原话再说一遍）。
    #[must_use]
    pub fn chain<T: Display>(self, layers: impl IntoIterator<Item = T>) -> String {
        let mut said: Vec<String> = Vec::new();
        for layer in layers {
            let layer = layer.to_string();
            if said.last() != Some(&layer) {
                said.push(layer);
            }
        }
        said.join(match self {
            Self::ZhCn => "：",
            Self::English => ": ",
        })
    }
}

/// 可能没有的值；没有时是短横。
pub fn optional<T: Display>(value: Option<T>) -> String {
    value.map_or_else(|| NONE.to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_decimal_units_with_one_decimal() {
        for lang in Lang::ALL {
            assert_eq!(lang.bytes(512), "512 B");
            assert_eq!(lang.bytes(1_250), "1.2 KB");
            assert_eq!(lang.bytes(3_400_000_000), "3.4 GB");
            assert_eq!(lang.bytes(2_000_000_000_000), "2.0 TB");
        }
    }

    #[test]
    fn durations_use_the_two_largest_units_in_each_language() {
        assert_eq!(Lang::ZhCn.duration(42), "42 秒");
        assert_eq!(Lang::ZhCn.duration(192), "3 分 12 秒");
        assert_eq!(Lang::ZhCn.duration(3 * 3600 + 12 * 60 + 5), "3 小时 12 分");
        assert_eq!(Lang::ZhCn.duration(2 * 86400 + 5 * 3600 + 60), "2 天 5 小时");
        assert_eq!(Lang::English.duration(42), "42s");
        assert_eq!(Lang::English.duration(192), "3m 12s");
        assert_eq!(Lang::English.duration(3 * 3600 + 12 * 60 + 5), "3h 12m");
        assert_eq!(Lang::English.duration(2 * 86400 + 5 * 3600 + 60), "2d 5h");
        assert_eq!((Lang::ZhCn.ago(5), Lang::English.ago(5)), ("5 秒前".to_owned(), "5s ago".to_owned()));
    }

    #[test]
    fn times_are_utc_dates_in_both_languages() {
        for lang in Lang::ALL {
            assert_eq!(lang.time(0), "1970-01-01 00:00:00Z");
            assert_eq!(lang.time(951_782_400), "2000-02-29 00:00:00Z");
            assert_eq!(lang.time(1_794_787_199), "2026-11-15 23:59:59Z");
            assert_eq!(lang.time(4_102_444_800), "2100-01-01 00:00:00Z");
        }
    }

    #[test]
    fn numbers_lists_and_asides_follow_the_language() {
        assert_eq!((Lang::English.millis(41_250), Lang::English.millis(999)), ("41.2".to_owned(), "0.9".to_owned()));
        assert_eq!(Lang::ZhCn.percent(25), "2.5");
        assert_eq!(Lang::English.mbps(40_123_456), "40.1");
        assert_eq!(Lang::ZhCn.list(["a", "b"]), "a、b");
        assert_eq!(Lang::English.list(["a", "b"]), "a, b");
        assert_eq!(Lang::English.list(Vec::<String>::new()), NONE);
        assert_eq!(Lang::ZhCn.paren("laptop", "离线"), "laptop（离线）");
        assert_eq!(Lang::English.paren("laptop", "offline"), "laptop (offline)");
        assert_eq!(Lang::ZhCn.chain(["打开 /x", "权限不够", "权限不够"]), "打开 /x：权限不够");
        assert_eq!(Lang::English.chain(["Opening /x", "denied"]), "Opening /x: denied");
        assert_eq!(Lang::ZhCn.sentences("没有对端。", "先加入网络。"), "没有对端。先加入网络。");
        assert_eq!(Lang::English.sentences("No peers.", "Join first."), "No peers. Join first.");
        assert_eq!((optional(Some(3)), optional(None::<u32>)), ("3".to_owned(), NONE.to_owned()));
    }
}
