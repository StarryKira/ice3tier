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
use serde::Deserialize;

/// 第一次运行时写出来的配置文件。里面的值就是默认值。
pub const TEMPLATE: &str = r#"# ice3tier 节点的配置。
# 改完之后重启 i3t 才生效。`i3t set …` 改的也是这个文件。

# 节点绑定的端口（UDP）。启用为 relay 的节点还会在同一个端口号上监听 TCP。
port = 20330

# 这台机器声明自己可以承担什么。
# 声明本身不启用任何东西：启用要管理员在 WebUI 上打开；没有声明的，WebUI 上启用不了。
[declare]
# 可以作为出口：别的节点的流量从这台机器的网络出去。
exit = false
# 可以作为 relay：为别的节点转发数据包。这台机器要有从公网连得上的端口。
relay = false
# 可以作为端口转发的目标：别的节点可以经它访问它的回环地址和局域网里指定的服务。
forward = false

# 本机的代理入口，只给这台机器上的程序用。不需要的那一种写 "off"。
[proxy]
socks5 = "127.0.0.1:1080"
http = "127.0.0.1:1081"

# 本机的指标接口，Prometheus 的文本格式：http://<这里的地址>/metrics。只能绑回环地址。不需要就写 "off"。
[metrics]
listen = "127.0.0.1:20333"

[log]
# 日志级别：error、warn、info、debug、trace。
level = "info"
"#;

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
            Err(err) => return Err(err).with_context(|| format!("读配置文件 {}", path.display())),
        };
        Self::parse(&text).with_context(|| format!("配置文件 {} 有错", path.display()))
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

    /// 配置文件不存在时写出模板；已经存在时不动它。
    ///
    /// # Errors
    ///
    /// 写文件失败。
    pub fn write_template_if_missing(path: &Path) -> anyhow::Result<()> {
        match fs::OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => io::Write::write_all(&mut file, TEMPLATE.as_bytes()).with_context(|| format!("写配置文件 {}", path.display())),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(err) => Err(err).with_context(|| format!("创建配置文件 {}", path.display())),
        }
    }

    /// 把配置文件里的一项声明改成 `on`，注释和排版原样保留。文件不存在时先写出模板。
    ///
    /// # Errors
    ///
    /// 文件读写失败，或者现有的内容解析不了。
    pub fn set_declared(path: &Path, capability: Capability, on: bool) -> anyhow::Result<()> {
        Self::write_template_if_missing(path)?;
        let text = fs::read_to_string(path).with_context(|| format!("读配置文件 {}", path.display()))?;
        let mut document: toml_edit::DocumentMut =
            text.parse().map_err(|err| anyhow::anyhow!("{err}")).with_context(|| format!("配置文件 {} 有错", path.display()))?;
        document["declare"][capability.key()] = toml_edit::value(on);
        let updated = document.to_string();
        // 改完先整个验一遍，不把一个读不回来的文件写下去。
        Self::parse(&updated).with_context(|| format!("配置文件 {} 有错", path.display()))?;
        fs::write(path, updated).with_context(|| format!("写配置文件 {}", path.display()))
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.port == 0 {
            bail!("port 不能是 0：节点要绑定一个固定的端口，别的节点才找得到它");
        }
        for (name, entry) in [("proxy.socks5", self.proxy.socks5), ("proxy.http", self.proxy.http)] {
            if let ProxyBind::At(addr) = entry
                && !addr.ip().is_loopback()
            {
                bail!(
                    "{name} = \"{addr}\" 不是回环地址。这里的代理入口只给本机的程序用，只能写 127.0.0.1 或 [::1]。\n\
                     要对别的机器开放代理入口，请在 WebUI 的“监听器”里配置，那里可以设凭据和来源白名单。"
                );
            }
        }
        if let ProxyBind::At(addr) = self.metrics.listen
            && !addr.ip().is_loopback()
        {
            bail!(
                "metrics.listen = \"{addr}\" 不是回环地址。指标接口不带认证，只能写 127.0.0.1 或 [::1]。\n\
                 要从别的机器抓指标，请在这台机器上用反向代理或者 SSH 端口转发把它转出去。"
            );
        }
        if let (ProxyBind::At(socks5), ProxyBind::At(http)) = (self.proxy.socks5, self.proxy.http)
            && socks5 == http
        {
            bail!("proxy.socks5 和 proxy.http 不能用同一个地址 {socks5}");
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: default_port(),
            declare: Declare::default(),
            proxy: Proxy::default(),
            metrics: Metrics::default(),
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

/// 命令行上的写法：`can-exit`、`can-relay`、`can-forward`。
impl FromStr for Capability {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "can-exit" => Ok(Self::Exit),
            "can-relay" => Ok(Self::Relay),
            "can-forward" => Ok(Self::Forward),
            other => bail!("不认识的设置项 {other}，可以写的有 can-exit、can-relay、can-forward"),
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
        text.parse().map(Self::At).map_err(|_| {
            serde::de::Error::custom(format!("\"{text}\" 不是一个地址。写成 \"127.0.0.1:1080\" 这样的“地址:端口”，或者写 \"off\" 表示不开"))
        })
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
        assert_eq!(Config::parse(TEMPLATE).unwrap(), Config::default());
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
        assert!(error.contains("地址:端口") && error.contains("off"), "{error}");
    }

    #[test]
    fn a_non_loopback_proxy_is_refused_with_a_pointer_to_the_right_place() {
        let error = error_of("[proxy]\nsocks5 = \"0.0.0.0:1080\"\n");
        assert!(error.contains("回环地址") && error.contains("监听器"), "{error}");
    }

    #[test]
    fn the_metrics_endpoint_can_be_moved_or_switched_off_but_not_exposed() {
        assert_eq!(Config::default().metrics.listen.addr(), Some("127.0.0.1:20333".parse().unwrap()));
        let moved = Config::parse("[metrics]\nlisten = \"[::1]:9100\"\n").unwrap();
        assert_eq!(moved.metrics.listen.addr(), Some("[::1]:9100".parse().unwrap()));
        assert_eq!(Config::parse("[metrics]\nlisten = \"off\"\n").unwrap().metrics.listen, ProxyBind::Off);
        let error = error_of("[metrics]\nlisten = \"0.0.0.0:20333\"\n");
        assert!(error.contains("回环地址") && error.contains("metrics.listen"), "{error}");
    }

    #[test]
    fn nonsense_values_are_refused() {
        assert!(error_of("port = 0\n").contains("port"));
        assert!(error_of("[proxy]\nsocks5 = \"127.0.0.1:1080\"\nhttp = \"127.0.0.1:1080\"\n").contains("同一个地址"));
        assert!(error_of("[log]\nlevel = \"loud\"\n").contains("line 2"));
    }

    #[test]
    fn set_declared_changes_one_value_and_keeps_every_comment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::set_declared(&path, Capability::Exit, true).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, TEMPLATE.replace("exit = false", "exit = true"));
        assert_eq!(Config::load(&path).unwrap().declare, Declare { exit: true, relay: false, forward: false });

        Config::set_declared(&path, Capability::Exit, false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), TEMPLATE);
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
    fn capabilities_use_the_command_line_spelling() {
        for capability in [Capability::Exit, Capability::Relay, Capability::Forward] {
            assert_eq!(capability.to_string().parse::<Capability>().unwrap(), capability);
        }
        assert!("exit".parse::<Capability>().is_err());
    }
}
