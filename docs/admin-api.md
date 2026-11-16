# ice3tier 管理 API

这份文档写给做 WebUI 和写脚本的人：controller 的管理接口有哪些，每个接口的方法、路径、请求和应答的 JSON，以及出错时的格式。命令行 `i3tctl` 用的就是这套接口。

对应的 Rust 类型在 `crates/i3t-model/src/admin.rs`，实现在 `crates/i3t-controller/src/admin.rs`。文档和代码不一致时以代码为准，并请修正文档。

> **本次新增**（节点更新、路由视图、指标、内嵌的 WebUI）。正文里新加的地方都标了「新增」，这里是清单：
>
> - 新接口：`GET /update`、`PATCH /update`、`POST /update/check`、`POST /update/resume`、`POST /update/force`（第 14 节）；`GET`、`POST /networks/{network}/nodes/{node}/update`（第 14 节）；`GET /networks/{network}/routes`、`GET /networks/{network}/path`（第 15 节）；`GET /metrics`（不在 `/api/v1` 下面，第 16 节）。
> - `Node` 多了两个字段：`hostname`、`update`（第 5 节）。节点详情 `NodeDetail` 随之也有。
> - `Status` 多了一个字段：`newer_version`（第 3 节）。
> - 新的错误码：`node_offline`、`cannot_self_update`、`no_update`（第 2 节）。
> - 操作记录多了六种 `action`：`update.settings`、`update.release`、`update.pause`、`update.resume`、`update.force`、`update.force_all`。
> - 行为上的变化：新节点的名字取自它的主机名，不再一律是 `node-<编号>`（第 5 节）；controller 自带 WebUI，没有带的构建在 `/` 上回一个说明页，不再是 404（1.1 节）。
>
> 已有接口的路径、请求体和应答里原有的字段都没有变。

## 1. 约定

### 1.1 地址与认证

- controller 的管理端口默认是 `127.0.0.1:20332`，明文 HTTP。所有接口的路径都以 `/api/v1` 开头，下文的路径省略这个前缀。
- 每个请求都要带管理令牌：

  ```
  Authorization: Bearer <管理令牌>
  ```

  管理令牌在 controller 第一次启动时打印一次，controller 只存它的哈希。只有一个管理员，没有角色。令牌丢了要停掉 controller，用 `i3t-controller --reset-admin-token` 重新生成。
- 没带令牌或令牌不对时，应答是 `401`，错误码 `unauthorized`，并带 `WWW-Authenticate: Bearer` 头。
- 「新增」WebUI 和管理 API 在同一个端口上：`/api/v1` 和 `/metrics` 之外的路径都是 WebUI 的静态文件，不是文件的路径回 `index.html`（给单页应用的前端路由用）。静态文件不需要令牌。WebUI 和 API 同源，所以 controller 不发 CORS 头；开发时请让开发服务器把 `/api` 代理到 controller。静态文件有三种来路：
  - 编译进 `i3t-controller` 里的：编译 controller 时如果仓库里有 `web/dist`（并且里面有 `index.html`），它的全部文件被打进可执行文件。`index.html` 带 `Cache-Control: no-cache`，`assets/` 下面的文件（文件名里带内容的哈希）带 `Cache-Control: public, max-age=31536000, immutable`。
  - `i3t-controller --static-dir <目录>`：从这个目录取，不用编译进去的那一份。
  - 都没有：`/api/v1` 和 `/metrics` 之外的 `GET` 一律回 `200` 和一个 HTML 说明页，内容是"这次构建没有带 WebUI"和补救的办法。
