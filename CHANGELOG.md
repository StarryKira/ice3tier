# Changelog

The node, the controller, the command line and the web UI share one version number.

## 0.1.30 (2026-11-20)

- Fixed on macOS: a node closed the connections it relays with a reset even when they ended normally. The last part of a transfer could be cut off with "connection reset by peer", or the transfer could hang near its end. Upgrade every node that runs on macOS.
- Fixed: a default 24-hour `i3tctl stats` query (`GET /networks/{n}/stats`) sometimes returned hourly points instead of five-minute points.
- Upgrade nodes and the controller.

## 0.1.29 (2026-11-20)

- Linux: the virtual adapter's two firewall rules are added with whichever of `nft` and `iptables` the machine has. Nothing has to be installed.
  - Without `nft`, the anti-spoofing rule is an `iptables` rule in the `raw` table.
  - Without `iptables`, the accept rule is not needed and is reported as `not_needed` (admin API: a new value of `vnet.firewall.accept.state`).
- Fixed on Windows: access refused by the other node over ice3proxy was reported as a broken connection instead of "not allowed".
- Fixed on Windows: a closed port was reported as unreachable or timed out instead of refused, on the exit's own node and in the controller's relay reachability check.
- Fixed on macOS: the TCP segment size limit (`tcp_mss`) had no effect. It now limits what the node sends; what a peer that is not ice3tier sends cannot be limited on macOS.
- Fixed on macOS: a local program using the node's proxy could hang after its connection was reset.
- Web UI: the overview access map and the route star view can be clicked again.
- The tests now pass on Windows and macOS as well as on Linux.
- Upgrade nodes, the controller and the command line.

## 0.1.28 (2026-11-20)

- The node pair list shows every pair of nodes in the network. Pairs nobody configured show as default, and any pair can be edited directly (web UI, `i3tctl pair list`).
- Web UI: keep-alive and congestion control are edited inline on the node pairs page, with reset to default, search by node name and a filter for configured pairs.
- Admin API: `GET /networks/{network}/pairs` returns every pair; unconfigured ones have `stored: false`.
- Upgrade the controller and the command line. Nodes are unchanged.

## 0.1.27 (2026-11-20)

- The virtual adapter's MTU is a local setting: `i3t set mtu <576-1500>`, applied after a node restart, default 1280. Two nodes may use different values.
- `i3t network status`, the admin API and the web UI's node detail show the MTU in use.
- Fixed: `i3t set` wrote a setting as an inline table into older configuration files; it now writes a section.
- Fixed: `i3t network status` no longer says root is needed when `iptables` or `nft` is simply not installed.
- Upgrade nodes and the command line.

## 0.1.26 (2026-11-20)

**The virtual adapter is on by default; ice3proxy becomes an option of the proxy.** Upgrade the controller, nodes, command line and web UI together.

- Virtual adapter: every node gets one by default. Local programs reach other nodes by their virtual IP (`ping`, TCP, UDP; IPv4 only).
  - Linux needs `CAP_NET_ADMIN` (already in the systemd unit). Windows needs administrator rights and `wintun.dll` next to `i3t.exe`. macOS needs root. When the adapter cannot be installed, proxies and port forwards keep working.
  - Firewall: two rules on Linux (accept what arrives on the adapter, drop packets that fake a network address) and one accept rule on Windows. `i3t set firewall off` adds none, `i3t set network off` installs no adapter, `i3t network status` shows the state.
  - Limits: not yet verified on real Windows and macOS machines; with firewalld the adapter has to be put into the trusted zone.
- Removed transparent access (`i3t set intercept`, `i3t intercept status`) and `i3t connect`: connect to the virtual IP instead. In the admin API's node detail `intercept` is replaced by `vnet`.
- ice3proxy moved from the node pair to the proxy: `i3tctl proxy add … --ice3proxy`, `i3tctl proxy set <id> --ice3proxy on|off`, a switch on the proxy in the web UI. TCP streams of a proxy with the option use ice3proxy; other proxies and port forwards do not.
  - **When upgrading**: proxies on a pair stored as `always` get the option; all others do not, so turn it on for the proxies that should use it.
  - Admin API: proxies gain `lane`, `lane_effect` and `lanes`; node pairs lose them.
