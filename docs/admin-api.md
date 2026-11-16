# ice3tier 管理 API

这份文档写给做 WebUI 和写脚本的人：controller 的管理接口有哪些，每个接口的方法、路径、请求和应答的 JSON，以及出错时的格式。命令行 `i3tctl` 用的就是这套接口。

对应的 Rust 类型在 `crates/i3t-model/src/admin.rs`，实现在 `crates/i3t-controller/src/admin.rs`。文档和代码不一致时以代码为准，并请修正文档。

## 1. 约定

### 1.1 地址与认证

- controller 的管理端口默认是 `127.0.0.1:20332`，明文 HTTP。所有接口的路径都以 `/api/v1` 开头，下文的路径省略这个前缀。
- 每个请求都要带管理令牌：

  ```
  Authorization: Bearer <管理令牌>
  ```

  管理令牌在 controller 第一次启动时打印一次，controller 只存它的哈希。只有一个管理员，没有角色。令牌丢了要停掉 controller，用 `i3t-controller --reset-admin-token` 重新生成。
- 没带令牌或令牌不对时，应答是 `401`，错误码 `unauthorized`，并带 `WWW-Authenticate: Bearer` 头。
- 配置了静态目录（`i3t-controller --static-dir`）时，`/api/v1` 之外的路径从那个目录取文件，不是文件的路径回 `index.html`（给单页应用的前端路由用）。静态文件不需要令牌。WebUI 和 API 同源，所以 controller 不发 CORS 头；开发时请让开发服务器把 `/api` 代理到 controller。
- 没有推送接口。界面需要实时数据时轮询（节点每 10 秒上报一次，轮询得比这更勤没有意义）。

### 1.2 请求和应答

- 请求体和应答体都是 JSON（`Content-Type: application/json`），请求体最大 2 MB。`POST`、`PUT`、`PATCH` 都要带 JSON 请求体，没有要写的字段时写 `{}`。
- 成功的状态码：`200`；创建成功是 `201`；删除成功是 `204`，没有应答体。
- 应答里的字段总是全的：没有值的字段写 `null`，空的列表写 `[]`，不会省略。
- 请求里标了"可省略"的字段可以不写。controller 忽略不认识的字段。
- 改东西的接口有两种：
  - `PATCH`：只改请求里写了的字段。个别字段可以写 `null`，表示清掉这项设置，下面会单独说明；别的字段写 `null` 等于没写。
  - `PUT`：整个替换，请求体的字段和创建时一样。

### 1.3 数据的写法

| 东西 | 写法 | 例子 |
|---|---|---|
| 时间 | Unix 秒，整数 | `1794798564` |
| 网络编号、节点编号、规则等的编号 | 正整数。节点编号在整个 controller 里唯一，删掉之后不复用 | `3` |
| 公钥 | 43 个字符的 base64url（不带填充） | `"AQEBAQEB…AQE"` |
| IPv4 地址 | 字符串 | `"10.66.0.5"` |
| 地址加端口 | 字符串；IPv6 地址加方括号 | `"203.0.113.7:20330"`、`"[2001:db8::1]:20330"` |
| 地址段 | CIDR 字符串 | `"10.66.0.0/16"`、`"192.168.1.0/24"` |

名字和标签的规则：

- **网络和节点的名字**：字母、数字、连字符，不能以连字符开头或结尾，最长 63 个字符，不能全是数字。大写会被转成小写。节点名在一个网络里唯一，网络名在整个 controller 里唯一。
- **标签**：字母、数字、连字符、下划线，最长 32 个字符，转成小写。`all` 是内置标签，每个节点都有；给节点打标签时写了 `all` 会被去掉，规则里可以用它。
- **规则、凭据、监听器、端口转发的名字**：给人看的一小段文字，最长 64 个字符，不能有控制字符，首尾的空白会被去掉。凭据和监听器的名字不能是空的。

几个到处用到的枚举：

| 字段 | 取值 | 含义 |
|---|---|---|
| `routing` | `"direct_first"` | 直连可用就走直连，否则只经一个 relay |
| | `"lowest_cost"` | 直连和各条 relay 路径按实测代价比较，取最低的 |
| `reach` | `"any"` | 不限制 |
| | `"direct_only"` | 不使用 relay |
| | `"relay_only"` | 不建立直连，不向对端暴露公网地址 |
| `direction` | `"both"` | `src <-> dst`，两边可以互相访问 |
| | `"one_way"` | `src -> dst`，只有 `src` 一侧能发起 |
| `protocol`（监听器） | `"socks5"`、`"http"` | |
| `protocol`（端口转发） | `"tcp"`、`"udp"` | |

`NodeRef` 是指代一个节点时附带的最少信息，很多应答里用到：

```json
{ "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" }
```

### 1.4 网络是硬隔离

除了 `/status`、`/log` 和 `/networks`，所有接口都在某一个网络下面（`/networks/{network}/…`，`{network}` 是网络编号）。路径里指的节点、规则、凭据、监听器、端口转发、令牌必须属于这个网络，否则是 `404 not_found`，和不存在一样。请求体里引用的节点（固定出口、端口转发的两端、继任的对象等）也必须在同一个网络里。

## 2. 错误

出错时的应答体都是这个形状：

```json
{
  "error": {
    "code": "not_declared",
    "message": "节点 laptop 没有声明可以作为出口。在节点上执行 `i3t set can-exit on` 之后才能启用"
  }
}
```