- 「新增」`GET /metrics` 是 Prometheus 的文本格式，不要令牌，见第 16 节。
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
| 409 | `node_offline` | 「新增」要让它立即更新的节点不在线 |
| 409 | `cannot_self_update` | 「新增」节点不能自己更新：在容器里、是包管理器装的、程序所在的目录不可写，或者它的版本太旧不懂更新。`message` 里写着原因 |
| 409 | `no_update` | 「新增」没有可以让这个节点更新到的版本：还没有查到发布、它已经是最新的、发布里没有它的平台的包，或者 controller 自己太旧。`message` 里写着是哪一种 |
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
  "online": 2,
  "newer_version": "0.3.0"
}
```

| 字段 | 含义 |
|---|---|
| `version` | controller 程序的版本号 |
| `controller_key` | controller 的公钥，写在每个注册令牌里 |
| `public_addr` | 节点连 controller 用的 `主机:端口`，写在每个注册令牌里 |
| `networks`、`nodes`、`online` | 全部网络的数量、全部节点的数量、在线节点的数量 |
| `newer_version` | 「新增」已知的最新正式版比 controller 自己新时，是那个版本号；否则 `null`。controller 不会自己更新，界面据此提醒管理员手动更新 |

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

`action` 的取值：`admin.token`、`network.create`、`network.update`、`network.delete`、`node.enroll`、`node.update`、`node.delete`、`node.succeed`、`node.relay.enable`、`node.relay.disable`、`node.relay.withdrawn`、`node.exit.enable`、`node.exit.disable`、`node.exit.withdrawn`、`node.forward.withdrawn`、`token.create`、`token.delete`、`rule.create`、`rule.update`、`rule.delete`、`credential.create`、`credential.delete`、`listener.create`、`listener.update`、`listener.delete`、`forward.create`、`forward.update`、`forward.delete`、`pair.set`、`pair.delete`，以及「新增」`update.settings`、`update.release`（发布源上有了新的正式版）、`update.pause`（节点更新失败，版本被暂停；它记在那个节点所在的网络上）、`update.resume`、`update.force`（记在节点所在的网络上）、`update.force_all`。带 `withdrawn` 的三种是节点撤回了本地声明、controller 随之关闭能力时记的。最多保留最近的一万条。

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
  "acked_version": 0,
  "hostname": "edge-hk-01.example.net",
  "update": {
    "self_update": { "able": true, "blocker": null },
    "available": "0.3.0",
    "status": { "state": "waiting_for_idle", "version": "0.3.0" },
    "ordered": { "version": "0.3.0", "timing": "idle" },
    "waiting": null,
    "failure": null
  }
}
```

| 字段 | 含义 |
|---|---|
| `id` | 节点编号 |
| `network` | 所属网络的编号 |
| `key` | 节点的公钥 |
| `name` | 名字，之后可以改。「新增」节点加入时取它上报的主机名：只取第一段（`nas.lan` 取 `nas`），转成小写，名字里不能有的字符换成连字符；同一个网络里重名（或者那个名字还在冷却期）就加 `-2`、`-3`……。主机名取不出合法的名字（空的、全是数字、不是 ASCII、`localhost`、`node-<数字>`）或者节点的版本太旧没有报主机名时，叫 `node-<编号>` |
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
| `hostname` | 「新增」节点上报的主机名，原样；旧版本的节点不报，是 `""`。主机名后来变了，`name` 不跟着变 |
| `update` | 「新增」这个节点的程序更新：能不能自己更新、有没有新版本、进行到哪一步。结构是 `NodeUpdate`，见第 14 节 |

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
  "hostname": "laptop",
  "update": {
    "self_update": { "able": true, "blocker": null },
    "available": null,
    "status": { "state": "idle" },
    "ordered": null,
    "waiting": null,
    "failure": null
  },
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
| GET、PATCH | `/update` | 「新增」更新的设置和现状；改设置 |
| POST | `/update/check` | 「新增」现在查一次发布源 |
| POST | `/update/resume` | 「新增」继续一个被暂停的版本的自动更新 |
| POST | `/update/force` | 「新增」让全部节点立即更新 |
| GET、POST | `/networks/{network}/nodes/{node}/update` | 「新增」一个节点的更新情况；让它立即更新 |
| GET | `/networks/{network}/routes` | 「新增」整个网络的路由矩阵 |
| GET | `/networks/{network}/path` | 「新增」两个节点之间两个方向的当前路径 |

不在 `/api/v1` 下面的：`GET /metrics`（「新增」，第 16 节）。

目前没有的：主动测试（连通性测试）、流量与配额的统计。

## 14. 节点更新「新增」

节点程序 `i3t` 可以自己更新。更新是全局的，不分网络：设置只有一份，发布也只有一份。大致的过程：

