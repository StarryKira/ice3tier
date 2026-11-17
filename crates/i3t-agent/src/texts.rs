//! 节点程序在终端上说的、出自这个 crate 的话：配置文件的模板，和交互的命令（`i3t join`、`i3t status`、`i3t set`、
//! `i3t self-check`）出错时的说明。两种语言各一份，按进程的语言（`i3t_lang::current`）取。
//!
//! 运行中的节点写进日志的话不在这里：日志照旧是中文。所以这里只放交互的命令走到的地方；`i3t run` 和交互的命令共用、
//! 出了错会进日志的地方（状态目录里文件的读写、长连接）保持原样。

use std::path::Path;

use i3t_lang::Lang;

/// 一种语言的文字。
#[derive(Debug)]
pub(crate) struct Texts {
    /// 第一次写出来的配置文件，注释是这种语言的。里面的值就是默认值。
    pub(crate) template: &'static str,
    pub(crate) read_config: fn(&Path) -> String,
    pub(crate) config_error: fn(&Path) -> String,
    pub(crate) write_config: fn(&Path) -> String,
    pub(crate) create_config: fn(&Path) -> String,
    pub(crate) port_zero: &'static str,
    pub(crate) proxy_not_loopback: fn(&str, &str) -> String,
    pub(crate) metrics_not_loopback: fn(&str) -> String,
    pub(crate) same_proxy_address: fn(&str) -> String,
    pub(crate) unknown_setting: fn(&str) -> String,
    pub(crate) not_a_switch: fn(&str) -> String,
    pub(crate) unknown_profile: fn(&str) -> String,
    pub(crate) bad_relay_address: fn(&str) -> String,
    pub(crate) performance_range: fn(&str, u64, u64) -> String,
    pub(crate) not_an_address: fn(&str) -> String,
    pub(crate) create_state_dir: fn(&Path) -> String,
    pub(crate) restrict_state_dir: fn(&Path) -> String,
    pub(crate) parse_key: fn(&Path) -> String,
    pub(crate) read_key: fn(&Path) -> String,
    pub(crate) generate_key: &'static str,
    pub(crate) find_program: &'static str,
    pub(crate) not_a_token: &'static str,
    pub(crate) not_joined: &'static str,
    pub(crate) enroll_timeout: &'static str,
    pub(crate) enroll_rejected: fn(&str) -> String,
    pub(crate) enroll_try_later: fn(&str) -> String,
    pub(crate) enroll_unexpected: fn(&str) -> String,
    pub(crate) enroll_closed: &'static str,
    pub(crate) connect: Connect,
    pub(crate) probe_close: &'static str,
    pub(crate) probe_refused: &'static str,
    pub(crate) query_running: fn(&str) -> String,
    pub(crate) status_unreadable: &'static str,
    /// `i3t ping` 的节点写得不对。
    pub(crate) bad_peer: fn(&str) -> String,
}

/// 连 controller 时每一步出错的说明。`i3t run` 的长连接用中文的这一份（它出错只进日志），交互的命令用当前语言的。
#[derive(Debug)]
pub(crate) struct Connect {
    pub(crate) tls_config: &'static str,
    pub(crate) server_name: &'static str,
    pub(crate) tcp: fn(&str) -> String,
    pub(crate) nodelay: &'static str,
    pub(crate) handshake: &'static str,
    pub(crate) timeout: fn(&str) -> String,
}

/// 这个进程的语言的文字。
pub(crate) fn texts() -> &'static Texts {
    texts_in(i3t_lang::current())
}

pub(crate) const fn texts_in(lang: Lang) -> &'static Texts {
    match lang {
        Lang::ZhCn => &ZH,
        Lang::English => &EN,
    }
}