- Node pairs keep two settings: keep-alive and congestion control.

## 0.1.25 (2026-11-19)

- A direct path no longer flips between direct and relay because of light packet loss.
- TCP-only nodes no longer switch back and forth between two relays with similar latency.
- Upgrade nodes.

## 0.1.24 (2026-11-19)

- Active tests gain a `host` item: system, CPU, load and memory of both ends, and the state of the connection between them. `i3tctl test` includes it; `--only host` runs it alone. Host data is available on Linux only.
- Upgrade nodes and the controller.

## 0.1.23 (2026-11-19)

- Machines without IPv6 no longer log a send-failure warning every minute.
- While two nodes are connected directly, TCP streams use the shared connection instead of ice3proxy.
- Upgrade nodes.

## 0.1.22 (2026-11-19)

- Admin API: each cell of the route matrix (`GET /networks/{network}/routes`) gains `lanes`, the ice3proxy connection counts of that pair.

## 0.1.21 (2026-11-19)

- `i3t tune` no longer turns on `net.ipv4.tcp_mtu_probing`: it slows down or stalls TCP on lossy lines. If `i3t tune --apply` of 0.1.13 to 0.1.20 turned it on, run `sudo i3t tune --apply` again to turn it off.

## 0.1.20 (2026-11-19)

- A relay-to-relay link that fell back to TCP returns to UDP by itself once UDP works again. Both relays need the upgrade.
- Relay-to-relay links fall back to TCP faster when UDP stops.
- Active tests: a downlink measurement refused by the peer is reported as refused, not as interrupted.

## 0.1.19 (2026-11-18)

**ice3proxy is used by default between node pairs without a UDP path.** Upgrade the controller, nodes and command line together.

- For a pair without settings, new TCP streams use ice3proxy when the path has no UDP. Stored pair settings are unchanged. (Since 0.1.26 ice3proxy is an option of the proxy.)
- New: `i3tctl pair show <a> <b>` and `GET /networks/{n}/pairs/{a}/{b}`.
- Node list and node detail gain `no_udp`: the controller found that the node cannot use UDP.
- Fixed: streams broke when two nodes connected to each other at the same moment.
- Fixed: a downlink measurement of an active test was sometimes refused as busy.
- Fixed: the reason of a route switch was sometimes recorded wrongly.
- Fixed: the controller sometimes failed to start with node port 0.
- `i3tctl` prints node names instead of numbers. `--json` output is unchanged.

## 0.1.18 (2026-11-18)

- Fixed (since 0.1.14): between two idle nodes that can never connect directly, the route switched twice a minute.

## 0.1.17 (2026-11-18)

- Latency-first routing: the latency a node reports for a TCP leg is now the average of the last 4 minutes. Both ends need the upgrade.

## 0.1.16 (2026-11-18)

- Latency-first routing estimates TCP legs more accurately.
- Fixed: `i3t run` exited silently when it could not install its Ctrl-C handler.
- Fixed: `i3t tune --apply` rewrote its record file when it could not read it; it now reports the error.
- A node running as the `i3t` user needs a polkit rule to register names with systemd-resolved: `deploy/polkit/50-i3t-resolved.rules`.
- Building needs Rust 1.91 or newer.

## 0.1.15 (2026-11-18)

**Breaking: proxies were redesigned.** Upgrade the web UI, command line, controller and every node that runs a proxy together. The database migrates automatically.

