//! 节点的配置文件 `config.toml`。
//!
//! 这是使用者会亲手改的文件，所以：第一次运行时写出一份带注释的模板；写错了要说清楚哪一行、错在哪、应该怎么写；
//! 程序自己改它的时候（`i3t set …`）不能把注释和排版弄丢。
//!
//! 这里只放必须在本机决定的东西。其余的配置（对端、访问规则、监听器、端口转发）都由 controller 下发。

use std::fmt;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, bail};
use i3t_model::control::Declared;
use i3t_model::netmap::Profile;
use serde::Deserialize;

use crate::texts::texts;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub declare: Declare,
    #[serde(default)]
    pub proxy: Proxy,
    #[serde(default)]
    pub metrics: Metrics,
    #[serde(default)]
    pub tests: Tests,
    #[serde(default)]
    pub performance: PerformanceConfig,
    #[serde(default)]
    pub log: Log,
}

impl Config {
    /// 读配置文件。文件不存在时返回默认值，不创建文件。
    ///
    /// # Errors
    ///
    /// 文件读不了、语法不对、有不认识的配置项，或者某个值不合规矩。错误信息里有文件名和出错的位置。
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err).with_context(|| (texts().read_config)(path)),
        };
        Self::parse(&text).with_context(|| (texts().config_error)(path))
    }

    /// # Errors
    ///
    /// 同 [`load`](Self::load)，只是没有文件名。
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        // toml 的错误信息自带行号、列号和出错的那一行，原样给使用者看。
        let config: Self = toml::from_str(text).map_err(|err| anyhow::anyhow!("{err}"))?;
        config.validate()?;
        Ok(config)
    }

    /// 配置文件不存在时写出模板，注释用这时的语言（`i3t_lang::current`）；已经存在时不动它，也不因为换了语言重写。
    ///
    /// # Errors
    ///
    /// 写文件失败。
    pub fn write_template_if_missing(path: &Path) -> anyhow::Result<()> {
        match fs::OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => io::Write::write_all(&mut file, texts().template.as_bytes()).with_context(|| (texts().write_config)(path)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(err) => Err(err).with_context(|| (texts().create_config)(path)),
        }
    }

    /// 把配置文件里的一项声明改成 `on`，注释和排版原样保留。文件不存在时先写出模板。
    ///
    /// # Errors
    ///
    /// 文件读写失败，或者现有的内容解析不了。
    pub fn set_declared(path: &Path, capability: Capability, on: bool) -> anyhow::Result<()> {
        Self::set(path, Change::Declare(capability, on))
    }

    /// 改配置文件里的一项，注释和排版原样保留。文件不存在时先写出模板。
    ///
    /// # Errors
    ///
    /// 文件读写失败，或者现有的内容解析不了。
    pub fn set(path: &Path, change: Change) -> anyhow::Result<()> {
        Self::write_template_if_missing(path)?;
        let text = fs::read_to_string(path).with_context(|| (texts().read_config)(path))?;
        let mut document: toml_edit::DocumentMut =
            text.parse().map_err(|err| anyhow::anyhow!("{err}")).with_context(|| (texts().config_error)(path))?;
        let (section, key) = change.setting().key();
        document[section][key] = match change {
            Change::Declare(_, on) | Change::ThroughputTests(on) => toml_edit::value(on),
            Change::Profile(profile) => toml_edit::value(profile.spelling()),
        };
        let updated = document.to_string();
        // 改完先整个验一遍，不把一个读不回来的文件写下去。
        Self::parse(&updated).with_context(|| (texts().config_error)(path))?;
        fs::write(path, updated).with_context(|| (texts().write_config)(path))
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.port == 0 {
            bail!(texts().port_zero);
        }
        for (name, entry) in [("proxy.socks5", self.proxy.socks5), ("proxy.http", self.proxy.http)] {
            if let ProxyBind::At(addr) = entry
                && !addr.ip().is_loopback()
            {
                bail!((texts().proxy_not_loopback)(name, &addr.to_string()));
            }
        }
        if let ProxyBind::At(addr) = self.metrics.listen
            && !addr.ip().is_loopback()
        {
            bail!((texts().metrics_not_loopback)(&addr.to_string()));
        }
        if let (ProxyBind::At(socks5), ProxyBind::At(http)) = (self.proxy.socks5, self.proxy.http)
            && socks5 == http
        {
            bail!((texts().same_proxy_address)(&socks5.to_string()));
        }
        self.performance.validate()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: default_port(),
            declare: Declare::default(),
            proxy: Proxy::default(),
            metrics: Metrics::default(),
            tests: Tests::default(),
            performance: PerformanceConfig::default(),
            log: Log::default(),
        }
    }
}

