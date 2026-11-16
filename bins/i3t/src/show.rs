//! 把正在运行的节点的状态打印成给人看的样子：`i3t status` 的下半段，和 `i3t routes`。

use i3t_agent::status::{Link, LiveStatus};
use i3t_model::control::Route;
use i3t_model::update::{FailureKind, UpdateStatus};

/// 没有值的地方显示的短横。
const NONE: &str = "-";

pub fn status(live: &LiveStatus) {
    println!("\n正在运行  i3t {}，已经运行 {}", live.version, duration(live.uptime_secs));
    let link = match live.link {
        Link::Static => "静态模式，不连 controller".to_owned(),
        Link::Online => "已连上".to_owned(),
        Link::Offline => "没有连上，按手上的配置运行，正在重连".to_owned(),
        Link::Revoked => "controller 吊销了这个节点".to_owned(),
        Link::Rejected => "controller 不认这个节点，要重新加入网络".to_owned(),
        Link::Unknown => "未知".to_owned(),
    };
    println!("controller {link}");
    println!("netmap    第 {} 版", live.netmap_version);
    println!("活跃的流  {}", live.flows);
    println!("候选端点  {}", list(live.endpoints.iter()));
    if live.mapping_varies == Some(true) {
        println!("  注意    这台机器所在的网络映射端口随对端变化，基本打不了洞");
    }
    println!("挂靠 relay {}", list(live.attached.iter().map(|attachment| format!("#{}({}ms)", attachment.relay, attachment.rtt_ms))));
    if let Some(relay) = &live.relay {
        println!(
            "本机 relay {} 个会话，转发了 {} 个包（{} 字节），丢掉 {} 个",
            relay.sessions, relay.forwarded_packets, relay.forwarded_bytes, relay.dropped
        );
    }
    println!("更新      {}", update(live));

    println!("\n监听：");
    if live.listeners.is_empty() {
        let why = if live.update.draining { "（节点正在排空，入口都关着）" } else { "（没有）" };
        println!("  {why}");
    }
    let rows = live.listeners.iter().map(|listener| {
        let state = match &listener.detail {
            Some(detail) => format!("{}（{detail}）", listener.state),
            None => listener.state.clone(),
        };
        vec![listener.name.clone(), listener.bind.to_string(), listener.active.to_string(), state]
    });
    table(&["NAME", "BIND", "ACTIVE", "STATE"], &rows.collect::<Vec<_>>());

    println!("\n路由：");
    if live.routes.is_empty() {
        println!("  （没有对端）");
    }
    route_table(live);
}

pub fn routes(live: &LiveStatus) {
    if live.routes.is_empty() {
        println!("没有对端：这个节点的配置里没有和它有访问关系的节点。");
    }
    route_table(live);
}

fn route_table(live: &LiveStatus) {
    let rows = live.routes.iter().map(|entry| {
        let (via, detail) = match &entry.route {
            Route::None => ("none", "没有可用的路".to_owned()),
            Route::Direct { addr } => ("direct", addr.to_string()),
            Route::Relay { relays } => ("relay", list(relays.iter().map(|relay| format!("#{relay}")))),
        };
        let since =
            entry.since.as_ref().map_or_else(|| NONE.to_owned(), |since| format!("{} 前：{}", duration(since.secs_ago), since.reason));
        vec![
            entry.name.clone(),
            entry.virtual_ip.map_or_else(|| NONE.to_owned(), |ip| ip.to_string()),
            via.to_owned(),
            entry.rtt_ms.map_or_else(|| NONE.to_owned(), |rtt| rtt.to_string()),
            detail,
            since,
        ]
    });
    table(&["PEER", "VIRTUAL-IP", "VIA", "RTT-MS", "DETAIL", "CHANGED"], &rows.collect::<Vec<_>>());
}