1. controller 每小时查一次**发布源**（默认是项目的 GitHub Release，可以改成镜像或者 controller 所在机器上的一个目录），取最新正式版的清单。清单带签名，controller 验过才认；预发布版不取。
2. 有节点要用的平台的包，controller 从发布源取一次，核对大小和 SHA-256，缓存下来。
3. controller 给该更新的节点下发指令：清单原文、签名、**时机**。节点自己再验一遍签名，经和 controller 的连接把包一段一段取回去，运行新程序的自检，到了时机就换上去并重启。
4. 新程序连上 controller 才算成功。连不上的话节点自己换回旧程序，回来报告失败。

时机（`timing`）有三种：

| `timing` | 含义 | 谁用 |
|---|---|---|
| `"idle"` | 等节点上没有活跃的流才换。一直有流的节点一直等 | 自动更新 |
| `"now"` | 马上换并重启，正在进行的流会断 | 管理员点"立即更新" |
| `"after_drain"` | 节点不再接新的流，等手上的流结束再换。排空期间发给它的新请求被拒绝，调用方要换别的出口 | 管理员选"排空后更新" |

让谁更新的规矩（界面上需要解释"为什么这个节点还没更新"时用）：

- 只有**能更新的节点**才会收到指令：在线、版本比已知的发布旧、发布里有它的平台的包、它自己报了能自己更新（`update.self_update.able`）。不满足的节点强制也不行。
- **自动更新**（`auto_update` 开着）给这些节点下发 `idle`，除非这个版本被暂停了，或者这个节点在这个版本上失败过。
- **强制更新**不看开关、暂停和以前的失败。
- **relay 一个一个来**：同一个网络里一次只让一个 relay 更新，它更新完、重新连上来再过 30 秒，才轮到下一个。轮到的那个 relay 离线期间别的 relay 一直等。单独对一个 relay 点"立即更新"不排这个队；"全部更新"里的 relay 照样排队。网络里唯一的 relay 会等到没有人经它转发才更新（这一条是节点自己判断的，时机是 `idle` 时生效）。
- **暂停**：节点报告"这个版本在我这台机器上不能用"（自检不通过，或者装上之后连不上 controller、已经换回旧程序）时，controller 记下这次失败，暂停这个版本对其余节点的自动更新，还没换上去的节点被叫停。管理员看过之后用 `POST /update/resume` 继续；失败过的那个节点仍然不会被自动更新到这个版本，要对它点"立即更新"才会再试。
- "没装成但不怪版本"的失败（包取不到、磁盘出错）不暂停，下一次查发布源时自动再试。
- 离线的节点不用特别处理：它重新连上来的时候和别的节点一样过一遍这些规矩。
- 发布要求的 controller 最低版本比 controller 自己新时，这个版本不下发给任何节点。

管理员强制的更新和 relay 的轮次只在 controller 的内存里：controller 重启之后，还没有做完的强制更新要重新点。设置、已知的发布、被暂停的版本和节点的失败记录存在数据库里。

### GET /update

更新的设置和现状。应答是 `UpdateOverview`：

```json
{
  "auto_update": true,
  "source": "https://github.com/fuqiuluo/ice3tier/releases",
  "source_is_default": true,
  "controller_version": "0.2.0",
  "release": {
    "version": "0.3.0",
    "published": 1763200000,
    "min_controller": "0.2.0",
    "controller_too_old": false,
    "newer_than_controller": true,
    "packages": [
      { "platform": "linux-aarch64", "file": "i3t-linux-aarch64", "size": 17100288, "state": "not_needed", "error": null },
      { "platform": "linux-x86_64", "file": "i3t-linux-x86_64", "size": 18234880, "state": "cached", "error": null }
    ]
  },
  "checked_at": 1794798564,
  "check_error": null,
  "paused": [
    {
      "version": "0.3.0",
      "node": { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
      "network": 1,
      "reason": "自检没有通过：退出码 1：读不了 config.toml",
      "at": 1794798600
    }
  ],
  "nodes": { "total": 5, "updating": 1, "failed": 1, "manual": 1, "outdated": 0, "up_to_date": 2 }
}
```

