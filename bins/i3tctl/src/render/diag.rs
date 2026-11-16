//! 主动测试和全网矩阵打印成给人看的样子。

use i3t_model::admin::NodeRef;
use i3t_model::admin::diag::{
    DecisionView, DirectResult, ItemState, MatrixRun, PathResult, TestProblem, TestView, ThroughputResult, Transfer,
};
use i3t_model::control::diag::{Choice, Hop, HopSource, Latency};
use i3t_proto::NodeId;

use super::{millis, route_cells, time};
use crate::table::{NONE, Table, label, list, optional};

/// 节点编号写成名字：测试的两端和矩阵里的节点有名字，别的（relay）写成 `#编号`。
fn namer(nodes: &[&NodeRef]) -> impl Fn(NodeId) -> String + use<> {
    let known: Vec<(NodeId, String)> = nodes.iter().map(|node| (node.id, node.name.clone())).collect();
    move |id| known.iter().find(|(known, _)| *known == id).map_or_else(|| format!("#{id}"), |(_, name)| name.clone())
}

/// 一个原因：`udp_blocked（nas）：UDP 不通`。
fn problem_text(problem: &TestProblem, name: &impl Fn(NodeId) -> String) -> String {
    let node = problem.node.map(|node| format!("（{}）", name(node))).unwrap_or_default();
    format!("{}{node}：{}", label(&problem.code), problem.message)
}

/// 千分之几写成带一位小数的百分数。
fn percent(permille: u16) -> String {
    format!("{}.{}", permille / 10, permille % 10)
}

fn mbps(transfer: Option<&Transfer>) -> String {
    transfer
        .and_then(|transfer| transfer.bits_per_second)
        .map_or_else(|| NONE.to_owned(), |rate| format!("{}.{}", rate / 1_000_000, rate % 1_000_000 / 100_000))
}

pub fn test(view: &TestView) {
    let name = namer(&[&view.a, &view.b]);
    let items = list(view.request.items.iter().map(label));
    let throughput = if view.request.throughput { "，含吞吐" } else { "" };
    println!(
        "测试 #{}  {} → {}  {}  开始于 {}（{items}{throughput}）",
        view.id,
        view.a.name,
        view.b.name,
        label(&view.state),
        time(view.started_at)
    );
    if let Some(error) = &view.error {
        println!("没有做成：{}", problem_text(error, &name));
    }
    if let Some(direct) = &view.direct {
        direct_lines(direct, &name);
    }
    let paths: Vec<&PathResult> = view.direct_quality.iter().chain(&view.relay_quality).collect();
    if !paths.is_empty() {
        println!("\n路径质量：");
        let mut rows = Table::new(&[
            "VIA",
            "PICKED",
            "STATE",
            "ANSWERED",
            "LOSS%",
            "UP%",
            "DOWN%",
            "MIN-MS",
            "AVG-MS",
            "P95-MS",
            "JITTER-MS",
            "COST-MS",
            "DETAIL",
        ]);
        for path in &paths {
            let (via, detail) = path.route.as_ref().map_or((NONE, NONE.to_owned()), route_cells);
            let latency = path.latency.unwrap_or_default();
            let ms = |value: Option<u32>| value.map_or_else(|| NONE.to_owned(), |us| millis(u64::from(us)));
            let detail = match &path.problem {
                Some(problem) => format!("{detail} {}", problem_text(problem, &name)),
                None => detail,
            };
            rows.row(vec![
                via.to_owned(),
                list(path.picked_by.iter().map(label)),
                label(&path.state),
                path.latency.map_or_else(|| NONE.to_owned(), |latency| format!("{}/{}", latency.answered, latency.probes)),
                path.latency.map_or_else(|| NONE.to_owned(), |latency| percent(latency.loss_permille)),
                optional(latency.loss_up_permille.map(percent)),
                optional(latency.loss_down_permille.map(percent)),
                ms(latency.min_us),
                ms(latency.avg_us),
                ms(latency.p95_us),
                ms(latency.jitter_us),
                path.cost_us.map_or_else(|| NONE.to_owned(), millis),
                detail,
            ]);
        }
        rows.print();
        for path in &paths {
            hop_lines(path, &name);
            reverse_note(path.latency.as_ref());
        }
        throughput_lines(&paths, &name);
    }
    if let Some(route) = &view.route {
        println!("\n两种选路模式的选择（{}）：", label(&route.state));
        let mut rows = Table::new(&["FROM", "TO", "MODE", "CURRENT", "DIRECT-FIRST", "LOWEST-COST"]);
        for decision in [&route.forward, &route.reverse].into_iter().flatten() {
            rows.row(decision_row(decision, &name));
        }
        rows.print();
        if let Some(problem) = &route.problem {
            println!("  {}", problem_text(problem, &name));
        }
    }
    for note in &view.notes {
        println!("注意：{}", problem_text(note, &name));
    }
}