static ZH: Texts = Texts {
    template: TEMPLATE_ZH,
    read_config: |path| format!("读配置文件 {}", path.display()),
    config_error: |path| format!("配置文件 {} 有错", path.display()),
    write_config: |path| format!("写配置文件 {}", path.display()),
    create_config: |path| format!("创建配置文件 {}", path.display()),
    port_zero: "port 不能是 0：别的节点要按固定的端口找它",
    proxy_not_loopback: |name, addr| {
        format!("{name} = \"{addr}\" 不是回环地址：本机的代理入口只能写 127.0.0.1 或 [::1]。给别的机器用的入口在 WebUI 的监听器里配置。")
    },
    metrics_not_loopback: |addr| format!("metrics.listen = \"{addr}\" 不是回环地址：指标接口没有认证，只能写 127.0.0.1 或 [::1]。"),
    same_proxy_address: |addr| format!("proxy.socks5 和 proxy.http 不能用同一个地址 {addr}"),
    unknown_setting: |other| {
        format!("没有设置项 {other}：可以写 can-exit、can-relay、can-forward、relay-address、throughput-tests、profile、intercept")
    },
    not_a_switch: |other| format!("\"{other}\" 不是开关的值：写 on 或 off"),
    unknown_profile: |other| format!("没有档位 \"{other}\"：可以写 low-footprint、balanced、high-throughput"),
    bad_relay_address: |text| {
        format!("\"{text}\" 不能作为中继地址：写别的节点连得上的“地址:端口”，比如 \"203.0.113.9:40330\"，或者 \"off\"")
    },
    performance_range: |key, low, high| format!("performance.{key} 要在 {low} 到 {high} 之间"),
    not_an_address: |text| format!("\"{text}\" 不是地址：写成 \"127.0.0.1:1080\" 这样的“地址:端口”，或者 \"off\""),
    create_state_dir: |path| format!("创建状态目录 {}", path.display()),
    restrict_state_dir: |path| format!("收紧状态目录 {} 的权限", path.display()),
    parse_key: |path| format!("私钥文件 {} 读不懂", path.display()),
    read_key: |path| format!("读私钥文件 {}", path.display()),
    generate_key: "生成节点私钥",
    find_program: "找不到正在运行的程序文件",
    not_a_token: "这不是注册令牌：令牌以 i3t- 开头，在 WebUI 的“添加节点”里生成",
    not_joined: "还没有加入网络",
    enroll_timeout: "controller 没有按时回答注册请求",
    enroll_rejected: |reason| format!("controller 拒绝了注册：{reason}"),
    enroll_try_later: |reason| format!("controller 暂时处理不了注册，令牌没有用掉，稍后再试：{reason}"),
    enroll_unexpected: |kind| format!("controller 的回答不对：{kind}"),
    enroll_closed: "controller 没有回答就关闭了连接",
    connect: CONNECT_ZH,
    probe_close: "关闭连接",
    probe_refused: "controller 拒绝了握手",
    query_running: |error| format!("问不到正在运行的节点：{error}"),
    status_unreadable: "正在运行的节点给的状态读不懂",
    bad_peer: |peer| format!("{peer:?} 不是节点名或者虚拟 IP"),
};

/// 中文的连接说明。长连接的日志一直用这一份。
pub(crate) const CONNECT_ZH: Connect = Connect {
    tls_config: "准备 TLS 配置",
    server_name: "构造 TLS 服务器名",
    tcp: |address| format!("连接 controller {address}"),
    nodelay: "设置 TCP_NODELAY",
    handshake: "和 controller 的 TLS 握手",
    timeout: |address| format!("连接 controller {address} 超时"),
};

