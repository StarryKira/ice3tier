//! 每个子命令做什么：解析出网络和节点，发请求，交给 [`crate::render`] 打印。

use std::net::Ipv4Addr;

use anyhow::{Context, anyhow, bail};
use i3t_model::admin::{
    CreateCredential, CreateEnrollToken, CreateNetwork, CreateRule, Direction, ForceUpdate, Forward, ForwardInput, ListenerInput, Network,
    NetworkId, Node, PairInput, RelaySettings, ResumeUpdate, Succeed, UpdateNetwork, UpdateNode, UpdateRule, UpdateSettings,
};
use i3t_model::netmap::{DefaultAction, ExitRole, ForwardProtocol, Listener, ListenerProtocol};
use i3t_model::update::UpdateTiming;
use i3t_proto::NodeId;
use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::cli::{
    AccessCommand, Cli, Command, CredentialCommand, ExitCommand, ForwardCommand, ListenerCommand, NetworkCommand, NetworkSettings,
    NodeCommand, PairCommand, RelayCommand, RuleCommand, TokenCommand, UpdateCommand,
};
use crate::client::{Client, decode};
use crate::render;

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let token = cli.token.context("没有管理令牌。用 --token 或环境变量 I3T_ADMIN_TOKEN 给出 controller 第一次启动时打印的那个")?;
    let session = Session { client: Client::new(&cli.url, token)?, network: cli.network, json: cli.json };
    match cli.command {
        Command::Status => session.show(session.get("/status").await?, render::status),
        Command::Overview => {
            let network = session.network().await?;
            session.show(session.get(&format!("/networks/{}/overview", network.id)).await?, render::overview)
        }
        Command::Log { limit } => session.show_list(session.get(&format!("/log?limit={limit}")).await?, render::log),
        Command::Network(command) => session.network_command(command).await,
        Command::Node(command) => session.node_command(command).await,
        Command::Relay(command) => session.relay_command(command).await,
        Command::Exit(command) => session.exit_command(command).await,
        Command::Token(command) => session.token_command(command).await,
        Command::Rule(command) => session.rule_command(command).await,
        Command::Credential(command) => session.credential_command(command).await,
        Command::Listener(command) => session.listener_command(command).await,
        Command::Forward(command) => session.forward_command(command).await,
        Command::Pair(command) => session.pair_command(command).await,
        Command::Access(command) => session.access_command(command).await,
        Command::Update(command) => session.update_command(command).await,
        Command::Path { a, b } => {
            let network = session.network().await?.id;
            let (a, b) = (session.node(network, &a).await?.id, session.node(network, &b).await?.id);
            session.show(session.get(&format!("/networks/{network}/path?from={a}&to={b}")).await?, render::path)
        }
        Command::Routes => {
            let network = session.network().await?.id;
            session.show(session.get(&format!("/networks/{network}/routes")).await?, render::route_matrix)
        }
    }
}

struct Session {
    client: Client,
    /// `--network` 的值：名字或编号。
    network: Option<String>,
    json: bool,
}