`code` 给程序看，`message` 给人看，可以直接显示在界面上。

| HTTP 状态 | `code` | 什么时候 |
|---|---|---|
| 401 | `unauthorized` | 没带管理令牌，或者令牌不对 |
| 404 | `not_found` | 路径或请求体里指的东西不存在，或者不属于路径里的那个网络；路径不是一个接口 |
| 400 | `invalid_request` | 请求本身不合法：JSON 解析不了、缺字段、路径里的编号不是数字、名字或标签的写法不对、地址不合法、数值超出范围 |
| 405 | `invalid_request` | 这个路径不支持这种请求方法 |
| 400 | `address_out_of_range` | 虚拟 IP 不在网络的地址段里，或者是网络地址、广播地址 |
| 409 | `name_taken` | 名字已经被占用（网络名；同一网络里的节点名、凭据名；同一节点上的监听器名） |
| 409 | `name_cooling_down` | 名字属于一个刚删除的节点，还在冷却期（90 天） |
| 409 | `address_taken` | 虚拟 IP 已经分给了另一个节点 |
| 409 | `address_cooling_down` | 虚拟 IP 属于一个刚删除的节点，还在冷却期（90 天） |
| 409 | `range_overlaps` | 地址段和另一个网络的重叠 |
| 409 | `range_exhausted` | 地址段里没有空闲地址了 |
| 409 | `range_not_superset` | 网络里已经有节点，新的地址段必须包含原来的 |
| 409 | `network_not_empty` | 网络里还有节点，不能删除 |
| 409 | `not_declared` | 节点没有在本地声明可以承担这项能力（relay、出口、端口转发的目标）。`message` 里写着要在节点上执行的命令 |
| 409 | `predecessor_online` | 要被继任的节点还在线 |
| 409 | `conflict` | 和已有的配置冲突：同一个监听地址上已经有监听器或同协议的转发；虚拟 IP 已经预设在另一个令牌上 |
| 500 | `internal` | controller 内部出错。细节在 controller 的日志里，不在应答里 |

以后可能增加新的 `code`，客户端遇到不认识的值请按 HTTP 状态处理，并显示 `message`。

## 3. controller

### GET /status

controller 自己的情况。也用来检查地址和令牌对不对。

```json
{
  "version": "0.1.0",
  "controller_key": "LltlBKa6eUXymVVonwneDvQzlnt_0z1jqdmwdPsruNY",
  "public_addr": "ctl.example.com:20331",
  "networks": 1,
  "nodes": 3,
  "online": 2
}
```

| 字段 | 含义 |
|---|---|
| `version` | controller 程序的版本号 |
| `controller_key` | controller 的公钥，写在每个注册令牌里 |
| `public_addr` | 节点连 controller 用的 `主机:端口`，写在每个注册令牌里 |
| `networks`、`nodes`、`online` | 全部网络的数量、全部节点的数量、在线节点的数量 |

### GET /log

操作记录，新的在前。修改类的操作和 controller 自动做的改动（例如节点撤回声明导致能力关闭）都记在这里。

查询参数：`limit`，可省略，最多返回多少条，默认 100，上限 1000。

```json
[
  { "id": 19, "at": 1794798564, "network": 1, "action": "pair.set", "detail": "节点 #2 和 #1：连接常驻 true" },
  { "id": 18, "at": 1794798564, "network": 1, "action": "forward.create", "detail": "转发 #1：节点 #2 的 127.0.0.1:8080 → 节点 #1 的 192.168.1.10:80（启用）" }
]
```

| 字段 | 含义 |
|---|---|
| `id` | 递增的编号 |
| `at` | 时间 |
| `network` | 属于哪个网络；和网络无关的操作是 `null` |
| `action` | 做了什么，见下 |
| `detail` | 给人看的说明 |

`action` 的取值：`admin.token`、`network.create`、`network.update`、`network.delete`、`node.enroll`、`node.update`、`node.delete`、`node.succeed`、`node.relay.enable`、`node.relay.disable`、`node.relay.withdrawn`、`node.exit.enable`、`node.exit.disable`、`node.exit.withdrawn`、`node.forward.withdrawn`、`token.create`、`token.delete`、`rule.create`、`rule.update`、`rule.delete`、`credential.create`、`credential.delete`、`listener.create`、`listener.update`、`listener.delete`、`forward.create`、`forward.update`、`forward.delete`、`pair.set`、`pair.delete`。带 `withdrawn` 的三种是节点撤回了本地声明、controller 随之关闭能力时记的。最多保留最近的一万条。

## 4. 网络

`Network`：

```json
{
  "id": 1,
  "name": "home",
  "domain": "home.i3t",
  "cidr": "10.66.0.0/16",
  "routing": "direct_first",
  "max_relay_hops": 2,
  "reach": "any",
  "created_at": 1794798564,
  "nodes": 3
}
```

| 字段 | 含义 |
|---|---|
| `id` | 网络编号 |
| `name` | 名字，建好之后不能改 |
| `domain` | 节点全名的后缀，固定是 `<name>.i3t` |
| `cidr` | 虚拟地址段 |
| `routing` | 默认的选路模式 |
| `max_relay_hops` | 一条路径最多经过几个 relay，1 到 4 |
| `reach` | 对可用路径的限制 |
| `created_at` | 创建时间 |
| `nodes` | 网络里有多少个节点 |

### GET /networks