- A proxy listens on node A, leaves through exit node B, and has its own username and password (`i3tctl proxy list|show|add|set|delete`, API `/networks/{n}/proxies`). One port accepts both SOCKS5 and HTTP.
- A proxy's status says why it cannot be used: exit not enabled, exit offline, or not allowed by the access rules.
- Removed: global access credentials, `i3tctl listener`, `i3tctl credential`, choosing the exit by proxy username.
- When upgrading:
  - Every listener becomes a proxy with the same number. One without a fixed exit is disabled until an exit is chosen.
  - A listener that required credentials gets a newly generated username and password; clients have to switch to them.
  - Nodes no longer listen on 127.0.0.1:1080 and :1081 by default.
  - The metrics `i3t_credential_*` are replaced by `i3t_proxy_bytes_total` and `i3t_proxy_connections`.
- Note: proxy passwords are stored as they are in the controller's database and returned by the admin API.

## 0.1.14 (2026-11-18)

- Two relays connect over TCP when UDP between them does not work. Both relays need the upgrade.
- Two nodes with no direct path and no relay in common are routed through several relays automatically.
- A node with at most 32 peers tries the direct path to each of them right after it starts.

## 0.1.13 (2026-11-17)

- Fixed: unstable connections on lines that drop full-size packets. The program's TCP connections now use segments of 1360 bytes; configurable with `[performance] tcp_mss` on a node and `--tcp-mss` on the controller, 0 for the system default.
- The first requests after a node restart no longer wait an extra second or two now and then.
- `i3t ping`: a round trip that never returns counts as lost.
- "TCP lanes" are now called ice3proxy; option and field names stay `lane`.

## 0.1.12 (2026-11-17)

The database migrates automatically.

- Automatic updates can be turned off per node: `i3tctl node set <node> --auto-update off`.
- A release manifest can carry a security floor (`i3t-release manifest --security-floor <version>`). Nodes below it are marked as needing a security update; nothing is forced.

## 0.1.11 (2026-11-17)

**Breaking: nodes no longer declare themselves as port-forward targets.** Upgrade the web UI, command line and controller together. The database migrates automatically.

- QoS (formerly service class): UDP data is not dropped by default. Only "low latency" drops stale UDP data when the link cannot keep up.
- The administrator turns a node into a port-forward target: `i3tctl node set <node> --forward-target on`. Note: whoever runs the controller can reach anything the target machine can reach through a port forward.
- Relays: nodes report which relays they can reach (`i3tctl relay list`); when two nodes have no relay in common the controller assigns one both can reach; a relay shows who is using it.
- The route table shows the state of the direct path and the streams and traffic per peer. `i3tctl path` shows estimated and measured latency and recent route switches.
- Congestion control can be set per node pair: `i3tctl pair set <a> <b> --congestion bbr|cubic|default`.
- A node's country flag follows its egress address.

## 0.1.10 (2026-11-17)

**Breaking: relay addresses and regions and the exit bandwidth limit are gone.** Upgrade the web UI, command line and controller together. The database migrates automatically.

- Relay addresses are found automatically; `i3tctl relay enable <node>` is enough. A server behind a port mapping states the address others use with `i3t set relay-address <address:port>`.
- Whoever can reach a relay uses it. A relay need not be reachable by everyone, nor have a public address.
- Exits have no bandwidth limit any more. Exits still on the old default ports 80 and 443 become unrestricted.
- A warning appears when a node's own network overlaps the virtual network's address range.
- Time series of online nodes, traffic and streams: `GET /networks/{n}/stats`, `i3tctl stats`.

## 0.1.9 (2026-11-17)

The database migrates automatically.

- Exits allow any destination port by default. Restrict them with a port list: `i3tctl exit enable <node> --ports 80,443`.
- The HTTP entry also forwards plain `http://` requests, not only CONNECT.
- Port forwards: an unreachable target is shown with its reason; TCP forwards can send a PROXY protocol header to the target (`--proxy-protocol v1|v2`).
- New commands: `i3tctl exit test <node> <target>`, `i3t ping <node>`.
- Controller backup: `i3t-controller --data-dir <dir> --backup-to <new dir>`, also while the controller is running.
- The controller's database file is readable by its owner only.
- The flow log records the route at the start and at the end of each stream.

## 0.1.8 (2026-11-17)

