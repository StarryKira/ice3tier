//! 透明访问在本机的几个命令（只在 Linux 上）：`i3t set intercept` 之后的检查和清理、`i3t intercept status`、`i3t hosts`。
//!
//! `/etc/hosts` 里 `i3t hosts --apply` 写的那一段不随透明访问拆掉：那是管理员写进去的文件，只有 `i3t hosts --remove` 去掉它。
//! 透明访问关着、那一段还在时，`i3t intercept status` 和 `i3t set intercept off` 提醒一句：没有拦截时这些名字指向的地址走默认路由，连不上。
//!
//! 装、拆都是运行着的节点做的（`i3t_agent` 的 `intercept` 模块）；这里只读系统里有什么，以及节点没在运行时
//! 把上次留下的东西拆掉。

use std::fs;

use anyhow::Context;
use i3t_agent::intercept::names_of;
use i3t_agent::state::StateDir;
use i3t_agent::status::LiveStatus;
use i3t_intercept::machine::Shell;
use i3t_intercept::names::{hosts_block, remove_hosts_block, replace_hosts_block};
use i3t_intercept::{Capability, CommandError, Program, Survey, TABLE, setup};
use i3t_lang::table::Fields;
use i3t_model::control::{InterceptCause, InterceptState};

use crate::HostsAction;
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
    remove_leftover(running);
    names_left_behind();
}

/// 节点没在运行时拆掉上次留下的。节点在运行时由它在停下的时候拆；它停下之后再起来，看到关着也会拆掉留下的。
fn remove_leftover(running: bool) {
    if running || Capability::missing().contains(&Capability::NetAdmin) || Shell::locate(Program::Nft).is_none() {
        return;
    }
    if !Survey::take(&mut Shell).found_any() {
        return;
    }
    let t = &texts().ui;
    match setup::remove(&mut Shell) {
        Ok(()) => println!("{}", t.intercept_removed),
        Err(_) => println!("{}", t.intercept_remove_failed),
    }
}

/// `/etc/hosts` 里还有 `i3t hosts --apply` 写的那一段时说一句，指出去掉它的命令。读不了这个文件就不说。
fn names_left_behind() {
    if fs::read_to_string(HOSTS).is_ok_and(|hosts| remove_hosts_block(&hosts).is_some()) {
        println!("{}", (texts().ui.hosts_left)(HOSTS));
    }
}

/// `i3t intercept status`：运行着的节点说的状态，和系统里属于透明访问的每一样东西。
/// `configured` 是配置文件里打开了没有，节点没在运行时按它算透明访问关没关。
pub(crate) fn status(live: Option<&LiveStatus>, configured: bool) {
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
    let off = match live.and_then(|live| live.intercept.as_ref()) {
        Some(report) => report.state == InterceptState::Off,
        None => !configured,
    };
    if off {
        names_left_behind();
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

/// `i3t hosts`：按最后一份 netmap 写出 `/etc/hosts` 的那一段；写进去时只换上次写的那一段，去掉时只去掉那一段，别的字节不动。
pub(crate) fn hosts(state: &StateDir, action: HostsAction) -> anyhow::Result<()> {
    let t = &texts().ui;
    let command = if action == HostsAction::Remove { "i3t hosts --remove" } else { "i3t hosts --apply" };
    let failed = || (t.hosts_write_failed)(HOSTS, command);
    if action == HostsAction::Remove {
        let existing = fs::read_to_string(HOSTS).with_context(failed)?;
        match remove_hosts_block(&existing) {
            Some(rest) => {
                fs::write(HOSTS, rest).with_context(failed)?;
                println!("{}", (t.hosts_removed)(HOSTS));
            }
            None => println!("{}", (t.hosts_no_block)(HOSTS)),
        }
        return Ok(());
    }
    let netmap = state.netmap()?.context(t.not_joined)?;
    let names = names_of(&netmap);
    let block = hosts_block(&names);
    if action == HostsAction::Print {
        print!("{block}");
        return Ok(());
    }
    let existing = fs::read_to_string(HOSTS).with_context(failed)?;
    fs::write(HOSTS, replace_hosts_block(&existing, &block)).with_context(failed)?;
    println!("{}", (t.hosts_written)(names.len(), HOSTS));
    Ok(())
}