全部网络，按编号排。应答是 `Network` 的数组。

### POST /networks

新建一个网络。它自带一条名叫 `default` 的 `all <-> all` 规则，所以装好就能互通。

```json
{ "name": "home", "cidr": "10.66.0.0/16", "routing": "direct_first", "max_relay_hops": 2, "reach": "any" }
```

| 字段 | 必填 | 含义 |
|---|---|---|
| `name` | 是 | 名字 |
| `cidr` | 否 | 地址段，前缀最长 `/30`，会被规整成网络地址的形式。省略时 controller 依次试 `10.66.0.0/16`、`10.67.0.0/16`……取第一个不和别的网络重叠的 |
| `routing` | 否 | 默认 `"direct_first"` |
| `max_relay_hops` | 否 | 默认 2 |
| `reach` | 否 | 默认 `"any"` |

应答：`201`，`Network`。
错误：`name_taken`、`range_overlaps`、`invalid_request`。

### GET /networks/{network}

应答：`Network`。

### PATCH /networks/{network}

改网络的设置，新的设置随即下发给网络里的节点。

```json
{ "routing": "lowest_cost" }
```

可以写的字段：`cidr`、`routing`、`max_relay_hops`、`reach`，都可省略。网络里已经有节点时，新的 `cidr` 必须包含原来的（地址段只能扩大）。

应答：`Network`。
错误：`range_overlaps`、`range_not_superset`、`invalid_request`。

### DELETE /networks/{network}

删掉一个空的网络，连同它的规则、凭据和注册令牌。

应答：`204`。
错误：`network_not_empty`。

### GET /networks/{network}/overview

一个网络的总览。

```json
{
  "network": 1,
  "nodes": 3,
  "online": 3,
  "relays": 1,
  "exits": 1,
  "rules": 1,
  "listeners": 0,
  "forwards": 0,
  "credentials": 0,
  "pending_tokens": 0,
  "online_nodes": [
    { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" },
    { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
    { "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3" }
  ],
  "unreachable_pairs": [
    {
      "a": { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
      "b": { "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3" }
    }
  ]
}
```

| 字段 | 含义 |
|---|---|
| `nodes`、`online` | 节点数、在线的节点数 |
| `relays`、`exits` | 启用了 relay、启用了出口的节点数 |
| `rules`、`listeners`、`forwards`、`credentials` | 各自的数量（规则包括停用的） |
| `pending_tokens` | 还没被用掉、也没过期的注册令牌数 |
| `online_nodes` | 在线的节点，`NodeRef` 的数组 |
| `unreachable_pairs` | 两端都在线、但至少一端上报"没有可用的路"的节点对：直连不通，也没有 relay 可走。`a` 的编号小于 `b` |

`unreachable_pairs` 不为空而 `relays` 是 0 时，界面应当提示启用一个 relay。

## 5. 节点

`Node`（列表里的一项）：

```json
{
  "id": 1,
  "network": 1,
  "key": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE",
  "name": "node-1",
  "fqdn": "node-1.home.i3t",
  "virtual_ip": "10.66.0.1",
  "tags": ["servers"],
  "declared": { "exit": true, "relay": true, "forward": true },
  "relay": {
    "udp": ["203.0.113.7:20330"],
    "tcp": ["203.0.113.7:20330"],
    "region": "hk",
    "bandwidth_mbps": 200
  },
  "exit": {
    "ports": [80, 443],
    "lan": true,
    "bandwidth_mbps": null,
    "upstream_socks5": null
  },
  "dial_out_only": false,
  "routing": null,
  "reach": null,
  "pinned_relays": [],
  "platform": "linux-x86_64",
  "version": "0.1.0",
  "online": true,
  "last_seen_at": 1794798564,
  "created_at": 1794798564,
  "netmap_version": 6,
  "sent_version": 6,
  "acked_version": 0
}
```

| 字段 | 含义 |
|---|---|
| `id` | 节点编号 |
| `network` | 所属网络的编号 |
| `key` | 节点的公钥 |
| `name` | 名字。节点加入时叫 `node-<编号>`，之后可以改 |
| `fqdn` | 全名：`<name>.<网络的 domain>` |
| `virtual_ip` | 虚拟 IP。加入时分配（地址段里最小的空闲地址），之后不变，除非管理员改 |
| `tags` | 管理员打的标签，按字典序。内置的 `all` 不列出 |
| `declared` | 节点在本地声明的"可以承担"：`exit` 出口、`relay`、`forward` 端口转发的目标。由节点上报，这里只读 |
| `relay` | 启用为 relay 时是它的设置（见 5.5），否则 `null` |
| `exit` | 启用为出口时是它的设置（见 5.6），否则 `null` |
| `dial_out_only` | 只拨出：不接受别人主动发来的连接，不参与打洞 |
| `routing` | 覆盖网络默认值的选路模式；`null` 表示用网络的 |
| `reach` | 覆盖网络默认值的路径限制；`null` 表示用网络的 |
| `pinned_relays` | 管理员固定的挂靠 relay（节点编号）；空表示节点自己挑 |
| `platform`、`version` | 节点上报的平台和程序版本 |
| `online` | 现在是不是连着 controller |
| `last_seen_at` | 最近一次在线的时间；在线时就是现在；从来没有连上过是 `null` |
| `created_at` | 加入的时间 |
| `netmap_version` | 这个节点的配置（netmap）现在是第几版。内容变了才加一 |
| `sent_version` | 最近一次发给它的版本 |
| `acked_version` | 它确认已经应用的版本。小于 `netmap_version` 说明它还在用旧的配置（例如不在线） |

