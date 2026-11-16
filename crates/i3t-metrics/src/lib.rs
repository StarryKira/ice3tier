//! 把指标写成 Prometheus 的文本格式。
//!
//! 各层把自己的计数放在普通的原子变量里，不依赖这个 crate。要输出的时候，由装配层读出当前的值，
//! 用 [`Exposition`] 排成文本。所以这里没有全局的注册表，也没有后台任务。
//!
//! ```
//! use i3t_metrics::Exposition;
//!
//! let mut out = Exposition::new();
//! out.counter("i3t_flows_total", "启动以来建立过的流的数量", &[("direction", "out")], 42);
//! out.gauge("i3t_flows_active", "正在进行的流的数量", &[], 3.0);
//! assert!(out.finish().contains("i3t_flows_total{direction=\"out\"} 42\n"));
//! ```

use std::fmt::Write;

/// 一次输出的全部指标。同名指标的多个样本要连着添加，`# HELP` 和 `# TYPE` 只在第一个样本前面写一次。
#[derive(Debug, Default)]
pub struct Exposition {
    text: String,
    /// 上一个样本的指标名，用来判断要不要再写说明行。
    last_name: Option<&'static str>,
}

impl Exposition {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 只增不减的累计量。按惯例名字以 `_total` 结尾。
    ///
    /// # Panics
    ///
    /// 指标名或标签名不符合 Prometheus 的命名规则。它们都是写死在代码里的，出错说明代码写错了。
    pub fn counter(&mut self, name: &'static str, help: &str, labels: &[(&str, &str)], value: u64) {
        self.sample(name, help, "counter", labels, &value.to_string());
    }

    /// 可增可减的当前值。
    ///
    /// # Panics
    ///
    /// 同 [`counter`](Self::counter)。
    pub fn gauge(&mut self, name: &'static str, help: &str, labels: &[(&str, &str)], value: f64) {
        self.sample(name, help, "gauge", labels, &format_float(value));
    }

    /// 输出的文本，可以直接作为 `/metrics` 的应答。
    #[must_use]
    pub fn finish(self) -> String {
        self.text
    }

    fn sample(&mut self, name: &'static str, help: &str, kind: &str, labels: &[(&str, &str)], value: &str) {
        assert!(is_metric_name(name), "指标名 {name:?} 不符合命名规则");
        if self.last_name != Some(name) {
            // 写进 String 不会失败。
            let _ = writeln!(self.text, "# HELP {name} {}", escape_help(help));
            let _ = writeln!(self.text, "# TYPE {name} {kind}");
            self.last_name = Some(name);
        }
        self.text.push_str(name);
        if !labels.is_empty() {
            self.text.push('{');
            for (index, (label, label_value)) in labels.iter().enumerate() {
                assert!(is_label_name(label), "标签名 {label:?} 不符合命名规则");
                if index > 0 {
                    self.text.push(',');
                }
                let _ = write!(self.text, "{label}=\"{}\"", escape_label_value(label_value));
            }
            self.text.push('}');
        }
        self.text.push(' ');
        self.text.push_str(value);
        self.text.push('\n');
    }
}

fn is_metric_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == ':')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn is_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn escape_help(help: &str) -> String {
    help.replace('\\', "\\\\").replace('\n', "\\n")
}

/// 标签的值来自运行时（节点名、凭据名），里面可能有任何字符。
fn escape_label_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn format_float(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "+Inf" } else { "-Inf" }.to_owned()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_of_one_metric_share_a_single_header() {
        let mut out = Exposition::new();
        out.counter("i3t_bytes_total", "收发的字节数", &[("direction", "tx")], 10);
        out.counter("i3t_bytes_total", "收发的字节数", &[("direction", "rx")], 20);
        out.gauge("i3t_flows_active", "正在进行的流", &[], 3.0);
        assert_eq!(
            out.finish(),
            "# HELP i3t_bytes_total 收发的字节数\n\
             # TYPE i3t_bytes_total counter\n\
             i3t_bytes_total{direction=\"tx\"} 10\n\
             i3t_bytes_total{direction=\"rx\"} 20\n\
             # HELP i3t_flows_active 正在进行的流\n\
             # TYPE i3t_flows_active gauge\n\
             i3t_flows_active 3\n"
        );
    }

    #[test]
    fn label_values_and_help_are_escaped() {
        let mut out = Exposition::new();
        out.gauge("m", "a\\b\nc", &[("node", "na\"me\\x\ny"), ("k", "v")], 1.5);
        let text = out.finish();
        assert!(text.contains("# HELP m a\\\\b\\nc\n"), "{text}");
        assert!(text.contains("m{node=\"na\\\"me\\\\x\\ny\",k=\"v\"} 1.5\n"), "{text}");
    }

    #[test]
    fn special_float_values_use_the_prometheus_spelling() {
        let mut out = Exposition::new();
        out.gauge("a", "", &[], f64::NAN);
        out.gauge("b", "", &[], f64::INFINITY);
        out.gauge("c", "", &[], f64::NEG_INFINITY);
        let text = out.finish();
        assert!(text.contains("a NaN\n") && text.contains("b +Inf\n") && text.contains("c -Inf\n"), "{text}");
    }

    #[test]
    #[should_panic(expected = "不符合命名规则")]
    fn a_bad_metric_name_is_a_programming_error() {
        Exposition::new().counter("1bad-name", "", &[], 0);
    }

    #[test]
    #[should_panic(expected = "不符合命名规则")]
    fn a_bad_label_name_is_a_programming_error() {
        Exposition::new().counter("ok", "", &[("bad-label", "v")], 0);
    }
}