| 字段 | 含义 |
|---|---|
| `auto_update` | 自动更新的开关，默认开 |
| `source` | 发布源：一个 `http://` 或 `https://` 的地址前缀，或者 controller 所在机器上一个目录的绝对路径 |
| `source_is_default` | `source` 是不是项目默认的发布源 |
| `controller_version` | controller 自己的版本 |
| `release` | 已知的最新正式版；还没有查到过是 `null` |
| `release.version`、`release.published` | 版本号，发布时间 |
| `release.min_controller` | 这个版本的节点需要的 controller 最低版本 |
| `release.controller_too_old` | `true` 时 controller 比 `min_controller` 旧：这个版本不会下发给任何节点，界面应当提示先手动更新 controller |
| `release.newer_than_controller` | `true` 时这个版本比 controller 自己新：提醒管理员手动更新 controller（和 `GET /status` 的 `newer_version` 是一回事） |
| `release.packages` | 发布里各个平台的节点程序，按平台名排。`state` 见下表；`error` 是最近一次取这个包失败的原因，没有失败是 `null` |
| `checked_at` | 最近一次查发布源的时间；还没有查过是 `null` |
| `check_error` | 最近一次查发布源失败的原因（连不上、清单的签名不对、最新版是预发布版）；成功是 `null`。失败时 `release` 保持上一次查到的 |
| `paused` | 被暂停自动更新的版本，和导致暂停的那次失败。`node` 是报告失败的节点，它后来被删掉了的话是 `null`；`network` 是它所在的网络；`reason` 是节点报的原因；`at` 是时间 |
| `nodes` | 全部网络的节点按更新情况数出来的数。一个节点只算在一项里，按这个顺序取第一个符合的：`updating`（已经下发了指令，还没有结束）、`up_to_date`（没有可以更新到的版本）、`failed`（有新版本，但它上一次装这个版本失败了）、`manual`（有新版本，但它不能自己更新，要手动更新）、`outdated`（有新版本，可以更新）。`total` 是总数 |

`packages[].state`：

| 值 | 含义 |
|---|---|
| `"not_needed"` | 没有要更新的节点用这个平台，所以没有取 |
| `"fetching"` | 有节点要用，正在取，或者排着队等着取 |
| `"cached"` | 已经取回来并且核对过 |
| `"failed"` | 没有取成（`error` 里有原因），下一次查发布源时再试 |

### PATCH /update

改设置。字段都可以省略，省略表示不改。

```json
{ "auto_update": false, "source": "https://mirror.example.com/ice3tier/releases" }
```

| 字段 | 含义 |
|---|---|
| `auto_update` | 自动更新的开关。关掉时，已经下发、还没有换上去的自动更新被叫停；之后节点列表里只标出"有新版本"，等管理员点 |
| `source` | 发布源。写 `null` 表示改回项目默认的。发布源变了的话 controller 马上去查一次（在后台，不等它查完才应答）；已知的发布在查到新的之前保持不变 |

应答：`UpdateOverview`。
错误：`invalid_request`（`source` 既不是 `http(s)://` 地址，也不是绝对路径）。

### POST /update/check

现在查一次发布源，查完才应答（连不上发布源时最多等大约 30 秒）。请求体写 `{}`。

应答：`UpdateOverview`，状态码总是 `200`：查不到、验不过不算请求失败，原因在 `check_error` 里。查到的发布里有节点要用的包时，取包在后台进行，`packages[].state` 先是 `fetching`，取回来之后变成 `cached`，指令随即下发。

每查一次，上次没取成的包和上次"没装成但不怪版本"的节点都会再试一遍。

### POST /update/resume

继续一个被暂停的版本的自动更新。

```json
{ "version": "0.3.0" }
```

应答：`UpdateOverview`（`paused` 里不再有这个版本）。
错误：`not_found`（这个版本没有被暂停）。

### POST /update/force

"全部更新"：让全部网络里所有能更新的节点立即更新，不管自动更新开没开。

```json
{ "timing": "now" }
```

`timing` 是 `"now"` 或者 `"after_drain"`。应答是 `ForcedUpdates`：