const fn default_port() -> u16 {
    crate::state::DEFAULT_PORT
}

/// 这台机器声明自己可以承担什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declare {
    #[serde(default)]
    pub exit: bool,
    #[serde(default)]
    pub relay: bool,
    #[serde(default)]
    pub forward: bool,
}

impl From<Declare> for Declared {
    fn from(declare: Declare) -> Self {
        Self { exit: declare.exit, relay: declare.relay, forward: declare.forward }
    }
}

/// 可以在本机声明的三种能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Exit,
    Relay,
    Forward,
}

impl Capability {
    const fn key(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Relay => "relay",
            Self::Forward => "forward",
        }
    }
}

/// `i3t set` 能改的项：三项声明、接不接受吞吐测试、性能档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Declare(Capability),
    /// 接不接受管理员发起的吞吐测试（`[tests] throughput`）。
    ThroughputTests,
    /// controller 没有选档位时用的性能档位（`[performance] profile`）。
    Profile,
}

impl Setting {
    /// 在配置文件里的位置：节和键。
    const fn key(self) -> (&'static str, &'static str) {
        match self {
            Self::Declare(capability) => ("declare", capability.key()),
            Self::ThroughputTests => ("tests", "throughput"),
            Self::Profile => ("performance", "profile"),
        }
    }

    /// 这一项改成命令行上写的 `value`：开关写 `on` 或 `off`，档位写它的名字。
    ///
    /// # Errors
    ///
    /// 值的写法不对。
    pub fn change(self, value: &str) -> anyhow::Result<Change> {
        let switch = || match value {
            "on" => Ok(true),
            "off" => Ok(false),
            other => Err(anyhow::anyhow!((texts().not_a_switch)(other))),
        };
        Ok(match self {
            Self::Declare(capability) => Change::Declare(capability, switch()?),
            Self::ThroughputTests => Change::ThroughputTests(switch()?),
            Self::Profile => Change::Profile(value.parse().map_err(|_| anyhow::anyhow!((texts().unknown_profile)(value)))?),
        })
    }
}

/// 命令行上的写法：`can-exit`、`can-relay`、`can-forward`、`throughput-tests`、`profile`。
impl FromStr for Setting {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "throughput-tests" => Ok(Self::ThroughputTests),
            "profile" => Ok(Self::Profile),
            other => other.parse().map(Self::Declare).map_err(|_| anyhow::anyhow!((texts().unknown_setting)(other))),
        }
    }
}

impl fmt::Display for Setting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declare(capability) => capability.fmt(f),
            Self::ThroughputTests => f.write_str("throughput-tests"),
            Self::Profile => f.write_str("profile"),
        }
    }
}

/// `i3t set` 的一次修改：哪一项，改成什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Declare(Capability, bool),
    ThroughputTests(bool),
    Profile(Profile),
}

impl Change {
    #[must_use]
    pub const fn setting(self) -> Setting {
        match self {
            Self::Declare(capability, _) => Setting::Declare(capability),
            Self::ThroughputTests(_) => Setting::ThroughputTests,
            Self::Profile(_) => Setting::Profile,
        }
    }
}