/// 更新的情况，一句话。
fn update(live: &LiveStatus) -> String {
    let progress = match &live.update.status {
        UpdateStatus::Idle => "没有更新在进行".to_owned(),
        UpdateStatus::Downloading { version, received, total } => format!("正在从 controller 取 {version} 的包（{received}/{total} 字节）"),
        UpdateStatus::Verifying { version } => format!("正在核对 {version} 的包"),
        UpdateStatus::SelfCheck { version } => format!("正在运行 {version} 的自检"),
        UpdateStatus::WaitingForIdle { version } => format!("{version} 已经准备好，等没有活跃的流时换上去"),
        UpdateStatus::Draining { version } => format!("{version} 已经准备好，正在排空：不接新的流，手上的结束之后换上去"),
        UpdateStatus::Installing { version } => format!("正在换上 {version}"),
        UpdateStatus::Restarting { version } => format!("已经换上 {version}，马上重启"),
        UpdateStatus::Confirmed { version } => format!("已经更新到 {version}"),
        UpdateStatus::Failed { version, kind, reason } => {
            let version = version.as_ref().map_or_else(|| "新版本".to_owned(), ToString::to_string);
            let blame = match kind {
                FailureKind::VersionRejected => "这个版本在这台机器上不能用，不会再自动重试",
                FailureKind::NotInstalled => "没有装成，以后会再试",
            };
            format!("更新到 {version} 失败（{blame}）：{reason}")
        }
        UpdateStatus::Unknown => "未知".to_owned(),
    };
    match live.update.self_update {
        Some(ability) if !ability.able => {
            let why = ability.blocker.map_or_else(|| "更新的记录读不了".to_owned(), |blocker| blocker.to_string());
            format!("{progress}。这个节点不能自己更新：{why}")
        }
        _ => progress,
    }
}

/// 一段时间写成"3 小时 12 分"这样。
fn duration(secs: u64) -> String {
    match (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60) {
        (0, 0, 0) => format!("{secs} 秒"),
        (0, 0, minutes) => format!("{minutes} 分 {} 秒", secs % 60),
        (0, hours, minutes) => format!("{hours} 小时 {minutes} 分"),
        (days, hours, _) => format!("{days} 天 {hours} 小时"),
    }
}

fn list<T: ToString>(values: impl IntoIterator<Item = T>) -> String {
    let joined = values.into_iter().map(|value| value.to_string()).collect::<Vec<_>>().join("，");
    if joined.is_empty() { NONE.to_owned() } else { joined }
}

/// 打印一张对齐的表，每行缩进两格。表头是英文：中文字符的宽度会让列对不齐，会有中文的内容放在最后几列。
fn table(header: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }
    let width = |column: usize| rows.iter().map(|row| row[column].chars().count()).chain([header[column].len()]).max().unwrap_or(0);
    let widths: Vec<usize> = (0..header.len()).map(width).collect();
    let line = |cells: &mut dyn Iterator<Item = &str>| {
        let padded = cells.zip(&widths).map(|(cell, width)| format!("{cell}{}", " ".repeat(width + 2 - cell.chars().count())));
        println!("  {}", padded.collect::<String>().trim_end());
    };
    line(&mut header.iter().copied());
    rows.iter().for_each(|row| line(&mut row.iter().map(String::as_str)));
}

#[cfg(test)]
mod tests {
    use i3t_agent::status::LiveUpdate;
    use i3t_model::control::SelfUpdate;
    use i3t_model::update::SelfUpdateBlocker;

    use super::*;

    #[test]
    fn durations_use_the_two_largest_units() {
        assert_eq!(duration(42), "42 秒");
        assert_eq!(duration(192), "3 分 12 秒");
        assert_eq!(duration(3 * 3600 + 12 * 60 + 5), "3 小时 12 分");
        assert_eq!(duration(2 * 86400 + 5 * 3600 + 60), "2 天 5 小时");
    }

    #[test]
    fn the_update_line_says_what_is_going_on_and_why_a_node_cannot_update_itself() {
        let with =
            |status, self_update| LiveStatus { update: LiveUpdate { status, self_update, draining: false }, ..LiveStatus::default() };
        assert_eq!(update(&with(UpdateStatus::Idle, Some(SelfUpdate::ABLE))), "没有更新在进行");
        let blocked = update(&with(UpdateStatus::Idle, Some(SelfUpdate::from(Some(SelfUpdateBlocker::Container)))));
        assert!(blocked.contains("不能自己更新") && blocked.contains("容器"), "{blocked}");
        let waiting = update(&with(UpdateStatus::WaitingForIdle { version: "0.2.0".parse().unwrap() }, None));
        assert!(waiting.contains("0.2.0") && waiting.contains("等没有活跃的流"), "{waiting}");
    }
}