```json
{
  "version": "0.3.0",
  "ordered": [{ "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" }],
  "queued": [{ "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3" }],
  "skipped": [
    { "node": { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" }, "reason": "节点 laptop 不在线。让节点立即更新要它连着 controller" }
  ]
}
```

| 字段 | 含义 |
|---|---|
| `version` | 要更新到的版本；没有可以下发的版本时是 `null`（这时节点都在 `skipped` 里） |
| `ordered` | 指令已经发出去的节点 |
| `queued` | 已经记下、稍后下发的节点：在等 controller 把包取回来，或者是 relay 在等轮到它 |
| `skipped` | 没有动的节点和原因（给人看的一句话）：不在线、不能自己更新、已经是最新的、没有它的平台的包 |

错误：`invalid_request`（`timing` 是 `"idle"`）。

### GET /networks/{network}/nodes/{node}/update

一个节点的更新情况。应答是 `NodeUpdate`，和 `Node` 里的 `update` 字段是同一个东西：

```json
{
  "self_update": { "able": false, "blocker": "container" },
  "available": "0.3.0",
  "status": { "state": "idle" },
  "ordered": null,
  "waiting": null,
  "failure": null
}
```

| 字段 | 含义 |
|---|---|
| `self_update` | 节点自己报的能不能自己更新。`able` 是 `false` 时 `blocker` 是原因（见下表），界面应当把"立即更新"置灰并显示手动更新的做法。整个字段是 `null` 表示节点的版本太旧，不懂更新，只能手动换程序 |
| `available` | 现在可以让它更新到的版本：已知的发布比它新、里面有它的平台的包、controller 也够新。没有是 `null`。不管它能不能自己更新、在不在线都给出，界面据此标"有新版本" |
| `status` | 节点本次连接期间最近一次报的进度（见下表）。不在线，或者没有更新在进行时是 `{ "state": "idle" }` |
| `ordered` | controller 在这次连接上发给它、还在做的更新指令：`version` 和 `timing`。没有是 `null` |
| `waiting` | controller 想让它更新、但指令还没有发的原因：`"package"`（它的平台的包还没有从发布源取回来）、`"relay_turn"`（它是 relay，在等前一个 relay 更新完重新上线）。没有在等是 `null` |
| `failure` | 它最近一次"这个版本在这台机器上不能用"的失败：`version`、`kind`（总是 `"version_rejected"`）、`reason`（节点报的原因）、`at`（时间）。有这条记录时，自动更新不会再让它装同一个版本。没有是 `null`。节点后来跑上了那个版本（或者更新的），或者管理员对它点了"立即更新"，这条记录就清掉 |

`self_update.blocker`：

| 值 | 含义 | 手动更新的做法 |
|---|---|---|
| `"container"` | 节点在容器里运行 | 换镜像 |
| `"package_manager"` | 节点程序是系统的包管理器装的 | 用包管理器更新 |
| `"read_only_dir"` | 节点程序所在的目录不可写 | 换一个服务账号可写的安装位置，或者手动替换程序文件 |
| `null`（而 `able` 是 `false`） | 节点说它不能，原因这个版本的 controller 不认识，或者节点上更新的记录读不了 | 手动替换程序文件 |

`status`：一个带 `state` 字段的对象，别的字段随 `state` 而不同。

| `state` | 别的字段 | 含义 |
|---|---|---|
| `"idle"` | — | 没有更新在进行 |
| `"downloading"` | `version`、`received`、`total` | 正在从 controller 取包，已经取了 `received` 字节，一共 `total` 字节 |
| `"verifying"` | `version` | 包取完了，正在核对大小和 SHA-256 |
| `"self_check"` | `version` | 正在运行新程序的自检 |
| `"waiting_for_idle"` | `version` | 新程序已经准备好，在等节点上没有活跃的流（界面显示"等待空闲"） |
| `"draining"` | `version` | 新程序已经准备好，节点不再接新的流，在等手上的流结束 |
| `"installing"` | `version` | 正在替换程序文件 |
| `"restarting"` | `version` | 程序文件已经换好，节点马上重启 |
| `"confirmed"` | `version` | 新程序连上了 controller，更新完成 |
| `"failed"` | `version`、`kind`、`reason` | 更新没有成功。`version` 在连清单都没有验过时是 `null`。`kind` 是 `"not_installed"`（没有装成，和这个版本好不好无关，以后会再试）或者 `"version_rejected"`（这个版本在这台机器上不能用）。`reason` 是给人看的原因 |
| `"unknown"` | — | 比 controller 新的节点报上来的、这个版本的 controller 不认识的状态 |

