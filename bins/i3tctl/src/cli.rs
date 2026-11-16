//! 命令行的参数。

use std::net::{Ipv4Addr, SocketAddr};

use clap::{Args, Parser, Subcommand, ValueEnum};
use i3t_model::netmap::{IpFamily, Reach, RoutingMode};
use i3t_proto::flow::Class;
use ipnet::{IpNet, Ipv4Net};

#[derive(Parser)]
#[command(name = "i3tctl", version, about = "ice3tier 的管理命令行：通过管理 API 操作 controller")]
pub struct Cli {
    /// 管理 API 的地址
    #[arg(long, env = "I3T_ADMIN_URL", default_value = "http://127.0.0.1:20332", global = true)]
    pub url: String,

    /// 管理令牌，controller 第一次启动时打印的那个
    #[arg(long, env = "I3T_ADMIN_TOKEN", hide_env_values = true, global = true, allow_hyphen_values = true)]
    pub token: Option<String>,

    /// 操作哪个网络，写名字或编号。controller 里只有一个网络时可以不写
    #[arg(long, env = "I3T_NETWORK", global = true)]
    pub network: Option<String>,

    /// 原样输出管理 API 的 JSON，给脚本用
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// 下面凡是写 NODE 的地方，都可以用节点的编号、名字或者虚拟 IP。
#[derive(Subcommand)]
pub enum Command {
    /// controller 自己的情况；也用来检查地址和令牌对不对
    Status,
    /// 当前网络的总览：在线的节点、各样东西的数量、互相到不了的节点对
    Overview,
    /// 操作记录
    Log {
        /// 最多看多少条
        #[arg(long, default_value_t = 30)]
        limit: u32,
        /// 只看这种操作，或者以它开头的，例如 node、credential.create
        #[arg(long)]
        action: Option<String>,
        /// 只看提到这样东西的，写成 <种类>:<编号>，例如 node:3、credential:1
        #[arg(long = "ref", value_name = "KIND:ID")]
        reference: Option<String>,
        /// 只看编号比它小的（翻页）
        #[arg(long)]
        before: Option<u64>,
    },
    /// 在当前网络里把一个标签改名：节点上的、规则里的、还没用掉的注册令牌里的
    #[command(subcommand)]
    Tag(TagCommand),
    /// 网络
    #[command(subcommand)]
    Network(NetworkCommand),
    /// 节点
    #[command(subcommand)]
    Node(NodeCommand),
    /// 把节点启用为 relay，或者停用
    #[command(subcommand)]
    Relay(RelayCommand),
    /// 把节点启用为出口，或者停用
    #[command(subcommand)]
    Exit(ExitCommand),
    /// 注册令牌
    #[command(subcommand)]
    Token(TokenCommand),
    /// 访问规则
    #[command(subcommand)]
    Rule(RuleCommand),
    /// 接入凭据
    #[command(subcommand)]
    Credential(CredentialCommand),
    /// 代理监听器
    #[command(subcommand)]
    Listener(ListenerCommand),
    /// 端口转发
    #[command(subcommand)]
    Forward(ForwardCommand),
    /// 节点对设置
    #[command(subcommand)]
    Pair(PairCommand),
    /// 访问关系：谁能访问谁
    #[command(subcommand)]
    Access(AccessCommand),
    /// 节点程序的更新：设置、查发布源、让节点立即更新
    #[command(subcommand)]
    Update(UpdateCommand),
    /// A 和 B 之间两个方向现在各走哪条路。先问两个节点，问不到的方向用它最近上报的
    Path { a: String, b: String },
    /// 整个网络的路由矩阵：每个节点到它的各个对端现在怎么走，取自各节点最近的上报
    Routes,
}

#[derive(Subcommand)]
pub enum NetworkCommand {
    /// 列出全部网络
    List,
    /// 新建一个网络。它自带一条 all <-> all 规则
    Create {
        /// 小写字母、数字和连字符。节点的全名是 <节点>.<网络>.i3t
        name: String,
        #[command(flatten)]
        settings: NetworkSettings,
    },
    /// 当前网络的设置
    Show,
    /// 改当前网络的设置
    Set {
        #[command(flatten)]
        settings: NetworkSettings,
    },
    /// 删掉一个空的网络
    Delete {
        /// 网络的名字或编号
        network: String,
    },
}

#[derive(Args)]
pub struct NetworkSettings {
    /// 虚拟地址段。网络里有了节点之后只能扩大
    #[arg(long)]
    pub cidr: Option<Ipv4Net>,
    /// 默认的选路模式
    #[arg(long)]
    pub routing: Option<RoutingArg>,
    /// 一条路径最多经过几个 relay，1 到 4
    #[arg(long)]
    pub max_relay_hops: Option<u8>,
    /// 对可用路径的限制
    #[arg(long)]
    pub reach: Option<ReachArg>,
    /// 默认的服务等级；default 表示 auto
    #[arg(long)]
    pub class: Option<Inherit<ClassArg>>,
}

#[derive(Subcommand)]
pub enum TagCommand {
    /// 把 FROM 改名为 TO
    Rename { from: String, to: String },
}

#[derive(Subcommand)]
pub enum NodeCommand {
    /// 列出当前网络的节点
    List,
    /// 一个节点的详情
    Show { node: String },
    /// 改名
    Rename { node: String, name: String },
    /// 整个替换节点的标签；不写标签就是清空
    Tag { node: String, tags: Vec<String> },
    /// 改节点的虚拟 IP 和各个开关
    Set {
        node: String,
        /// 换一个虚拟 IP。引用旧地址的配置会失效
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        /// 只拨出：不接受别人主动发来的连接
        #[arg(long, value_name = "on|off")]
        dial_out_only: Option<OnOff>,
        /// 选路模式；default 表示用网络的
        #[arg(long)]
        routing: Option<Inherit<RoutingArg>>,
        /// 对可用路径的限制；default 表示用网络的
        #[arg(long)]
        reach: Option<Inherit<ReachArg>>,
        /// 固定挂靠到这个 relay，可以写多次
        #[arg(long = "pin", value_name = "NODE")]
        pins: Vec<String>,
        /// 不再固定挂靠的 relay
        #[arg(long, conflicts_with = "pins")]
        unpin: bool,
    },
    /// 删掉节点：吊销它的公钥，它的虚拟 IP 和名字进入冷却期
    Delete { node: String },
    /// 把 NODE 指定为已下线节点 PREDECESSOR 的继任：继承它的虚拟 IP、名字、标签和指向它的配置
    Succeed { node: String, predecessor: String },
    /// 节点最近上报的路由表：它到各个对端现在怎么走
    Routes { node: String },
    /// 这个节点能访问谁、谁能访问它、各是哪条规则允许的
    Access { node: String },
    /// 节点上的全部监听：下发的监听器、它自己开的回环入口、端口转发；状态、连接数、流量
    Listeners { node: String },
}

#[derive(Subcommand)]
pub enum RelayCommand {
    /// 启用 relay，或者改它的设置。节点要先在本地声明（i3t set can-relay on）
    Enable {
        node: String,
        /// 从公网能连上的 UDP 地址，可以写多次。--udp 和 --tcp 至少要有一个
        #[arg(long)]
        udp: Vec<SocketAddr>,
        /// 从公网能连上的 TCP 地址，给 UDP 用不了的节点用，可以写多次
        #[arg(long)]
        tcp: Vec<SocketAddr>,
        /// 所属区域，随便起名
        #[arg(long, default_value = "")]
        region: String,
        /// 转发的带宽上限（Mbps）；不写表示不限
        #[arg(long)]
        bandwidth: Option<u32>,
    },
    /// 停用 relay
    Disable { node: String },
}

#[derive(Subcommand)]
pub enum ExitCommand {
    /// 启用出口，或者改它的设置。节点要先在本地声明（i3t set can-exit on）
    Enable {
        node: String,
        /// 互联网目标允许的端口，逗号分隔；any 表示不限制
        #[arg(long, default_value = "80,443")]
        ports: PortList,
        /// 允许经它访问它所在的局域网
        #[arg(long)]
        lan: bool,
        /// 带宽上限（Mbps）；不写表示不限
        #[arg(long)]
        bandwidth: Option<u32>,
        /// 把流交给节点本机的这个 SOCKS5 代理去连目标
        #[arg(long)]
        upstream_socks5: Option<SocketAddr>,
        /// 每个账期的流量配额，例如 500G、2T；不写表示不限
        #[arg(long)]
        quota: Option<ByteSize>,
        /// 账期从每月的哪一天开始（UTC），1 到 28
        #[arg(long, default_value_t = 1)]
        reset_day: u8,
        /// 只连这种地址
        #[arg(long, default_value = "any")]
        family: FamilyArg,
        /// 同一个发起节点每分钟最多让它向同一个目标新建多少条连接；不写表示不限
        #[arg(long)]
        connects_per_minute: Option<u32>,
        /// 经它出去的流的默认服务等级
        #[arg(long)]
        class: Option<ClassArg>,
    },
    /// 停用出口
    Disable { node: String },
    /// 启用了的出口：设置、它自己报的计数和配额、健康、用它的节点看到的
    List,
}

#[derive(Subcommand)]
pub enum TokenCommand {
    /// 列出还没用掉的注册令牌
    List {
        /// 连用掉的和过期的也列出来
        #[arg(long)]
        all: bool,
    },
    /// 生成一个注册令牌。令牌文本只显示这一次，在要加入的机器上交给 i3t join
    Create {
        /// 多久之后过期，例如 30m、24h、7d
        #[arg(long, default_value = "24h")]
        expires: Lifetime,
        /// 用它加入的节点自动带上的标签，可以写多次
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// 预设的虚拟 IP
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        /// 用它加入的节点继任这个已下线的节点
        #[arg(long, value_name = "NODE")]
        successor_of: Option<String>,
    },
    /// 收回一个还没用掉的令牌
    Delete { id: u32 },
}

#[derive(Subcommand)]
pub enum RuleCommand {
    /// 列出访问规则
    List,
    /// 加一条规则：带标签 SRC 的节点和带标签 DST 的节点可以互相访问
    Add {
        src: String,
        dst: String,
        /// 只有 SRC 一侧能发起（SRC -> DST）
        #[arg(long)]
        one_way: bool,
        #[arg(long, default_value = "")]
        name: String,
        /// 先建成停用的
        #[arg(long)]
        disabled: bool,
    },
    /// 启用一条规则
    Enable { id: u32 },
    /// 停用一条规则
    Disable { id: u32 },
    /// 删掉一条规则
    Delete { id: u32 },
}

#[derive(Subcommand)]
pub enum CredentialCommand {
    /// 列出接入凭据
    List,
    /// 签发一个凭据。令牌只显示这一次，调用者把它填在代理的密码字段里
    Create {
        name: String,
        /// 同时进行的连接数上限；不写表示不限
        #[arg(long)]
        max_concurrent: Option<u32>,
        /// 流量上限，例如 10G；不写表示不限
        #[arg(long)]
        max_bytes: Option<ByteSize>,
    },
    /// 改一个凭据的上限，不用重新签发令牌
    Set {
        id: u32,
        /// 同时进行的连接数上限；none 表示不限
        #[arg(long)]
        max_concurrent: Option<Limit<u32>>,
        /// 流量上限，例如 10G；none 表示不限
        #[arg(long)]
        max_bytes: Option<Limit<ByteSize>>,
        /// 用量清零
        #[arg(long)]
        reset_usage: bool,
    },
    /// 吊销一个凭据；对已经吊销的再执行一次就彻底删掉
    Delete { id: u32 },
}

#[derive(Subcommand)]
pub enum ListenerCommand {
    /// 列出监听器
    List {
        /// 只看这个节点上的
        #[arg(long)]
        node: Option<String>,
    },
    /// 在 NODE 上加一个代理入口，监听在 BIND
    Add {
        node: String,
        name: String,
        bind: SocketAddr,
        /// HTTP 代理；不写是 SOCKS5
        #[arg(long)]
        http: bool,
        /// 来源白名单，可以写多次；不写表示不限制
        #[arg(long)]
        allow_from: Vec<IpNet>,
        /// 要求调用者在密码字段里带凭据。绑定在非回环地址上时必须要求
        #[arg(long)]
        require_credential: bool,
        /// 没写用户名、目标又不是组网地址时怎么办：local、reject 或 exit:<NODE>
        #[arg(long = "default", value_name = "ACTION")]
        default_action: Option<String>,
        /// 这个监听器上的流量都从这个节点出去
        #[arg(long, value_name = "NODE")]
        fixed_exit: Option<String>,
        /// 这个监听器上的流的默认服务等级
        #[arg(long)]
        class: Option<ClassArg>,
        /// 乐观回复：on 时不等对端的应答就告诉客户端连上了，off 时一律等；不写按目标的种类
        #[arg(long, value_name = "on|off")]
        optimistic: Option<OnOff>,
    },
    /// 删掉一个监听器
    Delete { id: u32 },
}

#[derive(Subcommand)]
pub enum ForwardCommand {
    /// 列出端口转发和它们的状态
    List,
    /// 在 LISTEN_NODE 的 BIND 上监听，转发到 TARGET_NODE 去连的 TARGET_ADDR。BIND 的端口写 0 由监听节点挑一个空闲的。
    /// 目标节点要先在本地声明（i3t set can-forward on）
    Add {
        listen_node: String,
        bind: SocketAddr,
        target_node: String,
        target_addr: SocketAddr,
        /// 转发 UDP；不写是 TCP
        #[arg(long)]
        udp: bool,
        #[arg(long, default_value = "")]
        name: String,
        /// 来源白名单，可以写多次；不写表示不限制
        #[arg(long)]
        allow_from: Vec<IpNet>,
        /// 先建成停用的
        #[arg(long)]
        disabled: bool,
        /// 这条转发上的流的服务等级
        #[arg(long)]
        class: Option<ClassArg>,
    },
    /// 启用一条转发
    Enable { id: u32 },
    /// 停用一条转发
    Disable { id: u32 },
    /// 删掉一条转发
    Delete { id: u32 },
}

#[derive(Subcommand)]
pub enum PairCommand {
    /// 列出节点对设置
    List,
    /// 设置一对节点
    Set {
        a: String,
        b: String,
        /// 这一对节点之间的连接常驻，不因为没有流而关闭
        #[arg(long, value_name = "on|off")]
        keep_alive: OnOff,
    },
    /// 去掉一对节点的设置
    Delete { a: String, b: String },
}

#[derive(Subcommand)]
pub enum AccessCommand {
    /// 标签、规则，以及没有被任何规则连通的节点
    Graph,
    /// FROM 能不能访问 TO，是哪条规则允许的
    Check { from: String, to: String },
}

#[derive(Subcommand)]
pub enum UpdateCommand {
    /// 更新的设置和现状：自动更新的开关、发布源、已知的最新版、被暂停的版本
    Show,
    /// 改设置
    Set {
        /// 自动更新的开关。开着时，有新版本的节点在空闲的时候自己更新
        #[arg(long, value_name = "on|off")]
        auto: Option<OnOff>,
        /// 发布源：一个 https:// 或 http:// 地址，或者 controller 所在机器上的一个目录；default 表示项目的发布源
        #[arg(long, value_name = "地址|目录|default")]
        source: Option<String>,
    },
    /// 现在就查一次发布源
    Check,
    /// 各个节点的更新情况：有没有新版本、能不能自己更新、进行到哪一步
    Nodes,
    /// 让一个节点立即更新，不管自动更新开没开。默认马上重启，正在进行的流会断
    Node {
        node: String,
        /// 排空后更新：节点不再接新的流，等手上的流结束再重启
        #[arg(long)]
        drain: bool,
    },
    /// 让全部节点立即更新。relay 仍然一个一个来
    All {
        /// 排空后更新
        #[arg(long)]
        drain: bool,
    },
    /// 继续一个因为有节点更新失败而被暂停的版本的自动更新
    Resume {
        /// 版本号，例如 0.3.0
        version: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum RoutingArg {
    /// 直连可用就走直连
    DirectFirst,
    /// 直连和各条 relay 路径比较，取代价最低的
    LowestCost,
}

impl From<RoutingArg> for RoutingMode {
    fn from(arg: RoutingArg) -> Self {
        match arg {
            RoutingArg::DirectFirst => Self::DirectFirst,
            RoutingArg::LowestCost => Self::LowestCost,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ClassArg {
    Auto,
    /// 尾延迟最重要
    Interactive,
    /// 一个报文都不丢
    Reliable,
    /// 吞吐优先
    Bulk,
}

impl From<ClassArg> for Class {
    fn from(arg: ClassArg) -> Self {
        match arg {
            ClassArg::Auto => Self::Auto,
            ClassArg::Interactive => Self::Interactive,
            ClassArg::Reliable => Self::Reliable,
            ClassArg::Bulk => Self::Bulk,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum FamilyArg {
    Any,
    Ipv4,
    Ipv6,
}

impl From<FamilyArg> for IpFamily {
    fn from(arg: FamilyArg) -> Self {
        match arg {
            FamilyArg::Any => Self::Any,
            FamilyArg::Ipv4 => Self::Ipv4,
            FamilyArg::Ipv6 => Self::Ipv6,
        }
    }
}

/// 字节数：数字加可选的单位 K、M、G、T（按 1000 进位，和网络流量的计法一样）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteSize(pub u64);

impl std::str::FromStr for ByteSize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let text = s.trim().trim_end_matches(['B', 'b']);
        let (digits, scale) = match text.char_indices().last() {
            Some((index, unit)) if unit.is_ascii_alphabetic() => {
                let scale: u64 = match unit.to_ascii_uppercase() {
                    'K' => 1_000,
                    'M' => 1_000_000,
                    'G' => 1_000_000_000,
                    'T' => 1_000_000_000_000,
                    _ => return Err("单位要是 K、M、G 或 T，例如 500M、10G".to_owned()),
                };
                (&text[..index], scale)
            }
            _ => (text, 1),
        };
        let count: u64 = digits.trim().parse().map_err(|_| "要写成数字加单位，例如 500M、10G".to_owned())?;
        count.checked_mul(scale).map(Self).ok_or_else(|| "太大了".to_owned())
    }
}

/// 一个可以清掉的上限：写一个值，或者写 `none` 表示不限。
#[derive(Clone, Copy)]
pub enum Limit<T> {
    None,
    Value(T),
}

impl<T: std::str::FromStr<Err: std::fmt::Display>> std::str::FromStr for Limit<T> {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "none" { Ok(Self::None) } else { s.parse().map(Self::Value).map_err(|err| format!("{err}；或者写 none")) }
    }
}

impl<T> Limit<T> {
    /// 管理 API 里的写法：`None` 表示不限。
    pub fn into_option<U: From<T>>(self) -> Option<U> {
        match self {
            Self::None => None,
            Self::Value(value) => Some(U::from(value)),
        }
    }
}

impl From<ByteSize> for u64 {
    fn from(size: ByteSize) -> Self {
        size.0
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ReachArg {
    Any,
    /// 不使用 relay
    DirectOnly,
    /// 不建立直连，不向对端暴露公网地址
    RelayOnly,
}

impl From<ReachArg> for Reach {
    fn from(arg: ReachArg) -> Self {
        match arg {
            ReachArg::Any => Self::Any,
            ReachArg::DirectOnly => Self::DirectOnly,
            ReachArg::RelayOnly => Self::RelayOnly,
        }
    }
}

/// 节点上可以覆盖网络默认值的设置：写一个具体的值，或者写 `default` 改回用网络的。
#[derive(Clone, Copy)]
pub enum Inherit<T> {
    Default,
    Value(T),
}

impl<T: ValueEnum> std::str::FromStr for Inherit<T> {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "default" {
            Ok(Self::Default)
        } else {
            T::from_str(s, true).map(Self::Value).map_err(|err| format!("{err}；或者写 default"))
        }
    }
}

impl<T> Inherit<T> {
    /// 管理 API 里的写法：`None` 表示清掉覆盖值。
    pub fn into_override<U: From<T>>(self) -> Option<U> {
        match self {
            Self::Default => None,
            Self::Value(value) => Some(U::from(value)),
        }
    }
}

#[derive(Clone, Copy)]
pub struct OnOff(pub bool);

impl std::str::FromStr for OnOff {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "on" => Ok(Self(true)),
            "off" => Ok(Self(false)),
            _ => Err("要写 on 或 off".to_owned()),
        }
    }
}

/// 一段时间：数字加单位 s、m、h、d；只写数字按秒算。
#[derive(Clone, Copy)]
pub struct Lifetime {
    pub secs: u64,
}

impl std::str::FromStr for Lifetime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (digits, unit) = match s.char_indices().last() {
            Some((index, unit)) if unit.is_ascii_alphabetic() => (&s[..index], unit),
            _ => (s, 's'),
        };
        let scale: u64 = match unit {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86400,
            _ => return Err("单位要是 s、m、h 或 d，例如 30m、24h、7d".to_owned()),
        };
        let count: u64 = digits.parse().map_err(|_| "要写成数字加单位，例如 30m、24h、7d".to_owned())?;
        count.checked_mul(scale).map(|secs| Self { secs }).ok_or_else(|| "时间太长了".to_owned())
    }
}

/// 出口允许的端口：一组端口，或者 `any`。
#[derive(Clone)]
pub struct PortList(pub Option<Vec<u16>>);

impl std::str::FromStr for PortList {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "any" {
            return Ok(Self(None));
        }
        let ports =
            s.split(',').map(|port| port.trim().parse::<u16>().map_err(|_| format!("{port:?} 不是端口号；写成 80,443 这样，或者 any")));
        ports.collect::<Result<Vec<_>, _>>().map(|ports| Self(Some(ports)))
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn lifetimes_ports_and_switches_parse() {
        let secs = |text: &str| text.parse::<Lifetime>().map(|lifetime| lifetime.secs);
        assert_eq!((secs("90"), secs("30m"), secs("24h"), secs("7d")), (Ok(90), Ok(1800), Ok(86400), Ok(604_800)));
        assert!(secs("soon").is_err() && secs("5w").is_err() && secs("").is_err());
        assert_eq!("80, 443".parse::<PortList>().unwrap().0, Some(vec![80, 443]));
        assert_eq!("any".parse::<PortList>().unwrap().0, None);
        assert!("http".parse::<PortList>().is_err());
        assert!("on".parse::<OnOff>().unwrap().0 && !"off".parse::<OnOff>().unwrap().0 && "yes".parse::<OnOff>().is_err());
    }

    #[test]
    fn node_overrides_accept_a_value_or_default() {
        let parse = |text: &str| text.parse::<Inherit<RoutingArg>>().map(Inherit::into_override::<RoutingMode>);
        assert_eq!(parse("default"), Ok(None));
        assert_eq!(parse("lowest-cost"), Ok(Some(RoutingMode::LowestCost)));
        assert!(parse("fastest").is_err());
    }

    #[test]
    fn update_and_path_commands_parse() {
        let parse = |args: &[&str]| Cli::try_parse_from([&["i3tctl"], args].concat()).unwrap().command;
        assert!(matches!(parse(&["update", "node", "nas", "--drain"]), Command::Update(UpdateCommand::Node { drain: true, .. })));
        assert!(matches!(parse(&["update", "all"]), Command::Update(UpdateCommand::All { drain: false })));
        assert!(matches!(
            parse(&["update", "set", "--auto", "off"]),
            Command::Update(UpdateCommand::Set { auto: Some(OnOff(false)), source: None })
        ));
        assert!(matches!(parse(&["path", "a", "b"]), Command::Path { .. }));
        assert!(matches!(parse(&["routes"]), Command::Routes));
        assert!(Cli::try_parse_from(["i3tctl", "update", "set", "--auto", "maybe"]).is_err());
    }

    #[test]
    fn sizes_limits_and_the_new_commands_parse() {
        assert_eq!("500M".parse::<ByteSize>(), Ok(ByteSize(500_000_000)));
        assert_eq!("2t".parse::<ByteSize>(), Ok(ByteSize(2_000_000_000_000)));
        assert_eq!("1234".parse::<ByteSize>(), Ok(ByteSize(1234)));
        assert_eq!("10GB".parse::<ByteSize>(), Ok(ByteSize(10_000_000_000)));
        assert!("10X".parse::<ByteSize>().is_err() && "lots".parse::<ByteSize>().is_err());
        assert!(matches!("none".parse::<Limit<ByteSize>>(), Ok(Limit::None)));
        assert!(matches!("1K".parse::<Limit<ByteSize>>(), Ok(Limit::Value(ByteSize(1000)))));
        let parse = |args: &[&str]| Cli::try_parse_from([&["i3tctl"], args].concat()).unwrap().command;
        assert!(matches!(
            parse(&["credential", "set", "3", "--max-bytes", "none", "--reset-usage"]),
            Command::Credential(CredentialCommand::Set { id: 3, reset_usage: true, .. })
        ));
        assert!(matches!(
            parse(&["exit", "enable", "gw", "--quota", "1G", "--family", "ipv4"]),
            Command::Exit(ExitCommand::Enable { quota: Some(ByteSize(1_000_000_000)), .. })
        ));
        assert!(matches!(parse(&["tag", "rename", "a", "b"]), Command::Tag(TagCommand::Rename { .. })));
        assert!(matches!(parse(&["token", "list", "--all"]), Command::Token(TokenCommand::List { all: true })));
        assert!(matches!(parse(&["log", "--ref", "node:3", "--action", "node"]), Command::Log { .. }));
        assert!(matches!(parse(&["node", "listeners", "gw"]), Command::Node(NodeCommand::Listeners { .. })));
    }

    #[test]
    fn global_flags_can_follow_the_subcommand() {
        let cli = Cli::try_parse_from(["i3tctl", "node", "list", "--json", "--network", "home", "--url", "http://127.0.0.1:1"]).unwrap();
        assert!(cli.json);
        assert_eq!((cli.network.as_deref(), cli.url.as_str()), (Some("home"), "http://127.0.0.1:1"));
        assert!(matches!(cli.command, Command::Node(NodeCommand::List)));
    }
}