impl Session {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.client.request(Method::GET, path, None::<&()>).await
    }

    async fn send<B: Serialize + Sync>(&self, method: Method, path: &str, body: &B) -> anyhow::Result<Value> {
        self.client.request(method, path, Some(body)).await
    }

    async fn delete(&self, path: &str) -> anyhow::Result<Value> {
        self.client.request(Method::DELETE, path, None::<&()>).await
    }

    /// 打印一个应答：`--json` 时原样输出，否则读成模型里的类型交给 `human`。
    fn show<T: DeserializeOwned>(&self, value: Value, human: impl FnOnce(&T)) -> anyhow::Result<()> {
        if self.json {
            if !value.is_null() {
                println!("{}", serde_json::to_string_pretty(&value).context("输出 JSON")?);
            }
        } else {
            human(&decode(value)?);
        }
        Ok(())
    }

    /// 同 [`Session::show`]，应答是一个列表。
    fn show_list<T: DeserializeOwned>(&self, value: Value, human: impl FnOnce(&[T])) -> anyhow::Result<()> {
        self.show(value, |items: &Vec<T>| human(items))
    }

    /// 没有应答体的操作（删除）做完之后说一声；`--json` 时什么都不输出。
    fn done(&self, message: &str) {
        if !self.json {
            println!("{message}");
        }
    }

    /// 当前操作的网络：`--network` 指的那个；没写的话，只有一个网络时就是它。
    async fn network(&self) -> anyhow::Result<Network> {
        let networks: Vec<Network> = self.client.get("/networks").await?;
        match &self.network {
            Some(reference) => pick_network(networks, reference),
            None => match <[Network; 1]>::try_from(networks) {
                Ok([only]) => Ok(only),
                Err(networks) if networks.is_empty() => bail!("还没有网络。先用 i3tctl network create <名字> 建一个"),
                Err(networks) => {
                    let names = networks.iter().map(|network| network.name.as_str()).collect::<Vec<_>>().join("、");
                    bail!("有多个网络（{names}），用 --network 或环境变量 I3T_NETWORK 指定一个")
                }
            },
        }
    }

    /// 把命令行上写的节点（编号、名字或虚拟 IP）找出来。
    async fn node(&self, network: NetworkId, reference: &str) -> anyhow::Result<Node> {
        let nodes: Vec<Node> = self.client.get(&format!("/networks/{network}/nodes")).await?;
        let by_id = reference.parse::<NodeId>().ok();
        let by_ip = reference.parse::<Ipv4Addr>().ok();
        let found = nodes
            .into_iter()
            .find(|node| Some(node.id) == by_id || Some(node.virtual_ip) == by_ip || node.name.eq_ignore_ascii_case(reference));
        found.ok_or_else(|| anyhow!("这个网络里没有节点 {reference:?}（可以写编号、名字或虚拟 IP）"))
    }

    async fn network_command(&self, command: NetworkCommand) -> anyhow::Result<()> {
        match command {
            NetworkCommand::List => self.show_list(self.get("/networks").await?, render::networks),
            NetworkCommand::Create { name, settings } => {
                let NetworkSettings { cidr, routing, max_relay_hops, reach } = settings;
                let request = CreateNetwork { name, cidr, routing: routing.map(Into::into), max_relay_hops, reach: reach.map(Into::into) };
                self.show(self.send(Method::POST, "/networks", &request).await?, render::network)
            }
            NetworkCommand::Show => {
                let network = self.network().await?;
                self.show(self.get(&format!("/networks/{}", network.id)).await?, render::network)
            }
            NetworkCommand::Set { settings } => {
                let network = self.network().await?;
                let NetworkSettings { cidr, routing, max_relay_hops, reach } = settings;
                let request = UpdateNetwork { cidr, routing: routing.map(Into::into), max_relay_hops, reach: reach.map(Into::into) };
                self.show(self.send(Method::PATCH, &format!("/networks/{}", network.id), &request).await?, render::network)
            }
            NetworkCommand::Delete { network } => {
                let network = pick_network(self.client.get("/networks").await?, &network)?;
                self.delete(&format!("/networks/{}", network.id)).await?;
                self.done(&format!("已删除网络 {}", network.name));
                Ok(())
            }
        }
    }

    async fn node_command(&self, command: NodeCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        let nodes = format!("/networks/{network}/nodes");
        match command {
            NodeCommand::List => self.show_list(self.get(&nodes).await?, render::nodes),
            NodeCommand::Show { node } => {
                let node = self.node(network, &node).await?;
                self.show(self.get(&format!("{nodes}/{}", node.id)).await?, render::node_detail)
            }
            NodeCommand::Rename { node, name } => {
                let node = self.node(network, &node).await?;
                let request = UpdateNode { name: Some(name), ..UpdateNode::default() };
                self.show(self.send(Method::PATCH, &format!("{nodes}/{}", node.id), &request).await?, render::node_changed)
            }
            NodeCommand::Tag { node, tags } => {
                let node = self.node(network, &node).await?;
                let request = UpdateNode { tags: Some(tags), ..UpdateNode::default() };
                self.show(self.send(Method::PATCH, &format!("{nodes}/{}", node.id), &request).await?, render::node_changed)
            }
            NodeCommand::Set { node, ip, dial_out_only, routing, reach, pins, unpin } => {
                let node = self.node(network, &node).await?;
                let mut pinned_relays = if unpin { Some(Vec::new()) } else { None };
                if !pins.is_empty() {
                    let mut relays = Vec::with_capacity(pins.len());
                    for pin in &pins {
                        relays.push(self.node(network, pin).await?.id);
                    }
                    pinned_relays = Some(relays);
                }
                let request = UpdateNode {
                    virtual_ip: ip,
                    dial_out_only: dial_out_only.map(|switch| switch.0),
                    routing: routing.map(crate::cli::Inherit::into_override),
                    reach: reach.map(crate::cli::Inherit::into_override),
                    pinned_relays,
                    ..UpdateNode::default()
                };
                self.show(self.send(Method::PATCH, &format!("{nodes}/{}", node.id), &request).await?, render::node_changed)
            }
            NodeCommand::Delete { node } => {
                let node = self.node(network, &node).await?;
                self.delete(&format!("{nodes}/{}", node.id)).await?;
                self.done(&format!("已删除节点 {}（{}）。它的公钥已吊销，名字和虚拟 IP 进入冷却期", node.name, node.virtual_ip));
                Ok(())
            }
            NodeCommand::Succeed { node, predecessor } => {
                let node = self.node(network, &node).await?;
                let predecessor = self.node(network, &predecessor).await?;
                let request = Succeed { predecessor: predecessor.id };
                self.show(self.send(Method::POST, &format!("{nodes}/{}/succeed", node.id), &request).await?, render::node_changed)
            }
            NodeCommand::Routes { node } => {
                let node = self.node(network, &node).await?;
                self.show(self.get(&format!("{nodes}/{}/routes", node.id)).await?, render::routes)
            }
            NodeCommand::Access { node } => {
                let node = self.node(network, &node).await?;
                self.show(self.get(&format!("{nodes}/{}/access", node.id)).await?, render::node_access)
            }
        }
    }

    async fn relay_command(&self, command: RelayCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        match command {
            RelayCommand::Enable { node, udp, tcp, region, bandwidth } => {
                let node = self.node(network, &node).await?;
                let settings = RelaySettings { udp, tcp, region, bandwidth_mbps: bandwidth };
                self.show(
                    self.send(Method::PUT, &format!("/networks/{network}/nodes/{}/relay", node.id), &settings).await?,
                    render::node_changed,
                )
            }
            RelayCommand::Disable { node } => {
                let node = self.node(network, &node).await?;
                self.show(self.delete(&format!("/networks/{network}/nodes/{}/relay", node.id)).await?, render::node_changed)
            }
        }
    }

    async fn exit_command(&self, command: ExitCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        match command {
            ExitCommand::Enable { node, ports, lan, bandwidth, upstream_socks5 } => {
                let node = self.node(network, &node).await?;
                let settings = ExitRole { ports: ports.0, lan, bandwidth_mbps: bandwidth, upstream_socks5 };
                self.show(
                    self.send(Method::PUT, &format!("/networks/{network}/nodes/{}/exit", node.id), &settings).await?,
                    render::node_changed,
                )
            }
            ExitCommand::Disable { node } => {
                let node = self.node(network, &node).await?;
                self.show(self.delete(&format!("/networks/{network}/nodes/{}/exit", node.id)).await?, render::node_changed)
            }
        }
    }

    async fn token_command(&self, command: TokenCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        let tokens = format!("/networks/{network}/tokens");
        match command {
            TokenCommand::List => self.show_list(self.get(&tokens).await?, render::tokens),
            TokenCommand::Create { expires, tags, ip, successor_of } => {
                let successor_of = match successor_of {
                    Some(reference) => Some(self.node(network, &reference).await?.id),
                    None => None,
                };
                let request = CreateEnrollToken { expires_in_secs: Some(expires.secs), tags, virtual_ip: ip, successor_of };
                self.show(self.send(Method::POST, &tokens, &request).await?, render::created_token)
            }
            TokenCommand::Delete { id } => {
                self.delete(&format!("{tokens}/{id}")).await?;
                self.done(&format!("已收回注册令牌 #{id}"));
                Ok(())
            }
        }
    }

    async fn rule_command(&self, command: RuleCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        let rules = format!("/networks/{network}/rules");
        let set_enabled = |enabled| UpdateRule { enabled: Some(enabled), ..UpdateRule::default() };
        match command {
            RuleCommand::List => self.show_list(self.get(&rules).await?, render::rules),
            RuleCommand::Add { src, dst, one_way, name, disabled } => {
                let direction = if one_way { Direction::OneWay } else { Direction::Both };
                let request = CreateRule { name, src, dst, direction, enabled: !disabled };
                self.show(self.send(Method::POST, &rules, &request).await?, render::rule)
            }
            RuleCommand::Enable { id } => {
                self.show(self.send(Method::PATCH, &format!("{rules}/{id}"), &set_enabled(true)).await?, render::rule)
            }
            RuleCommand::Disable { id } => {
                self.show(self.send(Method::PATCH, &format!("{rules}/{id}"), &set_enabled(false)).await?, render::rule)
            }
            RuleCommand::Delete { id } => {
                self.delete(&format!("{rules}/{id}")).await?;
                self.done(&format!("已删除规则 #{id}"));
                Ok(())
            }
        }
    }

    async fn credential_command(&self, command: CredentialCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        let credentials = format!("/networks/{network}/credentials");
        match command {
            CredentialCommand::List => self.show_list(self.get(&credentials).await?, render::credentials),
            CredentialCommand::Create { name, max_concurrent } => {
                let request = CreateCredential { name, max_concurrent };
                self.show(self.send(Method::POST, &credentials, &request).await?, render::created_credential)
            }
            CredentialCommand::Delete { id } => {
                self.delete(&format!("{credentials}/{id}")).await?;
                self.done(&format!("已吊销凭据 #{id}"));
                Ok(())
            }
        }
    }

    async fn listener_command(&self, command: ListenerCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        let listeners = format!("/networks/{network}/listeners");
        match command {
            ListenerCommand::List { node } => {
                let path = match node {
                    Some(reference) => format!("{listeners}?node={}", self.node(network, &reference).await?.id),
                    None => listeners,
                };
                self.show_list(self.get(&path).await?, render::listeners)
            }
            ListenerCommand::Add { node, name, bind, http, allow_from, require_credential, default_action, fixed_exit } => {
                let node = self.node(network, &node).await?.id;
                let default_action = match default_action.as_deref() {
                    None => None,
                    Some("local") => Some(DefaultAction::Local),
                    Some("reject") => Some(DefaultAction::Reject),
                    Some(other) => {
                        let exit = other
                            .strip_prefix("exit:")
                            .with_context(|| format!("--default 要写 local、reject 或 exit:<NODE>，现在是 {other:?}"))?;
                        Some(DefaultAction::Exit { node: self.node(network, exit).await?.id })
                    }
                };
                let fixed_exit = match fixed_exit {
                    Some(reference) => Some(self.node(network, &reference).await?.id),
                    None => None,
                };
                let protocol = if http { ListenerProtocol::Http } else { ListenerProtocol::Socks5 };
                let config = Listener { name, bind, protocol, allow_from, require_credential, default_action, fixed_exit };
                self.show(self.send(Method::POST, &listeners, &ListenerInput { node, config }).await?, render::listener)
            }
            ListenerCommand::Delete { id } => {
                self.delete(&format!("{listeners}/{id}")).await?;
                self.done(&format!("已删除监听器 #{id}"));
                Ok(())
            }
        }
    }

    async fn forward_command(&self, command: ForwardCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        let forwards = format!("/networks/{network}/forwards");
        match command {
            ForwardCommand::List => self.show_list(self.get(&forwards).await?, render::forwards),
            ForwardCommand::Add { listen_node, bind, target_node, target_addr, udp, name, allow_from, disabled } => {
                let input = ForwardInput {
                    name,
                    protocol: if udp { ForwardProtocol::Udp } else { ForwardProtocol::Tcp },
                    listen_node: self.node(network, &listen_node).await?.id,
                    bind,
                    target_node: self.node(network, &target_node).await?.id,
                    target_addr,
                    allow_from,
                    enabled: !disabled,
                };
                self.show(self.send(Method::POST, &forwards, &input).await?, render::forward)
            }
            ForwardCommand::Enable { id } => self.switch_forward(&forwards, id, true).await,
            ForwardCommand::Disable { id } => self.switch_forward(&forwards, id, false).await,
            ForwardCommand::Delete { id } => {
                self.delete(&format!("{forwards}/{id}")).await?;
                self.done(&format!("已删除转发 #{id}"));
                Ok(())
            }
        }
    }

    /// 启用或停用一条转发。管理 API 只能整条替换，所以先取出来，改掉开关再放回去。
    async fn switch_forward(&self, forwards: &str, id: u32, enabled: bool) -> anyhow::Result<()> {
        let existing: Vec<Forward> = self.client.get(forwards).await?;
        let forward = existing.into_iter().find(|forward| forward.id == id).ok_or_else(|| anyhow!("这个网络里没有转发 #{id}"))?;
        let input = ForwardInput {
            name: forward.name,
            protocol: forward.protocol,
            listen_node: forward.listen_node,
            bind: forward.bind,
            target_node: forward.target_node,
            target_addr: forward.target_addr,
            allow_from: forward.allow_from,
            enabled,
        };
        self.show(self.send(Method::PUT, &format!("{forwards}/{id}"), &input).await?, render::forward)
    }

    async fn pair_command(&self, command: PairCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        let pairs = format!("/networks/{network}/pairs");
        match command {
            PairCommand::List => self.show_list(self.get(&pairs).await?, render::pairs),
            PairCommand::Set { a, b, keep_alive } => {
                let (a, b) = (self.node(network, &a).await?.id, self.node(network, &b).await?.id);
                let input = PairInput { keep_alive: keep_alive.0 };
                self.show(self.send(Method::PUT, &format!("{pairs}/{a}/{b}"), &input).await?, render::pair)
            }
            PairCommand::Delete { a, b } => {
                let (a, b) = (self.node(network, &a).await?.id, self.node(network, &b).await?.id);
                self.delete(&format!("{pairs}/{a}/{b}")).await?;
                self.done(&format!("已去掉节点 #{a} 和 #{b} 的节点对设置"));
                Ok(())
            }
        }
    }

    async fn access_command(&self, command: AccessCommand) -> anyhow::Result<()> {
        let network = self.network().await?.id;
        match command {
            AccessCommand::Graph => self.show(self.get(&format!("/networks/{network}/access/graph")).await?, render::access_graph),
            AccessCommand::Check { from, to } => {
                let (from, to) = (self.node(network, &from).await?.id, self.node(network, &to).await?.id);
                self.show(self.get(&format!("/networks/{network}/access/check?from={from}&to={to}")).await?, render::access_check)
            }
        }
    }
}