**能力的启用。** relay、出口和端口转发的目标都要节点先在本地声明（`declared` 里对应的字段是 `true`），管理员才能启用。没有声明时，启用的请求得到 `409 not_declared`，`message` 里写着要在节点上执行的命令（`i3t set can-relay on`、`i3t set can-exit on`、`i3t set can-forward on`）。界面上应当在 `declared` 里对应字段是 `false` 时把开关置灰并显示这个做法。节点撤回声明时，已经启用的能力随之关闭（`relay` 或 `exit` 变回 `null`，指向它的端口转发被停用），并在操作记录里留下 `node.*.withdrawn`；节点再声明回来，能力不会自己打开。

### GET /networks/{network}/nodes

网络里的全部节点，按编号排。应答是 `Node` 的数组。

### GET /networks/{network}/nodes/{node}

节点详情：`Node` 的全部字段，加上它最近上报的东西，都在同一层。

```json
{
  "id": 2,
  "network": 1,
  "key": "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI",
  "name": "laptop",
  "fqdn": "laptop.home.i3t",
  "virtual_ip": "10.66.0.2",
  "tags": ["laptops"],
  "declared": { "exit": false, "relay": false, "forward": false },
  "relay": null,
  "exit": null,
  "dial_out_only": false,
  "routing": null,
  "reach": null,
  "pinned_relays": [1],
  "platform": "linux-x86_64",
  "version": "0.1.0",
  "online": true,
  "last_seen_at": 1794798564,
  "created_at": 1794798564,
  "netmap_version": 4,
  "sent_version": 4,
  "acked_version": 0,
  "endpoints": ["192.168.1.5:20330"],
  "observed_endpoint": "198.51.100.7:20330",
  "attached": [{ "relay": 1, "rtt_ms": 12 }],
  "mapping_varies": false,
  "flows": 2,
  "tx_bytes": 1024,
  "rx_bytes": 4096
}
```

| 多出来的字段 | 含义 |
|---|---|
| `endpoints` | 节点上报的 UDP 候选端点 |
| `observed_endpoint` | controller 看到的来源 IP 加上节点自报的 UDP 端口；节点没报端口时是 `null` |
| `attached` | 节点挂靠的 relay（节点编号）和到它们的往返延迟（毫秒）。延迟只在变化明显时更新 |
| `mapping_varies` | 映射出来的端口是不是随对端变化。`true` 时这台机器所在的网络基本打不了洞，界面应当提示。不在线或者还不知道时是 `null` |
| `flows` | 正在进行的流的数量 |
| `tx_bytes`、`rx_bytes` | 节点启动以来发出和收到的字节数 |

`mapping_varies`、`flows`、`tx_bytes`、`rx_bytes` 是节点这次连接期间最近一次上报的值；节点不在线时是 `null` 和 0。

### PATCH /networks/{network}/nodes/{node}

改节点的名字、标签、虚拟 IP 和各个开关。

```json
{ "name": "laptop", "tags": ["laptops"], "pinned_relays": [1], "routing": null }
```

| 字段 | 含义 |
|---|---|
| `name` | 新名字 |
| `tags` | 整个替换节点的标签 |
| `virtual_ip` | 新的虚拟 IP。必须在网络的地址段里，不是网络地址和广播地址，没被占用，不在冷却期。改了之后引用旧地址的配置会失效，界面上要提示 |
| `dial_out_only` | `true` 或 `false` |
| `routing` | 一种选路模式；写 `null` 表示改回用网络的默认值 |
| `reach` | 一种路径限制；写 `null` 表示改回用网络的默认值 |
| `pinned_relays` | 整个替换固定的挂靠 relay，最多 8 个，每个都必须是这个网络里启用了的 relay；`[]` 表示不固定 |

全部可省略。`routing` 和 `reach` 是仅有的两个"写 `null` 有含义"的字段。

应答：`Node`。
错误：`name_taken`、`name_cooling_down`、`address_taken`、`address_cooling_down`、`address_out_of_range`、`invalid_request`、`not_found`（固定的 relay 不在这个网络里）。

### DELETE /networks/{network}/nodes/{node}

删掉节点：吊销它的公钥，立即把它从所有节点的配置里去掉；它要是在线，会收到吊销通知并被断开。它的监听器、和它有关的端口转发与节点对设置一起删掉。它的虚拟 IP 和名字进入 90 天的冷却期，期间不分给别的节点。

应答：`204`。

### 5.5 PUT /networks/{network}/nodes/{node}/relay

把节点启用为 relay，或者改它的 relay 设置。relay 必须公网可达，地址由管理员填。

```json
{ "udp": ["203.0.113.7:20330"], "tcp": ["203.0.113.7:20330"], "region": "hk", "bandwidth_mbps": 200 }
```

| 字段 | 必填 | 含义 |
|---|---|---|
| `udp` | 否 | 从公网能连上的 UDP 地址，最多 8 个。默认 `[]` |
| `tcp` | 否 | 从公网能连上的 TCP 地址，给 UDP 用不了的节点用，最多 8 个。默认 `[]` |
| `region` | 否 | 所属区域，随便起名。默认 `""` |
| `bandwidth_mbps` | 否 | 转发的带宽上限；`null` 或省略表示不限 |