节点每 10 秒上报一次，状态一变还会马上报一次，所以这里看到的进度最多滞后一两秒。

### POST /networks/{network}/nodes/{node}/update

"立即更新"：让一个节点立即更新到已知的最新版，不管自动更新开没开、这个版本有没有被暂停、它以前有没有在这个版本上失败过。

```json
{ "timing": "after_drain" }
```

`timing` 是 `"now"` 或者 `"after_drain"`。应答：`NodeUpdate`。指令发出去了的话 `ordered` 里是它；包还没有取回来时 `ordered` 是 `null`、`waiting` 是 `"package"`，包一取回来指令就会发出去，不用再点。

错误：`invalid_request`（`timing` 是 `"idle"`）、`node_offline`、`cannot_self_update`、`no_update`、`not_found`。

## 15. 路由视图「新增」

每个节点为它的每个对端维护"现在走哪条路"，路是发送方在本地选的，所以两个方向可以不一样，数据也都来自节点。`route` 的写法见 `GET /networks/{network}/nodes/{node}/routes`（第 5 节）。

### GET /networks/{network}/routes

整个网络的路由矩阵，取自各个节点最近的上报。

```json
{
  "nodes": [
    { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1", "online": true },
    { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2", "online": true },
    { "id": 3, "name": "node-3", "virtual_ip": "10.66.0.3", "online": false }
  ],
  "entries": [
    { "from": 1, "to": 2, "route": { "via": "direct", "addr": "198.51.100.7:20330" }, "rtt_ms": 12, "age_secs": 4, "stale": false },
    { "from": 2, "to": 1, "route": { "via": "direct", "addr": "203.0.113.7:20330" }, "rtt_ms": 13, "age_secs": 7, "stale": false },
    { "from": 3, "to": 1, "route": { "via": "relay", "relays": [1] }, "rtt_ms": 40, "age_secs": 5210, "stale": true }
  ]
}
```

| 字段 | 含义 |
|---|---|
| `nodes` | 网络里的全部节点，按编号排：`NodeRef` 的三个字段加上 `online` |
| `entries` | 有数据的格子，按 `from`、`to` 排。一格是一个方向：`from` 到 `to` 现在怎么走。没有出现的格子是没有数据：两个节点之间没有访问关系，或者 `from` 还没有上报过 |
| `entries[].route`、`rtt_ms` | 走的路和这条路上最近测到的往返延迟（毫秒，没有是 `null`） |
| `entries[].age_secs` | 这一格是多少秒之前的：`from` 最近一次上报到现在过了多久。节点每 10 秒上报一次（路由表没有变化时不重复报内容，但上报本身就说明"没有变"），在线的节点一般不超过 10 |
| `entries[].stale` | `true` 表示这一格不是 `from` 现在这次连接报的：它不在线（这是它离线之前最后报的），或者刚重新连上还没有上报。界面应当把它标成过期 |

矩阵的着色可以直接按 `route.via` 来：`direct` 直连、`relay` 且 `relays` 只有一个是一级中转、多于一个是多级中转、`none` 不可达。controller 重启之后矩阵是空的，节点重新连上来上报了才有。

### GET /networks/{network}/path

两个节点之间两个方向的当前路径。查询参数 `from` 和 `to` 是两个节点的编号，都必须写。

controller 先向两端各问一次"到对端现在会怎么走"，节点用当前的测量值回答；问不到的方向（节点不在线、版本太旧不会回答、3 秒之内没有回答）退回到它最近上报的数据。所以这个接口最多要等 3 秒左右。