impl Session {
    async fn update_command(&self, command: UpdateCommand) -> anyhow::Result<()> {
        let timing = |drain: bool| if drain { UpdateTiming::AfterDrain } else { UpdateTiming::Now };
        match command {
            UpdateCommand::Show => self.show(self.get("/update").await?, render::update_overview),
            UpdateCommand::Set { auto, source } => {
                let source = source.map(|source| (source != "default").then_some(source));
                let request = UpdateSettings { auto_update: auto.map(|switch| switch.0), source };
                self.show(self.send(Method::PATCH, "/update", &request).await?, render::update_overview)
            }
            UpdateCommand::Check => {
                self.show(self.send(Method::POST, "/update/check", &serde_json::json!({})).await?, render::update_overview)
            }
            UpdateCommand::Nodes => {
                let network = self.network().await?.id;
                self.show_list(self.get(&format!("/networks/{network}/nodes")).await?, render::node_updates)
            }
            UpdateCommand::Node { node, drain } => {
                let network = self.network().await?.id;
                let node = self.node(network, &node).await?;
                let request = ForceUpdate { timing: timing(drain) };
                let path = format!("/networks/{network}/nodes/{}/update", node.id);
                self.show(self.send(Method::POST, &path, &request).await?, |update| render::node_update(&node.name, update))
            }
            UpdateCommand::All { drain } => {
                let request = ForceUpdate { timing: timing(drain) };
                self.show(self.send(Method::POST, "/update/force", &request).await?, render::forced_updates)
            }
            UpdateCommand::Resume { version } => {
                let version = version.parse().map_err(|error| anyhow!("{error}"))?;
                self.show(self.send(Method::POST, "/update/resume", &ResumeUpdate { version }).await?, render::update_overview)
            }
        }
    }
}

/// 按名字或编号在网络列表里找一个。
fn pick_network(networks: Vec<Network>, reference: &str) -> anyhow::Result<Network> {
    let by_id = reference.parse::<NetworkId>().ok();
    let found = networks.into_iter().find(|network| network.name.eq_ignore_ascii_case(reference) || Some(network.id) == by_id);
    found.ok_or_else(|| anyhow!("没有网络 {reference:?}（可以写名字或编号）"))
}