`udp` 和 `tcp` 至少要有一个地址；地址不能是 `0.0.0.0`，端口不能是 0。

应答：`Node`（`relay` 字段是刚写入的设置）。
错误：`not_declared`、`invalid_request`。

### DELETE /networks/{network}/nodes/{node}/relay

停用 relay。本来就没启用时也成功。固定挂靠到它的节点不会被改，只是那一项不再下发。

应答：`Node`（`relay` 是 `null`）。

### 5.6 PUT /networks/{network}/nodes/{node}/exit

把节点启用为出口，或者改它的出口设置。

```json
{ "ports": [80, 443], "lan": true, "bandwidth_mbps": null, "upstream_socks5": null }
```

| 字段 | 必填 | 含义 |
|---|---|---|
| `ports` | 否 | 互联网目标允许的端口。**省略**时是 `[80, 443]`；写 `null` 表示不限制。局域网目标不受它限制 |
| `lan` | 否 | 允许经它访问它所在的局域网（本机网卡直连的网段）。默认 `false` |
| `bandwidth_mbps` | 否 | 带宽上限；`null` 或省略表示不限 |
| `upstream_socks5` | 否 | 把流交给节点本机的这个 SOCKS5 代理（`地址:端口`）去连目标；`null` 或省略表示自己直接连 |

请求体写 `{}` 就是用默认设置启用。

应答：`Node`（`exit` 字段是刚写入的设置）。
错误：`not_declared`、`invalid_request`。

### DELETE /networks/{network}/nodes/{node}/exit

停用出口。本来就没启用时也成功。

应答：`Node`（`exit` 是 `null`）。

### POST /networks/{network}/nodes/{node}/succeed

把 `{node}` 指定为另一个**已下线**节点的继任。用在机器重装之后：新机器先作为一个新节点加入，再用这个接口接手旧节点的身份，外部配置不用改。

```json
{ "predecessor": 2 }
```

`{node}` 继承 `predecessor` 的虚拟 IP、名字、标签（和它自己已有的标签合并），以及指向 `predecessor` 的端口转发、监听器和节点对设置；然后 `predecessor` 被删掉，它的公钥被吊销。节点编号不继承，`{node}` 还是它自己的编号。relay、出口不继承：要在新机器上重新声明，再重新启用。继承来的、以它为目标的端口转发，在它没有声明"可以作为端口转发的目标"时是停用的。

也可以在生成注册令牌时用 `successor_of` 指名继任（见第 6 节），效果相同。

应答：`Node`（继任之后的 `{node}`）。
错误：`predecessor_online`、`not_found`、`invalid_request`（节点继任它自己）。

### GET /networks/{network}/nodes/{node}/routes

节点最近上报的路由表：它到各个对端现在怎么走。

```json
{
  "node": { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
  "online": true,
  "routes": [
    {
      "peer": { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" },
      "route": { "via": "direct", "addr": "203.0.113.7:20330" },
      "rtt_ms": 12
    },
    {
      "peer": { "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3" },
      "route": { "via": "none" },
      "rtt_ms": null
    }
  ]
}
```

`route` 有三种：

| `via` | 别的字段 | 含义 |
|---|---|---|
| `"none"` | — | 没有可用的路 |
| `"direct"` | `addr` | 直连，对端的地址 |
| `"relay"` | `relays` | 经 relay，按经过的顺序列出节点编号，例如 `{ "via": "relay", "relays": [1] }` |

`rtt_ms` 是这条路上最近测到的往返延迟，没有时是 `null`。节点不在线时 `online` 是 `false`，`routes` 是空的。只有节点上报过的对端才在表里。

### GET /networks/{network}/nodes/{node}/access

访问关系图的节点视图：这个节点能访问谁、谁能访问它、各是哪些规则允许的。

```json
{
  "node": { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
  "can_reach": [
    { "node": { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" }, "rules": [1] },
    { "node": { "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3" }, "rules": [1] }
  ],
  "reachable_by": [
    { "node": { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" }, "rules": [1] },
    { "node": { "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3" }, "rules": [1] }
  ],
  "acknowledged": false
}
```

| 字段 | 含义 |
|---|---|
| `can_reach` | 它可以访问的节点，和允许这条访问的规则的编号 |
| `reachable_by` | 可以访问它的节点，和允许这条访问的规则的编号 |
| `acknowledged` | 它是否已经确认当前版本的配置。`false` 时它还在按旧的规则判定 |

两个列表都只算启用的规则，按 controller 当前的规则算出。

## 6. 注册令牌

管理员生成注册令牌，交给要加入的机器（`i3t join` 从标准输入读它）。令牌属于一个网络，只能用一次，用它加入的节点直接生效。

`EnrollTokenInfo`：

```json
{
  "id": 1,
  "network": 1,
  "created_at": 1794798564,
  "expires_at": 1794802164,
  "tags": ["servers"],
  "virtual_ip": null,
  "successor_of": null
}
```

| 字段 | 含义 |
|---|---|
| `id` | 令牌编号 |
| `network` | 所属网络 |
| `created_at`、`expires_at` | 生成和过期的时间 |
| `tags` | 用它加入的节点自动带上的标签 |
| `virtual_ip` | 预设的虚拟 IP；`null` 表示自动分配 |
| `successor_of` | 用它加入的节点继任这个节点（节点编号）；`null` 表示是一个普通的新节点 |

