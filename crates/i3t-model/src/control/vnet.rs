//! 虚拟网卡在一个节点上的状况：节点在上报里带着，controller 原样放进管理接口的节点详情，本机的状态接口也是它。
//!
//! 默认打开，所以没有装上时也报，用代码说清楚为什么（[`VnetCause`]），界面按自己的语言写。

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// 虚拟网卡的状况和这一路的计数。旧版本的节点不报。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VnetReport {
    #[serde(flatten)]
    pub state: VnetState,
    /// 程序加的两条防火墙规则。网卡没有装上、或者本机的开关关着时两条都是 `off`。
    #[serde(default)]
    pub firewall: Firewall,
    /// 这次运行以来经虚拟网卡发给别的节点、从别的节点收进来（写进网卡）的包和字节（IP 包的长度）。
    #[serde(default)]
    pub sent_packets: u64,
    #[serde(default)]
    pub sent_bytes: u64,
    #[serde(default)]
    pub received_packets: u64,
    #[serde(default)]
    pub received_bytes: u64,
    /// 这次运行以来丢掉的包，按原因。
    #[serde(default)]
    pub dropped: VnetDrops,
}

/// 虚拟网卡装上了没有。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VnetState {
    /// 装好了：网卡 `device` 上是本节点的虚拟 IP，网络的地址段经它走，本机的程序直接用虚拟 IP 访问别的节点。
    Installed {
        /// 网卡的名字（`i3t0`）。
        device: String,
        mtu: u16,
        names: NameService,
    },
    /// 本机关着（配置文件的 `[network] enabled`，`i3t set network off`）。
    Off,
    /// 本机开着，但是装不上。系统里什么都没有留下；代理和端口转发照常。
    Failed { cause: VnetCause },
    /// 这个平台上没有（Linux、Windows、macOS 之外的）。
    Unsupported,
    /// 比这个程序新的节点报的、这里不认识的状态。
    #[serde(other)]
    Unknown,
}

/// 虚拟网卡装着时，`<节点>.<网络的域名>` 怎么解析。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NameService {
    /// systemd-resolved 把网络的域名交给本节点的名字服务：本机的程序直接用名字就行。
    Resolved,
    /// 名字服务开着（本节点虚拟 IP 的 53 端口），这台机器没有 systemd-resolved：名字要写进 `/etc/hosts`（`i3t hosts`），
    /// 或者让本机的解析器把网络的域名转给它。
    NoResolver,
    /// 名字服务没有开起来，或者 systemd-resolved 不接受设置。按虚拟 IP 访问照常可用。
    Failed { cause: VnetCause },
    #[serde(other)]
    Unknown,
}

/// 虚拟网卡（或者它的一部分：防火墙规则、名字服务）装不上的原因，给界面按自己的语言写。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum VnetCause {
    /// 节点进程缺这些能力：`CAP_NET_ADMIN`、`CAP_NET_BIND_SERVICE`（写法和 systemd 的 `AmbientCapabilities=` 一样）。
    NoPermission { missing: Vec<String> },
    /// 打不开建虚拟网卡用的设备（Linux 上是 `/dev/net/tun`），`detail` 是系统的原话。容器里常见：要把这个设备给它。
    NoDevice { detail: String },
    /// 网卡建不起来，`detail` 是系统的原话。
    DeviceFailed { detail: String },
    /// Windows：程序旁边没有 Wintun 的驱动文件（`wintun.dll`），`path` 是找的位置。
    DriverMissing { path: String },
    /// 系统里没有这个程序：`ip`（iproute2）、`nft`（nftables）、`iptables`。
    ProgramMissing { program: String },
    /// 一个程序报错退出，`detail` 是它的原话。
    CommandFailed { program: String, detail: String },
    /// 这个地址的端口被占着（名字服务的 53 端口）。
    PortInUse { address: SocketAddr },
    /// 别的原因绑不上这个地址，`detail` 是系统的原话。
    BindFailed { address: SocketAddr, detail: String },
    /// 网络的地址段和这台机器所在的一个真实网络重叠，装上会把它从那个网络上切下来。`with` 是重叠的那个地址或者路由，
    /// 写法和 `ip` 一样：`192.168.1.5/24 dev eth0`、`10.0.0.0/8 dev tun0`。
    RangeInUse { with: String },
    /// 比这个程序新的节点报的、这里不认识的原因。
    #[serde(other)]
    Unknown,
}

/// 程序加的两条防火墙规则（做法同 Tailscale）。本机一个开关（`[network] firewall`）关掉之后一条也不碰。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Firewall {
    /// 从虚拟网卡进来的放行：插在系统 INPUT 链的最前面（iptables），默认拒绝入站的防火墙因此不挡别的节点。
    #[serde(default)]
    pub accept: RuleState,
    /// 自称来自网络地址段、却不是从虚拟网卡进来的包丢掉：在程序自己的一张 nftables 表里，不改用户的规则。
    #[serde(default)]
    pub guard: RuleState,
}