/// 命令行上的写法：`on`、`off`，或者档位的名字。
impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declare(_, on) | Self::ThroughputTests(on) => f.write_str(if *on { "on" } else { "off" }),
            Self::Profile(profile) => f.write_str(profile.spelling()),
        }
    }
}

/// 命令行上的写法：`can-exit`、`can-relay`、`can-forward`。
impl FromStr for Capability {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "can-exit" => Ok(Self::Exit),
            "can-relay" => Ok(Self::Relay),
            "can-forward" => Ok(Self::Forward),
            other => bail!((texts().unknown_setting)(other)),
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "can-{}", self.key())
    }
}

/// 本机的代理入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proxy {
    #[serde(default = "default_socks5")]
    pub socks5: ProxyBind,
    #[serde(default = "default_http")]
    pub http: ProxyBind,
}

impl Default for Proxy {
    fn default() -> Self {
        Self { socks5: default_socks5(), http: default_http() }
    }
}

fn default_socks5() -> ProxyBind {
    ProxyBind::At(SocketAddr::from(([127, 0, 0, 1], 1080)))
}

fn default_http() -> ProxyBind {
    ProxyBind::At(SocketAddr::from(([127, 0, 0, 1], 1081)))
}

/// 一个只给本机用的入口（代理、指标接口）绑在哪里，或者不开。配置文件里写地址，或者写 `"off"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyBind {
    Off,
    At(SocketAddr),
}

impl ProxyBind {
    #[must_use]
    pub const fn addr(self) -> Option<SocketAddr> {
        match self {
            Self::Off => None,
            Self::At(addr) => Some(addr),
        }
    }
}

impl<'de> Deserialize<'de> for ProxyBind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.eq_ignore_ascii_case("off") {
            return Ok(Self::Off);
        }
        text.parse().map(Self::At).map_err(|_| serde::de::Error::custom((texts().not_an_address)(&text)))
    }
}

/// 本机的指标接口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    /// 绑在哪里，或者不开。写法和代理入口一样：地址，或者 `"off"`。
    #[serde(default = "default_metrics")]
    pub listen: ProxyBind,
}

impl Default for Metrics {
    fn default() -> Self {
        Self { listen: default_metrics() }
    }
}

fn default_metrics() -> ProxyBind {
    ProxyBind::At(SocketAddr::from(([127, 0, 0, 1], 20333)))
}

/// 主动测试的本机开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tests {
    /// 接受吞吐测试。关掉之后对端开来的测量流一律拒绝，controller 也知道（节点报给它），不会再发吞吐的测试项。
    #[serde(default = "default_true")]
    pub throughput: bool,
}

impl Default for Tests {
    fn default() -> Self {
        Self { throughput: true }
    }
}

const fn default_true() -> bool {
    true
}

/// 性能档位和逐项的覆盖（`[performance]`）。档位的值和它们的来历见 [`crate::performance`]。
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceConfig {
    /// controller 没有给这个节点选档位时用的档位。写法是 `low-footprint`、`balanced`、`high-throughput`。
    #[serde(default, deserialize_with = "profile_spelling")]
    pub profile: Profile,
    pub threads: Option<u32>,
    pub stream_buffer_kb: Option<u32>,
    pub stream_window_kb: Option<u32>,
    pub connection_window_mb: Option<u32>,
    /// 0 是不因为空闲关闭。
    pub idle_close_secs: Option<u64>,
    pub return_memory: Option<bool>,
}