### GET /networks/{network}/tokens

还没被用掉、也没过期的令牌。应答是 `EnrollTokenInfo` 的数组。令牌的文本查不回来。

### POST /networks/{network}/tokens

```json
{ "expires_in_secs": 3600, "tags": ["servers"], "virtual_ip": "10.66.0.9", "successor_of": null }
```

| 字段 | 必填 | 含义 |
|---|---|---|
| `expires_in_secs` | 否 | 多少秒之后过期，默认 86400（一天） |
| `tags` | 否 | 默认 `[]` |
| `virtual_ip` | 否 | 预设的虚拟 IP，要求同"改节点的虚拟 IP"。预设着的地址不会被自动分给别的节点 |
| `successor_of` | 否 | 要继任的节点编号。那个节点在新节点加入的那一刻必须不在线。不能和 `virtual_ip` 同时写 |

应答：`201`，`EnrollTokenInfo` 的全部字段加上 `token`：

```json
{
  "id": 1,
  "network": 1,
  "created_at": 1794798564,
  "expires_at": 1794802164,
  "tags": ["servers"],
  "virtual_ip": null,
  "successor_of": null,
  "token": "i3t-eyJjIjoiY3RsLmV4YW1wbGUuY29tOjIwMzMxIiwiayI6Ikxs…"
}
```

`token` 是整段令牌文本（以 `i3t-` 开头），**只在这个应答里出现一次**，界面要让管理员当场复制。

错误：`address_taken`、`address_cooling_down`、`address_out_of_range`、`conflict`（这个虚拟 IP 已经预设在另一个令牌上）、`not_found`（要继任的节点不在这个网络里）、`invalid_request`。

### DELETE /networks/{network}/tokens/{id}

收回一个还没用掉的令牌。应答：`204`。

## 7. 访问规则

只有"允许"：一条规则说带标签 `src` 的节点可以访问带标签 `dst` 的节点。没有被任何启用的规则连通的两个节点互相不可达，也拿不到对方的地址和公钥。规则之间没有顺序和优先级。

`Rule`：

```json
{ "id": 2, "name": "ssh", "src": "laptops", "dst": "servers", "direction": "one_way", "enabled": true }
```

### GET /networks/{network}/rules

全部规则，包括停用的，按编号排。应答是 `Rule` 的数组。

### POST /networks/{network}/rules

```json
{ "name": "ssh", "src": "laptops", "dst": "servers", "direction": "one_way", "enabled": true }
```

| 字段 | 必填 | 含义 |
|---|---|---|
| `src`、`dst` | 是 | 两个标签。可以是还没有节点带着的标签，也可以是 `all` |
| `direction` | 否 | 默认 `"both"` |
| `enabled` | 否 | 默认 `true` |
| `name` | 否 | 默认 `""` |

应答：`201`，`Rule`。
错误：`invalid_request`。

### PATCH /networks/{network}/rules/{id}

可以写的字段：`name`、`src`、`dst`、`direction`、`enabled`，都可省略。启用和停用一条规则就是 `{ "enabled": true }` 和 `{ "enabled": false }`。

应答：`Rule`。

### DELETE /networks/{network}/rules/{id}

应答：`204`。

规则的变更按顺序生效：授权先让目的节点认下发起节点，目的节点确认之后发起节点才知道可以访问它；撤销相反。所以改完规则的一小段时间里（节点不在线时可能更久），有的节点还没有确认新的配置，见 8.1 的 `unacknowledged`。

## 8. 访问关系

### 8.1 GET /networks/{network}/access/graph

访问关系图的标签视图：每个标签是一个点，每条规则是一条连线。

```json
{
  "tags": [
    {
      "tag": "all",
      "nodes": [
        { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" },
        { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
        { "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3" }
      ]
    },
    { "tag": "laptops", "nodes": [{ "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" }] },
    { "tag": "servers", "nodes": [{ "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" }] }
  ],
  "rules": [
    { "id": 1, "name": "default", "src": "all", "dst": "all", "direction": "both", "enabled": false },
    { "id": 2, "name": "ssh", "src": "laptops", "dst": "servers", "direction": "one_way", "enabled": true }
  ],
  "unconnected": [{ "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3" }],
  "unacknowledged": []
}
```

| 字段 | 含义 |
|---|---|
| `tags` | 内置的 `all`、节点上的标签、规则里提到的标签，按字典序；每个标签带着它的成员。规则里提到而没有节点带着的标签，`nodes` 是空的 |
| `rules` | 全部规则。`direction` 是 `"both"` 的画双箭头，`"one_way"` 的画从 `src` 到 `dst` 的单箭头，`enabled` 是 `false` 的画虚线 |
| `unconnected` | 没有被任何启用的规则连通的节点 |
| `unacknowledged` | 还没有确认当前版本配置的节点（例如不在线）。它们还在按旧的规则判定，图上要标出来 |

### 8.2 GET /networks/{network}/access/check

访问检查：一个节点能不能访问另一个，是哪些规则允许的。

查询参数：`from`、`to`，都必填，节点编号。

```json
{
  "from": { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
  "to": { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" },
  "allowed": true,
  "rules": [
    { "id": 2, "name": "ssh", "src": "laptops", "dst": "servers", "direction": "one_way", "enabled": true }
  ]
}
```

不允许时 `allowed` 是 `false`，`rules` 是 `[]`。方向有意义：`from=1&to=2` 和 `from=2&to=1` 可以不一样。