/// 一条防火墙规则装上了没有。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RuleState {
    On,
    /// 没有装：本机的开关关着，或者网卡没有装上。
    #[default]
    Off,
    /// 装不上。网卡照样装着，这条规则管的事由这台机器自己的防火墙定。
    Failed {
        cause: VnetCause,
    },
    /// 这个平台上程序不加这条规则：macOS 上两条都不加（不碰 pf）；Windows 上不加防冒充的那条（它默认按强主机模型收包）。
    Unsupported,
    #[serde(other)]
    Unknown,
}

/// 丢掉的包，按原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VnetDrops {
    /// 对端发来的包，来源不是那个对端自己的虚拟 IP。
    #[serde(default)]
    pub bad_source: u64,
    /// 对端发来的包，目的不是本节点的虚拟 IP。
    #[serde(default)]
    pub bad_destination: u64,
    /// 访问规则不允许对端访问本节点：它发起的新会话（TCP 的第一个握手包、ping 的请求、没有先例的 UDP 和别的协议）。
    #[serde(default)]
    pub not_allowed: u64,
    /// 本机发出的包送不到：地址段里不是节点的地址、没有网卡的节点、连不上的节点，等连接的时候排不下。
    #[serde(default)]
    pub unreachable: u64,
    /// 读不懂的：不是 IPv4、头不对、片拼不起来。
    #[serde(default)]
    pub malformed: u64,
}

impl VnetDrops {
    /// 全部加起来。
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.bad_source
            .saturating_add(self.bad_destination)
            .saturating_add(self.not_allowed)
            .saturating_add(self.unreachable)
            .saturating_add(self.malformed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Report;

    #[test]
    fn the_report_is_a_state_code_with_its_values_and_survives_newer_codes() {
        let installed = VnetReport {
            state: VnetState::Installed { device: "i3t0".into(), mtu: 1280, names: NameService::Resolved },
            firewall: Firewall { accept: RuleState::On, guard: RuleState::On },
            sent_packets: 3,
            sent_bytes: 300,
            received_packets: 2,
            received_bytes: 200,
            dropped: VnetDrops { not_allowed: 1, ..VnetDrops::default() },
        };
        let json = serde_json::to_value(&installed).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "state": "installed", "device": "i3t0", "mtu": 1280, "names": { "state": "resolved" },
                "firewall": { "accept": { "state": "on" }, "guard": { "state": "on" } },
                "sent_packets": 3, "sent_bytes": 300, "received_packets": 2, "received_bytes": 200,
                "dropped": { "bad_source": 0, "bad_destination": 0, "not_allowed": 1, "unreachable": 0, "malformed": 0 }
            })
        );
        assert_eq!(serde_json::from_value::<VnetReport>(json).unwrap(), installed);

        let failed = VnetState::Failed { cause: VnetCause::NoPermission { missing: vec!["CAP_NET_ADMIN".into()] } };
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json, serde_json::json!({ "state": "failed", "cause": { "code": "no_permission", "missing": ["CAP_NET_ADMIN"] } }));
        let overlap = VnetCause::RangeInUse { with: "192.168.1.5/24 dev eth0".into() };
        assert_eq!(
            serde_json::to_value(&overlap).unwrap(),
            serde_json::json!({ "code": "range_in_use", "with": "192.168.1.5/24 dev eth0" })
        );

        let newer: VnetReport = serde_json::from_value(serde_json::json!({ "state": "paused" })).unwrap();
        assert_eq!(newer.state, VnetState::Unknown);
        assert_eq!(newer.firewall, Firewall::default(), "没报规则的按关着");
        let newer: VnetReport = serde_json::from_value(serde_json::json!({
            "state": "failed", "cause": { "code": "solar_flare" }, "firewall": { "accept": { "state": "half" } }
        }))
        .unwrap();
        assert_eq!(newer.state, VnetState::Failed { cause: VnetCause::Unknown });
        assert_eq!(newer.firewall.accept, RuleState::Unknown);
        let older: Report = serde_json::from_value(serde_json::json!({ "intercept": { "state": "on" } })).unwrap();
        assert_eq!(older.vnet, None, "旧的节点不报；它报的透明访问没有人再看");
    }

    #[test]
    fn drops_add_up() {
        let drops = VnetDrops { bad_source: 1, bad_destination: 2, not_allowed: 3, unreachable: 4, malformed: 5 };
        assert_eq!(drops.total(), 15);
        assert_eq!(VnetDrops { malformed: u64::MAX, unreachable: 1, ..VnetDrops::default() }.total(), u64::MAX);
    }
}