impl PerformanceConfig {
    /// 每一项的范围。流窗口的上限是连接层的硬上限（见 [`i3t_conn::MAX_STREAM_WINDOW`]）。
    const RANGES: [(&'static str, u64, u64); 4] = [
        ("threads", 1, 256),
        ("stream_buffer_kb", 1, 1024),
        ("stream_window_kb", 64, MAX_STREAM_WINDOW_KB),
        ("connection_window_mb", 1, 4095),
    ];

    fn validate(&self) -> anyhow::Result<()> {
        let values = [
            self.threads.map(u64::from),
            self.stream_buffer_kb.map(u64::from),
            self.stream_window_kb.map(u64::from),
            self.connection_window_mb.map(u64::from),
        ];
        for ((key, low, high), value) in Self::RANGES.into_iter().zip(values) {
            if let Some(value) = value
                && !(low..=high).contains(&value)
            {
                bail!((texts().performance_range)(key, low, high));
            }
        }
        Ok(())
    }
}

/// 流窗口最多能写多少 KB。
const MAX_STREAM_WINDOW_KB: u64 = i3t_conn::MAX_STREAM_WINDOW as u64 / 1024;

/// 配置文件里的档位用命令行的写法（`low-footprint`），也认 JSON 的写法（`low_footprint`）。
fn profile_spelling<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Profile, D::Error> {
    let text = String::deserialize(deserializer)?;
    text.parse().map_err(|_| serde::de::Error::custom((texts().unknown_profile)(&text)))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Log {
    #[serde(default)]
    pub level: LogLevel,
}

impl Default for Log {
    fn default() -> Self {
        Self { level: LogLevel::Info }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// 日志库认的写法。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_of(text: &str) -> String {
        format!("{:#}", Config::parse(text).unwrap_err())
    }

    #[test]
    fn the_template_parses_to_the_defaults() {
        assert_eq!(Config::parse(texts().template).unwrap(), Config::default());
    }

    #[test]
    fn an_empty_or_missing_file_means_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(&dir.path().join("config.toml")).unwrap(), Config::default());
    }

    #[test]
    fn a_misspelt_key_is_reported_with_its_line_and_the_valid_names() {
        let error = error_of("prot = 20330\n");
        assert!(error.contains("line 1"), "{error}");
        assert!(error.contains("prot") && error.contains("port"), "{error}");
    }

    #[test]
    fn a_wrong_type_is_reported_with_its_line() {
        let error = error_of("port = 20330\n[declare]\nexit = \"yes\"\n");
        assert!(error.contains("line 3"), "{error}");
    }

    #[test]
    fn proxy_entries_accept_an_address_or_off_and_explain_anything_else() {
        let config = Config::parse("[proxy]\nsocks5 = \"OFF\"\nhttp = \"[::1]:8080\"\n").unwrap();
        assert_eq!(config.proxy.socks5, ProxyBind::Off);
        assert_eq!(config.proxy.http.addr(), Some("[::1]:8080".parse().unwrap()));
        let error = error_of("[proxy]\nsocks5 = \"1080\"\n");
        assert!(error.contains(&(texts().not_an_address)("1080")), "{error}");
    }

    #[test]
    fn a_non_loopback_proxy_is_refused_with_a_pointer_to_the_right_place() {
        let error = error_of("[proxy]\nsocks5 = \"0.0.0.0:1080\"\n");
        assert!(error.contains(&(texts().proxy_not_loopback)("proxy.socks5", "0.0.0.0:1080")), "{error}");
    }

    #[test]
    fn the_metrics_endpoint_can_be_moved_or_switched_off_but_not_exposed() {
        assert_eq!(Config::default().metrics.listen.addr(), Some("127.0.0.1:20333".parse().unwrap()));
        let moved = Config::parse("[metrics]\nlisten = \"[::1]:9100\"\n").unwrap();
        assert_eq!(moved.metrics.listen.addr(), Some("[::1]:9100".parse().unwrap()));
        assert_eq!(Config::parse("[metrics]\nlisten = \"off\"\n").unwrap().metrics.listen, ProxyBind::Off);
        let error = error_of("[metrics]\nlisten = \"0.0.0.0:20333\"\n");
        assert!(error.contains(&(texts().metrics_not_loopback)("0.0.0.0:20333")), "{error}");
    }