节点视图见第 5 节的 `GET /networks/{network}/nodes/{node}/access`。

## 9. 接入凭据

凭据是 controller 签发的令牌，调用者把它填在代理的密码字段里。它属于一个网络，在这个网络里所有要求凭据的监听器上都能用。

`Credential`：

```json
{ "id": 1, "name": "phone", "max_concurrent": 8, "created_at": 1794798564 }
```

`max_concurrent` 是同时进行的连接数上限，`null` 表示不限。

### GET /networks/{network}/credentials

应答是 `Credential` 的数组。令牌查不回来。

### POST /networks/{network}/credentials

```json
{ "name": "phone", "max_concurrent": 8 }
```

`name` 必填，在网络里唯一；`max_concurrent` 可省略，不能是 0。

应答：`201`，`Credential` 的全部字段加上 `token`：

```json
{ "id": 1, "name": "phone", "max_concurrent": 8, "created_at": 1794798564, "token": "I7bRtukVyLJzSGhIbQPcl8bjl9ZjLvFsI2PTJUaKnKU" }
```

`token` **只在这个应答里出现一次**。

错误：`name_taken`、`invalid_request`。

### DELETE /networks/{network}/credentials/{id}

吊销一个凭据。应答：`204`。

## 10. 监听器

监听器是某个节点上的一个代理入口（SOCKS5 或 HTTP CONNECT）。

`Listener`：

```json
{
  "id": 1,
  "node": 2,
  "name": "lan",
  "bind": "192.168.1.5:1080",
  "protocol": "socks5",
  "allow_from": ["192.168.1.0/24"],
  "require_credential": true,
  "default_action": { "action": "exit", "node": 1 },
  "fixed_exit": null
}
```

| 字段 | 含义 |
|---|---|
| `id` | 监听器编号 |
| `node` | 它开在哪个节点上 |
| `name` | 名字，在一个节点上唯一 |
| `bind` | 监听地址，端口不能是 0 |
| `protocol` | `"socks5"` 或 `"http"` |
| `allow_from` | 来源白名单（地址段的数组）；空表示不限制 |
| `require_credential` | 要求调用者在密码字段里带凭据。`bind` 不是回环地址时必须是 `true` |
| `default_action` | 调用者没有写用户名、目标又不是组网地址时怎么办，见下。`null` 时按 `bind` 取默认：回环地址是就地出站，其他地址是拒绝 |
| `fixed_exit` | 固定出口（节点编号）：这个监听器上的流量都从这个节点出去；`null` 表示不固定 |

`default_action` 有三种：

| 写法 | 含义 |
|---|---|
| `{ "action": "local" }` | 就地出站：从监听器所在的节点自己的网络出去 |
| `{ "action": "reject" }` | 拒绝 |
| `{ "action": "exit", "node": 1 }` | 固定走这个出口节点 |

被当作出口引用的节点（`fixed_exit`、`default_action` 里的 `node`）被删掉时，这两个字段自动变回 `null`。

### GET /networks/{network}/listeners

网络里全部节点上的监听器，按编号排。查询参数 `node`（可省略，节点编号）：只列这个节点上的。应答是 `Listener` 的数组。

### POST /networks/{network}/listeners

请求体是 `Listener` 去掉 `id`：

```json
{ "node": 2, "name": "local", "bind": "127.0.0.1:1080", "protocol": "http" }
```

`node`、`name`、`bind`、`protocol` 必填；`allow_from` 默认 `[]`，`require_credential` 默认 `false`，`default_action` 和 `fixed_exit` 默认 `null`。

应答：`201`，`Listener`。
错误：`invalid_request`（非回环地址却不要求凭据、端口是 0、名字是空的）、`name_taken`（这个节点上已经有同名的监听器）、`conflict`（这个节点上已经有监听器监听在同一个地址）、`not_found`（节点或引用的出口节点不在这个网络里）。

### PUT /networks/{network}/listeners/{id}

整个替换一个监听器，请求体同创建。可以换到另一个节点上。

应答：`Listener`。错误同创建。

### DELETE /networks/{network}/listeners/{id}

应答：`204`。

## 11. 端口转发

在监听节点的一个端口上监听，把连进来的 TCP 连接或 UDP 报文转发到目标节点去连的一个地址（它的回环地址，或者它所在局域网里的地址）。

`Forward`：

```json
{
  "id": 1,
  "name": "web",
  "protocol": "tcp",
  "listen_node": 2,
  "bind": "127.0.0.1:8080",
  "target_node": 1,
  "target_addr": "192.168.1.10:80",
  "allow_from": [],
  "enabled": true,
  "status": "active"
}
```

| 字段 | 含义 |
|---|---|
| `id` | 转发编号 |
| `name` | 名字，可以是空的 |
| `protocol` | `"tcp"` 或 `"udp"` |
| `listen_node` | 监听节点 |
| `bind` | 监听节点上的监听地址，端口不能是 0。IP 是监听节点的虚拟 IP 时不占用操作系统的端口，只对组网里被允许访问它的节点可达（这时监听节点和目标节点通常是同一个） |
| `target_node` | 目标节点。必须声明过"可以作为端口转发的目标"才能启用 |
| `target_addr` | 目标节点去连的地址，不能是 `0.0.0.0`，端口不能是 0 |
| `allow_from` | 来源白名单；空表示不限制 |
| `enabled` | 开关 |
| `status` | 现在的状态，只读，见下 |

