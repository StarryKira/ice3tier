# 随 controller 发布的数据

| 文件 | 内容 | 来源和许可 |
| --- | --- | --- |
| `dbip-country-lite.mmdb.gz` | IP 地址到国家或地区的离线库（MMDB 格式，gzip 压缩），给节点的 `country` 用（`src/geo.rs`） | [DB-IP](https://db-ip.com) 的 IP to Country Lite，[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)。使用时要注明来源：WebUI 设置的「关于」里写着 |

更新地址库：从 <https://db-ip.com/db/download/ip-to-country-lite> 下载当月的 `dbip-country-lite-<年>-<月>.mmdb.gz`，换掉上面那个文件（文件名不变）。