    #[test]
    fn nonsense_values_are_refused() {
        assert!(error_of("port = 0\n").contains("port"));
        let error = error_of("[proxy]\nsocks5 = \"127.0.0.1:1080\"\nhttp = \"127.0.0.1:1080\"\n");
        assert!(error.contains(&(texts().same_proxy_address)("127.0.0.1:1080")), "{error}");
        assert!(error_of("[log]\nlevel = \"loud\"\n").contains("line 2"));
    }

    #[test]
    fn set_declared_changes_one_value_and_keeps_every_comment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::set_declared(&path, Capability::Exit, true).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, texts().template.replace("exit = false", "exit = true"));
        assert_eq!(Config::load(&path).unwrap().declare, Declare { exit: true, relay: false, forward: false });

        Config::set_declared(&path, Capability::Exit, false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), texts().template);
    }

    #[test]
    fn set_declared_works_on_a_hand_written_file_without_the_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# 我的节点\nport = 20444\n").unwrap();
        Config::set_declared(&path, Capability::Relay, true).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!((config.port, config.declare.relay), (20444, true));
        assert!(fs::read_to_string(&path).unwrap().starts_with("# 我的节点\n"));
    }

    #[test]
    fn set_declared_refuses_to_touch_a_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "port = = 1\n").unwrap();
        assert!(Config::set_declared(&path, Capability::Exit, true).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "port = = 1\n");
    }

    #[test]
    fn throughput_tests_are_accepted_unless_switched_off_and_the_switch_keeps_the_comments() {
        assert!(Config::default().tests.throughput);
        assert!(Config::parse("[tests]\n").unwrap().tests.throughput, "没写就是接受");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let setting: Setting = "throughput-tests".parse().unwrap();
        Config::set(&path, setting.change("off").unwrap()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), texts().template.replace("throughput = true", "throughput = false"));
        assert!(!Config::load(&path).unwrap().tests.throughput);
        assert_eq!(setting.to_string(), "throughput-tests");
        assert_eq!("can-relay".parse::<Setting>().unwrap(), Setting::Declare(Capability::Relay));
        assert!("throughput".parse::<Setting>().unwrap_err().to_string().contains("throughput-tests"));
    }

    #[test]
    fn the_profile_is_set_by_name_and_each_item_has_a_range() {
        assert_eq!(Config::default().performance, PerformanceConfig::default());
        assert_eq!(Config::default().performance.profile, Profile::Balanced);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let change = Setting::Profile.change("low-footprint").unwrap();
        assert_eq!(change.to_string(), "low-footprint");
        Config::set(&path, change).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), texts().template.replace("profile = \"balanced\"", "profile = \"low-footprint\""));
        assert_eq!(Config::load(&path).unwrap().performance.profile, Profile::LowFootprint);
        assert!(Setting::Profile.change("fast").unwrap_err().to_string().contains("high-throughput"));
        assert!(Setting::ThroughputTests.change("yes").is_err());

        let tuned = Config::parse("[performance]\nprofile = \"high_throughput\"\nthreads = 8\nidle_close_secs = 0\n").unwrap();
        assert_eq!((tuned.performance.profile, tuned.performance.threads), (Profile::HighThroughput, Some(8)));
        assert!(error_of("[performance]\nprofile = \"turbo\"\n").contains("turbo"));
        assert!(error_of("[performance]\nthreads = 0\n").contains(&(texts().performance_range)("threads", 1, 256)));
        assert!(error_of("[performance]\nstream_window_kb = 4096\n").contains("stream_window_kb"), "流窗口不能超过连接层的上限");
        assert!(error_of("[performance]\nwindow = 1\n").contains("window"));
    }

    #[test]
    fn capabilities_use_the_command_line_spelling() {
        for capability in [Capability::Exit, Capability::Relay, Capability::Forward] {
            assert_eq!(capability.to_string().parse::<Capability>().unwrap(), capability);
        }
        assert!("exit".parse::<Capability>().is_err());
    }
}