`status` 按 controller 知道的算，排在前面的优先：

| 取值 | 含义 |
|---|---|
| `"target_not_declared"` | 目标节点没有声明（或者撤回了声明）可以作为端口转发的目标。这时转发是停用的，也启用不了。界面应当提示在目标节点上执行 `i3t set can-forward on` |
| `"disabled"` | 被停用了（`enabled` 是 `false`） |
| `"access_denied"` | 访问规则不允许监听节点访问目标节点，转发没有下发。转发不绕过访问规则 |
| `"listener_offline"` | 监听节点不在线 |
| `"target_offline"` | 目标节点不在线 |
| `"active"` | 已经下发，两端都在线 |

"端口被占用""目标连不上"这两种状态要节点上报，目前节点协议里没有，所以这里没有。

### GET /networks/{network}/forwards

应答是 `Forward` 的数组，按编号排。

### POST /networks/{network}/forwards

请求体是 `Forward` 去掉 `id` 和 `status`：

```json
{
  "name": "web",
  "protocol": "tcp",
  "listen_node": 2,
  "bind": "127.0.0.1:8080",
  "target_node": 1,
  "target_addr": "192.168.1.10:80",
  "allow_from": [],
  "enabled": true
}
```

`protocol`、`listen_node`、`bind`、`target_node`、`target_addr` 必填；`name` 默认 `""`，`allow_from` 默认 `[]`，`enabled` 默认 `true`。

应答：`201`，`Forward`。
错误：`not_declared`（`enabled` 是 `true` 而目标节点没有声明）、`conflict`（监听节点上同一个地址已经有一条同协议的转发）、`invalid_request`、`not_found`（节点不在这个网络里）。

### PUT /networks/{network}/forwards/{id}

整个替换一条转发，请求体同创建。启用和停用也用它：取出这条转发，把 `enabled` 改掉再放回去。

应答：`Forward`。错误同创建。

### DELETE /networks/{network}/forwards/{id}

应答：`204`。

## 12. 节点对设置

`PairSetting`：

```json
{ "a": 1, "b": 2, "keep_alive": true }
```

`a` 和 `b` 是两个节点的编号，`a` 小于 `b`。`keep_alive` 为 `true` 时这一对节点之间的连接常驻，不因为没有流而关闭。

### GET /networks/{network}/pairs

有设置的节点对。应答是 `PairSetting` 的数组。没有列出的节点对用默认值（`keep_alive` 为 `false`）。

### PUT /networks/{network}/pairs/{a}/{b}

写下一对节点的设置，已经有就覆盖。路径里两个节点的先后顺序无所谓。

```json
{ "keep_alive": true }
```

应答：`PairSetting`。
错误：`invalid_request`（两个编号相同）、`not_found`。

### DELETE /networks/{network}/pairs/{a}/{b}

去掉一对节点的设置，回到默认。应答：`204`。这一对本来没有设置时是 `404 not_found`。

## 13. 接口一览

| 方法 | 路径（前面加 `/api/v1`） | 作用 |
|---|---|---|
| GET | `/status` | controller 的情况 |
| GET | `/log` | 操作记录 |
| GET、POST | `/networks` | 列出、新建网络 |
| GET、PATCH、DELETE | `/networks/{network}` | 查看、修改、删除网络 |
| GET | `/networks/{network}/overview` | 总览 |
| GET | `/networks/{network}/nodes` | 节点列表 |
| GET、PATCH、DELETE | `/networks/{network}/nodes/{node}` | 节点详情、修改、删除 |
| PUT、DELETE | `/networks/{network}/nodes/{node}/relay` | 启用（或修改）、停用 relay |
| PUT、DELETE | `/networks/{network}/nodes/{node}/exit` | 启用（或修改）、停用出口 |
| POST | `/networks/{network}/nodes/{node}/succeed` | 继任 |
| GET | `/networks/{network}/nodes/{node}/routes` | 节点上报的路由表 |
| GET | `/networks/{network}/nodes/{node}/access` | 访问关系的节点视图 |
| GET、POST | `/networks/{network}/tokens` | 列出、生成注册令牌 |
| DELETE | `/networks/{network}/tokens/{id}` | 收回注册令牌 |
| GET、POST | `/networks/{network}/rules` | 列出、新建规则 |
| PATCH、DELETE | `/networks/{network}/rules/{id}` | 修改、删除规则 |
| GET、POST | `/networks/{network}/credentials` | 列出、签发凭据 |
| DELETE | `/networks/{network}/credentials/{id}` | 吊销凭据 |
| GET、POST | `/networks/{network}/listeners` | 列出、新建监听器 |
| PUT、DELETE | `/networks/{network}/listeners/{id}` | 替换、删除监听器 |
| GET、POST | `/networks/{network}/forwards` | 列出、新建端口转发 |
| PUT、DELETE | `/networks/{network}/forwards/{id}` | 替换、删除端口转发 |
| GET | `/networks/{network}/pairs` | 列出节点对设置 |
| PUT、DELETE | `/networks/{network}/pairs/{a}/{b}` | 设置、去掉一对节点的设置 |
| GET | `/networks/{network}/access/graph` | 访问关系的标签视图 |
| GET | `/networks/{network}/access/check` | 访问检查 |

目前没有的：主动测试（连通性测试）、节点更新、"到某个节点现在会怎么走"的即时查询、流量与配额的统计。