```json
{
  "forward": {
    "from": { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
    "to": { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" },
    "source": "query",
    "route": { "via": "direct", "addr": "203.0.113.7:20330" },
    "rtt_ms": 12,
    "age_secs": 0,
    "since": { "secs_ago": 340, "reason": "直连打通了" }
  },
  "reverse": {
    "from": { "id": 1, "name": "node-1", "virtual_ip": "10.66.0.1" },
    "to": { "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" },
    "source": "report",
    "route": { "via": "relay", "relays": [3] },
    "rtt_ms": 48,
    "age_secs": 6,
    "since": null
  }
}
```

`forward` 是 `from` 到 `to`，`reverse` 是 `to` 到 `from`。每个方向：

| 字段 | 含义 |
|---|---|
| `from`、`to` | 这个方向的发送方和接收方。这一段是发送方说的 |
| `source` | 数据从哪里来，见下表 |
| `route` | 现在走的路；没有数据是 `null` |
| `rtt_ms` | 这条路上最近测到的往返延迟，没有是 `null` |
| `age_secs` | 这份数据是多少秒之前的。刚问到的是 0；没有数据是 `null` |
| `since` | 这条路是多久之前换上的（`secs_ago`，秒）、为什么换（`reason`，节点的路由层给的一句话）。只有 `source` 是 `"query"`、并且节点这次启动以来换过这条路时才有，否则 `null` |

`source`：

| 值 | 含义 |
|---|---|
| `"query"` | 刚刚问的发送方，它用当前的测量值答的 |
| `"report"` | 没有问到发送方，这是它最近一次上报的路由表里的 |
| `"not_peers"` | 发送方说对端不在它的配置里：访问规则没有把这两个节点连通（或者配置还没有下发到）。`route` 是 `null` |
| `"none"` | 没有数据：发送方没有上报过到这个对端的路，现在也问不到 |

节点回答的是它的路由层手上现成的结论，不会为了回答去探测或者建立连接。两个节点之间从来没有通过信的话，结论多半是 `{ "via": "none" }` 或者一条还没有验证过直连的 relay 路径。候选路径的列表、每一段的延迟和丢包、直连的类型（局域网、打洞等）现在都还没有。

错误：`invalid_request`（缺 `from` 或 `to`，或者两个是同一个节点）、`not_found`（节点不在这个网络里）。

## 16. 指标「新增」

### GET /metrics

Prometheus 的文本格式（`Content-Type: text/plain; version=0.0.4`）。**路径前面没有 `/api/v1`，也不要管理令牌**：抓指标的程序不该拿着能改一切的管理令牌，所以这里只有合计的数，没有节点的名字、地址和任何配置。

| 指标 | 类型 | 含义 |
|---|---|---|
| `i3t_controller_info{version}` | gauge | 恒为 1，标签里是 controller 的版本 |
| `i3t_controller_networks` | gauge | 网络的数量 |
| `i3t_controller_nodes` | gauge | 注册了的节点的数量 |
| `i3t_controller_nodes_online` | gauge | 现在连着的节点的数量 |
| `i3t_controller_snapshots_sent_total` | counter | 启动以来发给节点的配置（netmap）的份数 |
| `i3t_controller_route_queries_total` | counter | 启动以来发给节点的路由查询的条数 |
| `i3t_controller_update_auto` | gauge | 自动更新的开关：开是 1，关是 0 |
| `i3t_controller_update_release_info{version}` | gauge | 恒为 1，标签里是已知的最新正式版；还没有查到时没有这个指标 |
| `i3t_controller_update_paused_versions` | gauge | 被暂停自动更新的版本的数量 |
| `i3t_controller_update_nodes{state}` | gauge | 节点按更新情况数出来的数量，`state` 是 `updating`、`failed`、`manual`、`outdated`、`up_to_date`（和 `GET /update` 的 `nodes` 一样） |
| `i3t_controller_update_orders_total` | counter | 启动以来下发的更新指令的条数 |
| `i3t_controller_update_cancels_total` | counter | 启动以来撤回的更新指令的条数 |
| `i3t_controller_update_version_failures_total` | counter | 启动以来节点报告的"这个版本不能用"的次数 |
| `i3t_controller_release_checks_total` | counter | 启动以来查发布源的次数 |
| `i3t_controller_release_check_failures_total` | counter | 其中没有成功的次数 |
| `i3t_controller_package_bytes_sent_total` | counter | 启动以来发给节点的更新包的字节数 |