static EN: Texts = Texts {
    template: TEMPLATE_EN,
    read_config: |path| format!("Reading the configuration file {}", path.display()),
    config_error: |path| format!("Error in the configuration file {}", path.display()),
    write_config: |path| format!("Writing the configuration file {}", path.display()),
    create_config: |path| format!("Creating the configuration file {}", path.display()),
    port_zero: "port can't be 0: other nodes find this one at a fixed port",
    proxy_not_loopback: |name, addr| {
        format!(
            "{name} = \"{addr}\" isn't a loopback address: local proxy entries take 127.0.0.1 or [::1]. \
             Entries for other machines are listeners in the WebUI."
        )
    },
    metrics_not_loopback: |addr| {
        format!("metrics.listen = \"{addr}\" isn't a loopback address: the metrics endpoint has no authentication.")
    },
    same_proxy_address: |addr| format!("proxy.socks5 and proxy.http can't share the address {addr}"),
    unknown_setting: |other| {
        format!(
            "No setting {other}: possible values: can-exit, can-relay, can-forward, relay-address, throughput-tests, profile, intercept"
        )
    },
    not_a_switch: |other| format!("\"{other}\" isn't a switch value: write on or off"),
    unknown_profile: |other| format!("No profile \"{other}\": possible values: low-footprint, balanced, high-throughput"),
    bad_relay_address: |text| {
        format!("\"{text}\" can't be a relay address: write an address:port other nodes can reach, e.g. \"203.0.113.9:40330\", or \"off\"")
    },
    performance_range: |key, low, high| format!("performance.{key} must be between {low} and {high}"),
    not_an_address: |text| format!("\"{text}\" isn't an address: write \"127.0.0.1:1080\" (address:port) or \"off\""),
    create_state_dir: |path| format!("Creating the state directory {}", path.display()),
    restrict_state_dir: |path| format!("Restricting the state directory {}", path.display()),
    parse_key: |path| format!("Private key file {} unreadable", path.display()),
    read_key: |path| format!("Reading the private key file {}", path.display()),
    generate_key: "Generating the node key",
    find_program: "Can't find the running program file",
    not_a_token: "Not an enrollment token: tokens start with i3t- and come from \"Add node\" in the WebUI",
    not_joined: "Not joined to a network yet",
    enroll_timeout: "The controller didn't answer the enrollment in time",
    enroll_rejected: |reason| format!("The controller rejected the enrollment: {reason}"),
    enroll_try_later: |reason| format!("The controller can't enroll right now; the token is still unused. Try again later: {reason}"),
    enroll_unexpected: |kind| format!("Unexpected answer from the controller: {kind}"),
    enroll_closed: "The controller closed the connection without answering",
    connect: Connect {
        tls_config: "Preparing TLS",
        server_name: "Preparing the TLS server name",
        tcp: |address| format!("Connecting to the controller at {address}"),
        nodelay: "Setting TCP_NODELAY",
        handshake: "TLS handshake with the controller",
        timeout: |address| format!("Connecting to the controller at {address} timed out"),
    },
    probe_close: "Closing the connection",
    probe_refused: "The controller refused the handshake",
    query_running: |error| format!("Couldn't ask the running node: {error}"),
    status_unreadable: "The running node's status is unreadable",
    bad_peer: |peer| format!("{peer:?} isn't a node name or virtual IP"),
};

const TEMPLATE_ZH: &str = r#"# ice3tier 节点的配置。改完重启 i3t 生效。`i3t set …` 改的也是这个文件。

# 节点的 UDP 端口。启用为中继的节点在同一个端口号上还监听 TCP。
port = 20330

# 这台机器声明可以承担什么。声明本身不启用任何东西：要管理员在 WebUI 上启用；没有声明的启用不了。
[declare]
# 出口：别的节点的流量从这台机器的网络出去。
exit = false
# 中继：为别的节点转发。别的节点连得上它的端口就行，谁连得上它，它就是谁的中继。
relay = false
# 中继的另一个地址，只有机房或路由器把另一个 IP 上的端口映射到这台机器时才要写，比如：
# relay_address = "203.0.113.9:40330"
# 也可以执行 i3t set relay-address 203.0.113.9:40330。别的地址由 controller 自己找出来。
# 端口转发的目标：别的节点经它访问它的回环地址和局域网里指定的服务。
forward = false

# 本机的代理入口，只给这台机器上的程序用。不需要的写 "off"。
[proxy]
socks5 = "127.0.0.1:1080"
http = "127.0.0.1:1081"

# 本机的指标接口（Prometheus）：http://<地址>/metrics。只能绑回环地址。不需要的写 "off"。
[metrics]
listen = "127.0.0.1:20333"

# 管理员发起的主动测试。
[tests]
# 接受吞吐测试：每个方向最多 5 秒或 20 MB。按流量计费的机器可以写 false（i3t set throughput-tests off）。
throughput = true

# 性能档位：low-footprint（一个核、几百 MB 内存的机器）、balanced、high-throughput（对端多、流量大）。
# controller 给这个节点选了档位时以它为准。i3t set profile <档位> 改的是这一行。
[performance]
profile = "balanced"
# 下面各项不写就用档位的值；写了就盖过档位，不管档位是谁选的。
# threads = 2               # 工作线程。只有一个对端在忙的机器、纯粹的中继写 1 最快。改了要重启
# stream_buffer_kb = 16     # 每条流每个方向的搬运缓冲
# stream_window_kb = 2048   # 每条流的接收窗口，最大 2048
# connection_window_mb = 64 # 一条连接上收、发各自最多压住多少数据
# idle_close_secs = 300     # 没有流之后多久关闭连接，0 是不关
# return_memory = false     # 每分钟把空着的内存还给系统

