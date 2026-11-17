//! 透明访问在本机的几个命令（只在 Linux 上）：`i3t set intercept` 之后的检查和清理、`i3t intercept status`、`i3t hosts`。
//!
//! 装、拆都是运行着的节点做的（`i3t_agent` 的 `intercept` 模块）；这里只读系统里有什么，以及节点没在运行时
//! 把上次留下的东西拆掉。

use std::fs;

use anyhow::Context;
use i3t_agent::intercept::names_of;
use i3t_agent::state::StateDir;
use i3t_agent::status::LiveStatus;
use i3t_intercept::machine::Shell;
use i3t_intercept::names::{hosts_block, replace_hosts_block};
use i3t_intercept::{Capability, CommandError, Program, Survey, TABLE, setup};
use i3t_lang::table::Fields;
use i3t_model::control::InterceptCause;

use crate::texts::texts;

const HOSTS: &str = "/etc/hosts";

/// `i3t set intercept on|off` 写好配置文件之后：打开时说这台机器上缺什么；关掉、节点又没在运行时，拆掉上次留下的东西。
pub(crate) fn after_set(on: bool, running: bool) {
    let t = &texts().ui;
    let codes = i3t_lang::current().codes();
    if on {
        let missing: Vec<String> = [Program::Nft, Program::Ip]
            .into_iter()
            .filter(|program| Shell::locate(*program).is_none())
            .map(|program| (codes.intercept_cause)(&InterceptCause::ProgramMissing { program: program.name().to_owned() }))
            .collect();
        if !missing.is_empty() {
            println!("{}", (t.intercept_missing)(&i3t_lang::current().list(missing)));
        }
        return;
    }
    // 节点在运行时由它在停下的时候拆；它停下之后再起来，看到关着也会拆掉留下的。
    if running || Capability::missing().contains(&Capability::NetAdmin) || Shell::locate(Program::Nft).is_none() {
        return;
    }
    if !Survey::take(&mut Shell).found_any() {
        return;
    }
    match setup::remove(&mut Shell) {
        Ok(()) => println!("{}", t.intercept_removed),
        Err(_) => println!("{}", t.intercept_remove_failed),
    }
}

/// `i3t intercept status`：运行着的节点说的状态，和系统里属于透明访问的每一样东西。
pub(crate) fn status(live: Option<&LiveStatus>) {
    let t = &texts().ui;
    let lang = i3t_lang::current();
    if let Some(report) = live.and_then(|live| live.intercept.as_ref()) {
        println!("{}", lang.labelled(t.intercept, &crate::show::intercept_line(report)));
    }
    let survey = Survey::take(&mut Shell);
    let mut fields = Fields::new(2);
    match &survey.table {
        Ok(Some(table)) => {
            fields.add(t.packet_filter, (t.table_summary)(&format!("table ip {TABLE}"), &lang.list(&table.chains), table.peers.len()))
        }
        Ok(None) => {}
        Err(error) => fields.add(t.packet_filter, (t.unreadable)(&said(error))),
    }
    for rule in survey.rules.iter().flatten() {
        fields.add(t.policy_rule, rule.text.clone());
    }
    for route in survey.routes.iter().flatten().filter(|route| route.ours) {
        fields.add(t.route, route.text.clone());
    }
    if let Ok(Some(link)) = &survey.link
        && link.ours
    {
        fields.add(t.link, format!("{} {}", i3t_intercept::LINK, lang.list(&link.addresses)));
    }
    if survey.found_any() || survey.table.is_err() {
        fields.print();
    } else {
        println!("{}", t.nothing_installed);
    }
    if live.is_none() && survey.found_any() {
        println!("{}", t.leftover);
    }
}

/// 一个命令为什么失败：程序自己的原话；没有这个程序时按代码写。
fn said(error: &CommandError) -> String {
    match error {
        CommandError::Missing(program) => {
            (i3t_lang::current().codes().intercept_cause)(&InterceptCause::ProgramMissing { program: program.name().to_owned() })
        }
        CommandError::Failed { detail, .. } | CommandError::Io { detail, .. } => detail.clone(),
    }
}

/// `i3t hosts`：按最后一份 netmap 写出 `/etc/hosts` 的那一段；`apply` 时写进去，只换上次写的那一段。
pub(crate) fn hosts(state: &StateDir, apply: bool) -> anyhow::Result<()> {
    let t = &texts().ui;
    let netmap = state.netmap()?.context(t.not_joined)?;
    let names = names_of(&netmap);
    let block = hosts_block(&names);
    if !apply {
        print!("{block}");
        return Ok(());
    }
    let existing = fs::read_to_string(HOSTS).with_context(|| (t.hosts_write_failed)(HOSTS))?;
    fs::write(HOSTS, replace_hosts_block(&existing, &block)).with_context(|| (t.hosts_write_failed)(HOSTS))?;
    println!("{}", (t.hosts_written)(names.len(), HOSTS));
    Ok(())
}