fn direct_lines(direct: &DirectResult, name: &impl Fn(NodeId) -> String) {
    let verdict = match (direct.state, direct.ok) {
        (ItemState::Done, true) if direct.already => format!("可以：已经在直连（{}）", optional(direct.kind.map(|kind| label(&kind)))),
        (ItemState::Done, true) => {
            format!("可以：{}，用了 {}ms", optional(direct.kind.map(|kind| label(&kind))), optional(direct.elapsed_ms))
        }
        (ItemState::Done, false) => "不行".to_owned(),
        (state, _) => label(&state),
    };
    println!("\n直连能力：{verdict}");
    if let Some(problem) = &direct.problem {
        println!("  原因：{}", problem_text(problem, name));
    }
    for side in &direct.sides {
        let side_view = &side.side;
        let failure = side_view.failure.map(|code| format!(" 原因 {}", label(&code))).unwrap_or_default();
        let udp = side_view.udp.map_or(NONE, |udp| if udp { "通" } else { "不通" });
        println!(
            "  {}：{}{failure}，UDP {udp}，候选地址 {} 个",
            name(side.node),
            if side_view.ok { "连上了" } else { "没连上" },
            side_view.candidates
        );
    }
}

/// 经 relay 的路径上每一段的延迟，和它从哪里来。
fn hop_lines(path: &PathResult, name: &impl Fn(NodeId) -> String) {
    if path.hops.len() < 2 {
        return;
    }
    let (via, detail) = path.route.as_ref().map_or((NONE, NONE.to_owned()), route_cells);
    let hops: Vec<String> = path.hops.iter().map(|hop| hop_text(hop, name)).collect();
    println!("  {via} {detail} 每一段：{}", hops.join("；"));
}

fn hop_text(hop: &Hop, name: &impl Fn(NodeId) -> String) -> String {
    let rtt = hop.rtt_us.map_or_else(|| NONE.to_owned(), |us| format!("{}ms", millis(u64::from(us))));
    let loss = hop.loss_permille.map(|loss| format!(" 丢 {}%", percent(loss))).unwrap_or_default();
    let source = match hop.source {
        HopSource::Measured => "实测",
        HopSource::Backbone => "骨干",
        HopSource::Reported => "对端报的",
        HopSource::Unknown => "不知道",
    };
    format!("{}→{} {rtt}{loss}（{source}）", name(hop.from), name(hop.to))
}

/// 回答没有全部沿原路回来时说一声：那时回程走的是对端自己的路。
fn reverse_note(latency: Option<&Latency>) {
    if let Some(latency) = latency.filter(|latency| latency.answered > 0 && latency.same_path < latency.answered) {
        println!("  {} 个回答里有 {} 个走的是对端自己的路，不是原路", latency.answered, latency.answered - latency.same_path);
    }
}

fn throughput_lines(paths: &[&PathResult], name: &impl Fn(NodeId) -> String) {
    let measured: Vec<(&PathResult, &ThroughputResult)> =
        paths.iter().filter_map(|path| path.throughput.as_ref().map(|throughput| (*path, throughput))).collect();
    if measured.is_empty() {
        return;
    }
    println!("\n吞吐：");
    let mut rows = Table::new(&["VIA", "STATE", "UP-MBPS", "UP-MB", "UP-MS", "DOWN-MBPS", "DOWN-MB", "DOWN-MS", "PINNED", "DETAIL"]);
    for (path, throughput) in &measured {
        let (via, detail) = path.route.as_ref().map_or((NONE, NONE.to_owned()), route_cells);
        let measured = &throughput.measured;
        let megabytes = |transfer: Option<&Transfer>| {
            transfer.map_or_else(
                || NONE.to_owned(),
                |transfer| format!("{:.1}", f64::from(u32::try_from(transfer.bytes / 100_000).unwrap_or(u32::MAX)) / 10.0),
            )
        };
        let detail = match &throughput.problem {
            Some(problem) => format!("{detail} {}", problem_text(problem, name)),
            None => detail,
        };
        rows.row(vec![
            via.to_owned(),
            label(&throughput.state),
            mbps(measured.up.as_ref()),
            megabytes(measured.up.as_ref()),
            optional(measured.up.map(|transfer| transfer.ms)),
            mbps(measured.down.as_ref()),
            megabytes(measured.down.as_ref()),
            optional(measured.down.map(|transfer| transfer.ms)),
            if measured.pinned { "yes" } else { "no" }.to_owned(),
            detail,
        ]);
    }
    rows.print();
    if let Some((_, throughput)) = measured.iter().find(|(_, throughput)| !throughput.measured.congestion.is_empty()) {
        let measured = &throughput.measured;
        println!(
            "  用的是 {} + {}，每个方向最多 {}MB 或 {}ms，先到为止；测量期间这一对节点之间别的流量也走同一条路",
            measured.transport,
            measured.congestion,
            measured.limit_bytes / 1_000_000,
            measured.limit_ms
        );
    }
}