# 透明访问（只在 Linux 上）：本机的程序不配代理，直接连别的节点的虚拟 IP 或者 <名字>.<网络的域名>。
# 装包过滤规则、策略路由和网卡 i3t0，节点停下时拆掉；节点进程要有 CAP_NET_ADMIN 和 CAP_NET_BIND_SERVICE。
# i3t set intercept on 改的是这一行，i3t intercept status 列出装了什么。
[intercept]
enabled = false

[log]
# 日志级别：error、warn、info、debug、trace。
level = "info"
"#;

const TEMPLATE_EN: &str = r#"# ice3tier node configuration. Restart i3t after editing. `i3t set …` edits this file too.

# The node's UDP port. A node enabled as a relay also listens on TCP at the same port number.
port = 20330

# What this machine declares it can do. Declaring enables nothing: an admin enables it in the WebUI,
# and only declared roles can be enabled.
[declare]
# Exit: other nodes' traffic leaves through this machine's network.
exit = false
# Relay: forward for other nodes. A relay serves whoever can reach its port.
relay = false
# Another address of the relay. Only needed when a provider or router maps a port on another IP to this machine, e.g.:
# relay_address = "203.0.113.9:40330"
# Or run: i3t set relay-address 203.0.113.9:40330. The controller finds the other addresses itself.
# Port forward target: other nodes reach chosen services on its loopback and LAN through it.
forward = false

# Local proxy entries, for programs on this machine only. "off" turns one off.
[proxy]
socks5 = "127.0.0.1:1080"
http = "127.0.0.1:1081"

# Local metrics endpoint (Prometheus): http://<address>/metrics. Loopback only. "off" turns it off.
[metrics]
listen = "127.0.0.1:20333"

# Active tests started by the admin.
[tests]
# Accept throughput tests: up to 5 s or 20 MB each way. Metered machines can set false (i3t set throughput-tests off).
throughput = true

# Performance profile: low-footprint (one core, a few hundred MB of memory), balanced, high-throughput (many peers, heavy traffic).
# A profile chosen for this node on the controller applies instead. i3t set profile <profile> edits this line.
[performance]
profile = "balanced"
# Each item below defaults to the profile's value; set here, it overrides the profile, whoever chose it.
# threads = 2               # Worker threads. 1 is fastest with one busy peer or on a pure relay. Needs a restart
# stream_buffer_kb = 16     # Copy buffer per stream and direction
# stream_window_kb = 2048   # Receive window per stream, at most 2048
# connection_window_mb = 64 # Most data one connection holds, each way
# idle_close_secs = 300     # Close a connection this long after its last stream; 0 never
# return_memory = false     # Give unused memory back to the system every minute

# Transparent access (Linux only): programs here reach other nodes by virtual IP or <name>.<network domain>, no proxy settings.
# Installs packet-filter rules, a routing rule and a link i3t0, removed when the node stops; needs CAP_NET_ADMIN and CAP_NET_BIND_SERVICE.
# i3t set intercept on edits this line; i3t intercept status lists what is installed.
[intercept]
enabled = false

[log]
# Log level: error, warn, info, debug, trace.
level = "info"
"#;

#[cfg(test)]
mod tests {
    use i3t_lang::cli::has_chinese;

    use super::*;
    use crate::config::Config;

    #[test]
    fn the_configuration_template_reads_the_same_in_both_languages() {
        for lang in Lang::ALL {
            assert_eq!(Config::parse(texts_in(lang).template).unwrap(), Config::default(), "{lang:?} 的模板里的值就是默认值");
        }
        assert!(!has_chinese(EN.template), "英文的模板里有中文");
        assert!(has_chinese(ZH.template));
        // 两份模板的配置项一行一行对得上，只有注释不同。
        fn settings(template: &str) -> Vec<&str> {
            template.lines().filter(|line| !line.starts_with('#') && !line.is_empty()).collect()
        }
        assert_eq!(settings(ZH.template), settings(EN.template));
    }

    #[test]
    fn english_messages_have_no_chinese() {
        let path = Path::new("/var/lib/i3t/config.toml");
        let english = [
            (EN.read_config)(path),
            (EN.config_error)(path),
            (EN.proxy_not_loopback)("proxy.http", "0.0.0.0:1081"),
            (EN.metrics_not_loopback)("0.0.0.0:1"),
            (EN.unknown_setting)("x"),
            (EN.enroll_rejected)("x"),
            (EN.connect.tcp)("ctl:1"),
            EN.not_a_token.to_owned(),
            EN.port_zero.to_owned(),
        ];
        for text in english {
            assert!(!has_chinese(&text), "{text}");
        }
    }
}