- Released programs are smaller and use less memory when idle.
- Transparent access: `i3t hosts --remove`; it is taken down automatically while the address range overlaps a real network.
- Admin API: group references carry `builtin`.
- The release tool `i3t-release` speaks Simplified Chinese and English.

## 0.1.7 (2026-11-17)

- Transparent access (Linux, off by default): after `i3t set intercept on`, ordinary programs connect to another node's virtual IP or name directly. (Replaced by the virtual adapter in 0.1.26.)

## 0.1.6 (2026-11-17)

- Performance profiles: low footprint, balanced (default) and high throughput. Choose locally with `i3t set profile …` or per node with `i3tctl node set … --profile`.
- Faster recovery after a network outage; connections through a TCP relay survive a 20-second outage.
- Fixed: an update withdrawn during its self-check no longer leaves the unverified program behind.

## 0.1.5 (2026-11-16)

**Breaking: groups replace tags.** Upgrade the web UI, command line and controller together. The database migrates automatically.

- Groups: a node can be in several groups; every network has the built-in group `All`; each former tag becomes a group of the same name. Commands: `i3tctl group …`, `i3tctl node groups`, `i3tctl token create --group`.
- The command line speaks Simplified Chinese and English, following the system language; `I3T_LANG=zh-CN|en` overrides it.
- Errors and audit log entries in the admin API carry codes and named values, so a client can render them in any language.
- Nodes gain a country or region (`country`).

## 0.1.4 (2026-11-16)

- Active tests: `i3tctl test A B [--throughput] [--via r1,r2]`, and the whole-network matrix `i3tctl tests matrix`. A node can refuse throughput tests: `i3t set throughput-tests off`.
- Web UI: Simplified Chinese and English; a new shell.

## 0.1.3 (2026-11-16)

- TCP lanes (later renamed ice3proxy): can be turned on per node pair, one TCP connection per stream: `i3tctl pair set A B --lane always`.
- The web UI was redesigned.
- An exit handles up to 16384 concurrent streams per peer.
- After a node crashes and restarts, its peers reconnect at once.

## 0.1.2 (2026-11-16)

Security hardening. The protocol between nodes changed incompatibly; upgrade nodes and the controller together.

- Limits on the data a peer can queue, on connections per peer and on concurrent streams; streams from a peer that no access rule allows are reset at once.
- An exit does not connect to its own node's or other nodes' public addresses on behalf of others.
- When a node loses a capability or the access rules tighten, the streams that depended on it are closed at once.
- Relay error frames cannot be forged; replayed probes can no longer keep a direct path down; idle connections can no longer lock nodes out of the controller.
- A node falsely reporting a failed update can no longer pause updates for every network.
- Streams have no idle timeout; TCP keep-alive is on at both ends.
- Routing decisions can be inspected: `i3tctl path`, `i3tctl backbone`.
- New commands: `i3t connect`, `i3t tune`.

## 0.1.1 (2026-11-16)

- Routing: latency-first mode; paths through several relays.
- Direct paths: LAN discovery, router port mapping (UPnP, NAT-PMP, PCP), IPv6.
- BBR is the default congestion control.
- UDP: SOCKS5 UDP ASSOCIATE and UDP port forwards; service classes.
- Exits: traffic quota, outbound address family and other policies.
- Automatic node updates: signature verification, distribution through the controller, automatic rollback on failure.
- Web UI: topology graph, route matrix, node update page.
- `/metrics` on the node.

## 0.1.0 (2026-11-15)

The first usable version.

- One end-to-end QUIC connection between two nodes, one stream per user connection. Packets travel directly or through a relay; connections and streams survive a route switch.
- Relays only forward packets. Nodes that cannot use UDP reach a relay over TCP.
- Direct paths: candidate addresses are verified before use; nodes behind NAT punch holes.
- Entry: SOCKS5 and HTTP CONNECT; TCP port forwards.
- Exits: access rules and outbound policy; names are resolved at the exit.
- Controller: enrollment tokens, fixed virtual IPs, access rules, admin API; command line `i3tctl`.
- Node command line `i3t`: join, run, local declarations.