fn choice_text(choice: &Choice) -> String {
    let route = choice.route.as_ref().map_or_else(
        || "none".to_owned(),
        |route| {
            let (via, detail) = route_cells(route);
            format!("{via} {detail}")
        },
    );
    let cost = choice.cost_us.map(|cost| format!(" {}ms", millis(cost))).unwrap_or_default();
    format!("{route}（{}{cost}）", label(&choice.reason))
}

fn decision_row(decision: &DecisionView, name: &impl Fn(NodeId) -> String) -> Vec<String> {
    let current = decision.current.as_ref().map_or_else(
        || NONE.to_owned(),
        |route| {
            let (via, detail) = route_cells(route);
            format!("{via} {detail}")
        },
    );
    vec![
        name(decision.from),
        name(decision.to),
        optional(decision.mode.as_ref().map(label)),
        current,
        choice_text(&decision.direct_first),
        choice_text(&decision.lowest_cost),
    ]
}

pub fn tests(views: &[TestView]) {
    let mut rows = Table::new(&["ID", "A", "B", "STATE", "STARTED", "DIRECT", "AVG-MS", "ITEMS"]);
    for view in views {
        let direct = view.direct.as_ref().map_or_else(
            || NONE.to_owned(),
            |direct| match direct.state {
                ItemState::Done => {
                    if direct.ok {
                        optional(direct.kind.map(|kind| label(&kind)))
                    } else {
                        "no".to_owned()
                    }
                }
                state => label(&state),
            },
        );
        let latency =
            view.direct_quality.iter().chain(&view.relay_quality).find_map(|path| path.latency.and_then(|latency| latency.avg_us));
        let throughput = if view.request.throughput { "+throughput" } else { "" };
        rows.row(vec![
            view.id.to_string(),
            view.a.name.clone(),
            view.b.name.clone(),
            label(&view.state),
            time(view.started_at),
            direct,
            latency.map_or_else(|| NONE.to_owned(), |us| millis(u64::from(us))),
            format!("{}{throughput}", list(view.request.items.iter().map(label))),
        ]);
    }
    rows.print();
}

pub fn matrix(run: &MatrixRun) {
    let nodes: Vec<&NodeRef> = run.nodes.iter().collect();
    let name = namer(&nodes);
    println!(
        "全网矩阵 #{}  {}  {}/{} 对有了结果，同时最多 {} 个，开始于 {}",
        run.id,
        label(&run.state),
        run.finished,
        run.total,
        run.concurrency,
        time(run.started_at)
    );
    let mut rows = Table::new(&["A", "B", "STATE", "TEST", "DIRECT", "DIRECT-MS", "VIA", "AVG-MS", "LOSS%", "DETAIL"]);
    for cell in &run.cells {
        let (via, detail) = cell.route.as_ref().map_or((NONE, NONE.to_owned()), route_cells);
        let direct = match cell.direct_ok {
            Some(true) => optional(cell.direct_kind.map(|kind| label(&kind))),
            Some(false) => "no".to_owned(),
            None => NONE.to_owned(),
        };
        let detail = match &cell.problem {
            Some(problem) => format!("{detail} {}", problem_text(problem, &name)),
            None => detail,
        };
        rows.row(vec![
            name(cell.a),
            name(cell.b),
            label(&cell.state),
            optional(cell.test),
            direct,
            optional(cell.direct_elapsed_ms),
            via.to_owned(),
            cell.latency.and_then(|latency| latency.avg_us).map_or_else(|| NONE.to_owned(), |us| millis(u64::from(us))),
            cell.latency.map_or_else(|| NONE.to_owned(), |latency| percent(latency.loss_permille)),
            detail,
        ]);
    }
    rows.print();
}

#[cfg(test)]
mod tests {
    use i3t_model::admin::diag::TestItem;

    use super::*;

    #[test]
    fn numbers_are_shown_in_the_units_people_read() {
        assert_eq!(percent(25), "2.5");
        let transfer = Transfer {
            bytes: 20_000_000,
            ms: 4000,
            bits_per_second: Some(40_123_456),
            stopped_by: i3t_model::control::diag::TransferEnd::Bytes,
        };
        assert_eq!(mbps(Some(&transfer)), "40.1");
        assert_eq!(mbps(None), NONE);
        let name = |id: NodeId| format!("n{id}");
        let problem = TestProblem::new(i3t_model::control::diag::TestCode::UdpBlocked, NodeId::new(2));
        assert_eq!(problem_text(&problem, &name), "udp_blocked（n2）：UDP 不通");
        assert_eq!(label(&TestItem::DirectQuality), "direct_quality");
    }
}
