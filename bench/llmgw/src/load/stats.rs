//! 测量值的汇总：延迟分布（直方图），以及跨流的卡顿合并成"一次停顿影响了几条流"。

use std::time::Duration;

use hdrhistogram::Histogram;
use serde::Serialize;

/// 一个量的分布，内部单位是微秒，范围 1 µs 到 1 小时，3 位有效数字。
#[derive(Clone, Debug)]
pub struct Dist {
    hist: Histogram<u64>,
}

/// 分布的摘要，单位毫秒。没有样本时各项都是 0。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Summary {
    pub count: u64,
    pub min: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub p999: f64,
    pub max: f64,
    pub mean: f64,
}

impl Dist {
    const MAX_US: u64 = 3_600_000_000;

    pub fn new() -> Self {
        let hist = Histogram::new_with_bounds(1, Self::MAX_US, 3).expect("直方图的上下界是常量，一定合法");
        Self { hist }
    }

    /// 记一个样本。0 记成 1 µs（直方图的下界），超过 1 小时记成 1 小时。
    pub fn record_us(&mut self, us: u64) {
        self.hist.saturating_record(us.clamp(1, Self::MAX_US));
    }

    pub fn record(&mut self, value: Duration) {
        self.record_us(u64::try_from(value.as_micros()).unwrap_or(u64::MAX));
    }

    pub fn summary(&self) -> Summary {
        if self.hist.is_empty() {
            return Summary::default();
        }
        let ms = |us: u64| us as f64 / 1000.0;
        let at = |quantile: f64| ms(self.hist.value_at_quantile(quantile));
        Summary {
            count: self.hist.len(),
            min: ms(self.hist.min()),
            p50: at(0.50),
            p90: at(0.90),
            p99: at(0.99),
            p999: at(0.999),
            max: ms(self.hist.max()),
            mean: self.hist.mean() / 1000.0,
        }
    }
}

impl Default for Dist {
    fn default() -> Self {
        Self::new()
    }
}

/// 一条流的一次卡顿：这条流在 [start, end) 这段时间里等着下一个事件（时间从运行开始算，微秒）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StallSpan {
    pub start_us: u64,
    pub end_us: u64,
    pub stream: u32,
}

/// 重叠的卡顿合在一起算一次停顿。同一个丢包让一条连接上所有的流一起等时，这一次停顿就会牵连很多条流。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Episodes {
    /// 一共几次停顿。
    pub count: u64,
    /// 每次停顿牵连的流数：中位数、最大值。
    pub streams_p50: u64,
    pub streams_max: u64,
    /// 牵连了不止一条流的停顿有几次。
    pub shared: u64,
    /// 每次停顿从最早开始等的流到最后一条恢复的流，中位数和最大值，毫秒。
    pub length_ms_p50: f64,
    pub length_ms_max: f64,
}

impl Episodes {
    pub fn from_spans(mut spans: Vec<StallSpan>) -> Self {
        if spans.is_empty() {
            return Self::default();
        }
        spans.sort_by_key(|span| span.start_us);
        let mut episodes: Vec<(u64, u64, Vec<u32>)> = Vec::new();
        for span in spans {
            match episodes.last_mut() {
                Some((_, end, streams)) if span.start_us < *end => {
                    *end = (*end).max(span.end_us);
                    streams.push(span.stream);
                }
                _ => episodes.push((span.start_us, span.end_us, vec![span.stream])),
            }
        }
        let mut sizes: Vec<u64> = episodes
            .iter_mut()
            .map(|(_, _, streams)| {
                streams.sort_unstable();
                streams.dedup();
                streams.len() as u64
            })
            .collect();
        let mut lengths: Vec<u64> = episodes.iter().map(|(start, end, _)| end - start).collect();
        sizes.sort_unstable();
        lengths.sort_unstable();
        let median = |sorted: &[u64]| sorted[sorted.len() / 2];
        Self {
            count: episodes.len() as u64,
            streams_p50: median(&sizes),
            streams_max: *sizes.last().expect("至少有一次停顿"),
            shared: sizes.iter().filter(|&&size| size > 1).count() as u64,
            length_ms_p50: median(&lengths) as f64 / 1000.0,
            length_ms_max: *lengths.last().expect("至少有一次停顿") as f64 / 1000.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_quantiles_in_milliseconds() {
        let mut dist = Dist::new();
        for us in 1..=1000 {
            dist.record_us(us * 1000);
        }
        let summary = dist.summary();
        assert_eq!(summary.count, 1000);
        assert!((summary.min - 1.0).abs() < 0.01, "{summary:?}");
        assert!((summary.p50 - 500.0).abs() < 1.0, "{summary:?}");
        assert!((summary.p99 - 990.0).abs() < 1.0, "{summary:?}");
        assert!((summary.p999 - 999.0).abs() < 1.0, "{summary:?}");
        assert!((summary.max - 1000.0).abs() < 1.0, "{summary:?}");
        assert!((summary.mean - 500.5).abs() < 1.0, "{summary:?}");
    }

    #[test]
    fn empty_distributions_summarise_to_zero_and_extremes_are_clamped() {
        assert_eq!(Dist::new().summary(), Summary::default());
        let mut dist = Dist::new();
        dist.record_us(0);
        dist.record(Duration::from_secs(10 * 3600));
        let summary = dist.summary();
        assert_eq!(summary.count, 2);
        assert!(summary.p50 <= 0.002, "{summary:?}");
        assert!((summary.max - 3_600_000.0).abs() / 3_600_000.0 < 0.001, "{summary:?}");
    }

    #[test]
    fn overlapping_stalls_of_different_streams_form_one_episode() {
        let span = |start_us, end_us, stream| StallSpan { start_us, end_us, stream };
        // 一次丢包让三条流一起等了约 200 ms；之后一条流自己卡了一次；同一条流的两段算一条。
        let spans = vec![
            span(1_000_000, 1_200_000, 1),
            span(1_010_000, 1_210_000, 2),
            span(1_190_000, 1_205_000, 3),
            span(1_195_000, 1_206_000, 3),
            span(5_000_000, 5_300_000, 7),
        ];
        let episodes = Episodes::from_spans(spans);
        assert_eq!(episodes.count, 2);
        assert_eq!(episodes.streams_max, 3);
        assert_eq!(episodes.shared, 1);
        assert!((episodes.length_ms_max - 300.0).abs() < 0.01, "{episodes:?}");
        assert_eq!(Episodes::from_spans(Vec::new()), Episodes::default());
    }
}
