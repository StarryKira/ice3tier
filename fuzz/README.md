# fuzz

读外人能控制的字节的代码的 fuzz 目标，用 [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz)（libFuzzer）。

这个目录自成一个 workspace，不是根目录 workspace 的成员：平常的 `cargo build`、`cargo test` 和 CI 都不碰它。

## 怎么跑

要 nightly 工具链（sanitizer 只在 nightly 上有）和 cargo-fuzz：

```sh
cargo install cargo-fuzz --locked
cd fuzz
cargo fuzz list
cargo fuzz run datagram corpus/datagram seeds/datagram -- -max_total_time=300
cargo fuzz run access_http corpus/access_http seeds/access_http -- -dict=dicts/http.dict -max_total_time=300
cargo fuzz run control_json corpus/control_json seeds/control_json -- -dict=dicts/control.dict -max_total_time=300
```

- 第一个目录是工作语料，libFuzzer 把新找到的输入写进去（`corpus/` 不进版本库）；`seeds/` 是手写的起点，只读。
- 崩溃的输入写在 `artifacts/<目标>/`。复现：`cargo fuzz run <目标> artifacts/<目标>/crash-…`；缩小：`cargo fuzz tmin <目标> <文件>`。
- 构建目录可以用 `--target-dir` 指到别处，`fuzz/target/` 不进版本库。
- `access_*` 和 `relay_udp` 在回环上开端口，同一台机器上可以同时跑几个（端口都是系统挑的）。

## 目标

| 目标 | 读的是什么 | 除了不 panic 还查什么 |
|---|---|---|
| `datagram` | 节点端口上的一个 UDP 数据报（也是 TCP 中转里的一帧）：`Datagram::parse` 分出的中转头、多级中转头、中转会话帧、探测帧、地址查询、骨干矩阵的行；relay 只看头的 `forwarding_header_len`、`relayed_dst`；ERROR 的回显；局域网信标 | 中转帧的头重新编码、接上包就是原来的数据报；两个只看头的函数和完整解析一致；每种帧编码之后再解析不变 |
| `flow_open` | 流上的第一样东西：OPEN（流、数据报上下文、测量流）、REPLY、目标 | 照 `Decoded::Incomplete` 给的字节数一点一点读，要的不超过头的上限、头读完时一个数据字节都没多读、结果和一次读完一样；解析出来的值重新编码就是原来的字节 |
| `flow_datagram` | 数据报上下文的一帧（分片头）、测量流的摘要；发送一侧的分片 | 头重新编码就是原来的帧；切出来的每片不超过上限，拼回来是原来的内容 |
| `tcp_frame` | TCP 中转上读到的字节流 | 分块交给解码器和一次交进去切出的帧一样，帧加上长度前缀就是读走的字节 |
| `relay_udp` | relay 收到的 UDP（`Relay::handle_udp`）：握手、保活、一级和多级中转、一批里发往不同节点的数据报，来源是有会话的节点、骨干上的 relay 或陌生地址 | 不 panic。环境里两个节点和另一个 relay 已经握好手（`src/relay.rs`），所以转发的路径走得到 |
| `names_dns` | 透明访问的名字服务收到的 DNS 报文（`Names::answer`） | 有回答时编号照抄、标着是应答、不比请求长出一条 A 记录以上 |
| `control_json` | 节点和 controller 之间的 JSON 消息（`ToController`、`ToNode`，含经连接带的打洞信令），注册令牌的文本 | 读得出来的写回 JSON 再读，内容不变 |
| `access_socks5` | SOCKS5 入口：问候、用户名密码认证（用户名是出口和服务等级的写法）、CONNECT / UDP ASSOCIATE，之后的选路和首批数据；UDP 关联里每个报文的头 | 入口在客户端关掉写的一半之后 5 秒之内关连接 |
| `access_http` | HTTP 代理入口：CONNECT 和普通 `http://` 请求的转发、`Proxy-Authorization: Basic`、请求体的分帧（`Content-Length`、`chunked`、尾部） | 同上 |
| `access_forward` | TCP 端口转发的首批数据：TLS ClientHello 那条记录要读完整，别的有多少读多少 | 同上 |

`access_*` 三个目标从入口的监听器进去：入口里这些解析器都是 crate 私有的。`src/access.rs` 在回环上起一个入口，
对端节点是内存连接层上的替身，就地出站和对端都连到一个回环上的"目标"；每个输入是一条 TCP 连接上客户端发的全部字节
（第二个字节可以把它切成两次发）。所以它们一秒只跑几百个输入，比别的目标慢两三个数量级。

## 种子和字典

`seeds/<目标>/` 是用各个 crate 自己的编码函数造的合法输入，每个文件名说明它是什么。没有它们，libFuzzer 很难猜对开头的
类型字节、长度和节点编号，走不进后面的解析。`dicts/` 是 HTTP 的方法和请求头、控制消息的字段名。

## 发现

`regressions/<目标>/` 里是 fuzz 找到的、缩小过的输入，对应的问题都已经改掉，留着是为了以后不再犯：
改动解析的代码之后把它们重放一遍（`cargo fuzz run <目标> regressions/<目标>/*`）。

| 输入 | 是什么 |
|---|---|
| `regressions/datagram/addr-reply-v4-mapped-v6` | 地址查询的应答里地址族写 6、地址是 IPv4 映射的 IPv6（`::ffff:a.b.c.d`）。解析原样给出 IPv6 的地址，编码却把它写成 IPv4，同一个地址有两种写法。中转会话的 OK、PONG 和探测的 PONG 也是这样。现在解析时就换成 IPv4 |
| `regressions/flow_datagram/split-empty-body-tiny-max-len` | `Frames::split` 对空的内容、比头还小的 `max_len`（这里是 1）照样给出一个 5 字节的帧，没有返回 `FrameTooSmall`。现在返回了 |
