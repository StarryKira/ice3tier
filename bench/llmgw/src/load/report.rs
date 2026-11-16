//! 运行结果：写给程序看的 JSON，和打给人看的表。

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

use super::client::{Failure, GroupStats};
use super::scenario::{GroupSpec, Kind, Outcome};
use super::stats::{Episodes, Summary};
use super::watch::Usage;

#[derive(Debug, Serialize)]
pub struct Report {
    pub scenario: String,
    pub label: String,
    pub proxy: String,
    pub target: String,
    pub started_unix_ms: u64,
    pub elapsed_s: f64,
    pub stall_ms: u64,
    /// 全部的流都按期望结束了。
    pub passed: bool,
    pub streams: u64,
    pub peak_active: u64,
    pub peak_events_per_s: u64,
    pub events_per_s: f64,
    /// 跨流合并的卡顿：一次停顿牵连了几条流。
    pub stall_episodes: Episodes,
    pub groups: Vec<GroupReport>,
    pub processes: Vec<Usage>,
    pub load_cpu_s: Option<f64>,
    pub load_rss_peak_mb: Option<f64>,
    pub loadavg_start: Option<f64>,
    pub loadavg_end: Option<f64>,
    pub cores: usize,
}

#[derive(Debug, Serialize)]
pub struct GroupReport {
    pub name: String,
    pub kind: Kind,
    pub count: usize,
    pub expected: Outcome,
    pub passed: u64,
    pub failed: u64,
    pub outcomes: BTreeMap<Outcome, u64>,
    /// 连到代理（或者直连时连到上游）并完成代理握手。
    pub connect_ms: Summary,
    /// 经隧道和上游的 TLS 握手。
    pub tls_ms: Summary,
    /// 从开始连接到收到响应的第一个字节。
    pub first_byte_ms: Summary,
    /// 从开始写请求到上游读完请求体（单向，两边时钟一致时才有意义）。
    pub upload_ms: Summary,
    /// 从请求发完（WebSocket：一轮的请求发出）到收到第一个事件。
    pub ttft_ms: Summary,
    /// 第一个事件从上游写出到负载端收到（单向，两边时钟一致时才有意义）。思考之后的第一个事件到得慢就是它大。
    pub first_token_lag_ms: Summary,
    /// 相邻两个事件到达的间隔。
    pub gap_ms: Summary,
    /// 到达间隔比上游写出的间隔多出来的部分：路上多等了多久。
    pub gap_excess_ms: Summary,
    /// 每个事件从上游写出到负载端收到（单向，两边时钟一致时才有意义）。
    pub event_delay_ms: Summary,
    pub ping_rtt_ms: Summary,
    /// 流结束的时刻，从运行开始算。
    pub ended_at_ms: Summary,
    /// 到达间隔超出节拍 `stall_ms` 以上的次数。
    pub stalls: u64,
    pub events: u64,
    pub keepalives: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub failures: Vec<Failure>,
}

impl GroupReport {
    pub fn new(spec: &GroupSpec, stats: &GroupStats) -> Self {
        Self {
            name: spec.name.clone(),
            kind: spec.kind,
            count: spec.count,
            expected: spec.expected(),
            passed: stats.passed,
            failed: stats.failed,
            outcomes: stats.outcomes.clone(),
            connect_ms: stats.connect.summary(),
            tls_ms: stats.tls.summary(),
            first_byte_ms: stats.first_byte.summary(),
            upload_ms: stats.upload.summary(),
            ttft_ms: stats.ttft.summary(),
            first_token_lag_ms: stats.first_token_lag.summary(),
            gap_ms: stats.gap.summary(),
            gap_excess_ms: stats.gap_excess.summary(),
            event_delay_ms: stats.event_delay.summary(),
            ping_rtt_ms: stats.ping_rtt.summary(),
            ended_at_ms: stats.ended_at.summary(),
            stalls: stats.stalls,
            events: stats.events,
            keepalives: stats.keepalives,
            bytes_up: stats.bytes_up,
            bytes_down: stats.bytes_down,
            failures: stats.failures.clone(),
        }
    }
}

/// 按显示宽度补空格：汉字占两格，`format!` 的宽度按字符数算，对不齐。
fn pad(text: &str, width: usize) -> String {
    let shown: usize = text.chars().map(|ch| if ch.is_ascii() { 1 } else { 2 }).sum();
    format!("{text}{}", " ".repeat(width.saturating_sub(shown)))
}

fn line(name: &str, summary: &Summary) -> String {
    if summary.count == 0 {
        return String::new();
    }
    format!(
        "    {} p50 {:>9.1}  p90 {:>9.1}  p99 {:>9.1}  p99.9 {:>9.1}  max {:>9.1} ms  ({} 个)\n",
        pad(name, 14),
        summary.p50,
        summary.p90,
        summary.p99,
        summary.p999,
        summary.max,
        summary.count
    )
}

fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

impl Report {
    /// 给人看的表。
    pub fn table(&self) -> String {
        let mut out = String::new();
        let verdict = if self.passed { "全部按期望结束" } else { "有流没有按期望结束" };
        writeln!(
            out,
            "== 场景 {}{}：{verdict}。{} 条流，用了 {:.1} 秒，经 {}，到 {}",
            self.scenario,
            if self.label.is_empty() { String::new() } else { format!("（{}）", self.label) },
            self.streams,
            self.elapsed_s,
            self.proxy,
            self.target
        )
        .expect("写 String 不会失败");
        writeln!(
            out,
            "   同时在线最多 {} 条，事件峰值 {}/s、平均 {:.0}/s；机器负载 {} → {}（{} 核）",
            self.peak_active,
            self.peak_events_per_s,
            self.events_per_s,
            self.loadavg_start.map_or("?".to_owned(), |load| format!("{load:.1}")),
            self.loadavg_end.map_or("?".to_owned(), |load| format!("{load:.1}")),
            self.cores
        )
        .expect("写 String 不会失败");
        for group in &self.groups {
            let outcomes: Vec<String> = group.outcomes.iter().map(|(outcome, count)| format!("{} {count}", outcome.as_str())).collect();
            writeln!(
                out,
                "-- 组 {}（{}，{} 条，期望 {}）：符合 {}，不符合 {}；结果 {}",
                group.name,
                match group.kind {
                    Kind::Sse => "SSE",
                    Kind::Ws => "WebSocket",
                },
                group.count,
                group.expected.as_str(),
                group.passed,
                group.failed,
                outcomes.join("，")
            )
            .expect("写 String 不会失败");
            for (name, summary) in [
                ("连接+代理握手", &group.connect_ms),
                ("TLS 握手", &group.tls_ms),
                ("首字节", &group.first_byte_ms),
                ("上传请求体", &group.upload_ms),
                ("首个事件", &group.ttft_ms),
                ("首事件迟到", &group.first_token_lag_ms),
                ("事件间隔", &group.gap_ms),
                ("间隔超出节拍", &group.gap_excess_ms),
                ("单向延迟", &group.event_delay_ms),
                ("ping 往返", &group.ping_rtt_ms),
            ] {
                out.push_str(&line(name, summary));
            }
            writeln!(
                out,
                "    事件 {}，保活注释 {}，超出节拍 {} ms 以上的卡顿 {} 次；上行 {:.2} MB，下行 {:.2} MB",
                group.events,
                group.keepalives,
                self.stall_ms,
                group.stalls,
                megabytes(group.bytes_up),
                megabytes(group.bytes_down)
            )
            .expect("写 String 不会失败");
            for failure in group.failures.iter().take(8) {
                writeln!(
                    out,
                    "    ! {} 期望 {} 实际 {}，{:.1}s→{:.1}s，收到 {} 个事件：{}",
                    failure.stream,
                    failure.expected.as_str(),
                    failure.outcome.as_str(),
                    failure.started_s,
                    failure.ended_s,
                    failure.events,
                    failure.detail
                )
                .expect("写 String 不会失败");
            }
            if group.failures.len() > 8 {
                writeln!(out, "    ……另外 {} 条见 JSON", group.failed.saturating_sub(8)).expect("写 String 不会失败");
            }
        }
        let episodes = &self.stall_episodes;
        if episodes.count > 0 {
            writeln!(
                out,
                "-- 停顿：{} 次（牵连多条流的 {} 次），每次牵连的流数 p50 {} / 最多 {}，长度 p50 {:.0} / 最长 {:.0} ms",
                episodes.count, episodes.shared, episodes.streams_p50, episodes.streams_max, episodes.length_ms_p50, episodes.length_ms_max
            )
            .expect("写 String 不会失败");
        }
        for process in &self.processes {
            writeln!(
                out,
                "-- 进程 {}：CPU {:.2} 秒（平均 {:.0}%），常驻内存峰值 {:.1} MB、结束时 {:.1} MB{}",
                process.name,
                process.cpu_s,
                process.cpu_pct,
                process.rss_peak_mb,
                process.rss_end_mb,
                match (process.pids, process.missing) {
                    (0, _) => "，没有找到这个进程".to_owned(),
                    (1, 0) => String::new(),
                    (pids, missing) => format!("，期间换过进程（{pids} 个 pid，{missing} 次采样没找到）"),
                }
            )
            .expect("写 String 不会失败");
        }
        if let (Some(cpu), Some(rss)) = (self.load_cpu_s, self.load_rss_peak_mb) {
            writeln!(out, "-- 负载端自己：CPU {cpu:.2} 秒，常驻内存峰值 {rss:.1} MB").expect("写 String 不会失败");
        }
        out
    }
}
