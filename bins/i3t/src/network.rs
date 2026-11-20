//! 虚拟网卡在本机的几个命令：`i3t network status`、`i3t set network|firewall off` 之后的清理、`i3t hosts`。
//!
//! 装、拆都是运行着的节点做的（`i3t_agent::vnet`）；这里只读系统里有什么，以及节点没在运行时把上次留下的规则拆掉。
//! hosts 文件里 `i3t hosts --apply` 写的那一段不随虚拟网卡拆掉：那是管理员写进去的文件，只有 `i3t hosts --remove` 去掉它。

use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use i3t_agent::state::StateDir;
use i3t_agent::status::LiveStatus;
use i3t_agent::vnet::{devices, names_of, present_devices};
use i3t_lang::table::Fields;
use i3t_model::control::{VnetCause, VnetState};
use i3t_vnet::device::Platform;
use i3t_vnet::machine::{Shell, missing_privilege};
use i3t_vnet::names::{hosts_block, remove_hosts_block, replace_hosts_block};
use i3t_vnet::{CommandError, Survey, legacy, setup};

use crate::HostsAction;
use crate::texts::texts;

/// 这台机器的 hosts 文件。
fn hosts_file() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        root.join(r"System32\drivers\etc\hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

/// `i3t set network off` 或者 `i3t set firewall off` 写好配置文件之后：节点没在运行时拆掉上次留下的规则。
/// 节点在运行时由它在停下的时候拆；它再起来时看到关着也会拆。
pub(crate) fn after_switch_off(running: bool) {
    if running || missing_privilege().is_some() {
        return;
    }
    let t = &texts().network;
    // 本节点没在运行，可这台机器上可能还跑着进别的网络的节点程序：它们的网卡在，规则不拆。
    match setup::remove_leftovers(&mut Shell, Platform::CURRENT, &present_devices()) {
        Ok(0) => {}
        Ok(count) => println!("{}", (t.leftover_removed)(count)),
        Err(_) => println!("{}", t.leftover_remove_failed),
    }
    if Platform::CURRENT == Platform::Linux
        && let Ok(found) = legacy::Leftover::find(&mut Shell)
        && !found.is_empty()
        && found.remove(&mut Shell).is_err()
    {
        println!("{}", t.leftover_remove_failed);
    }
    names_left_behind();
}

/// hosts 文件里还有 `i3t hosts --apply` 写的那一段时说一句，指出去掉它的命令。读不了这个文件就不说。
fn names_left_behind() {
    let path = hosts_file();
    if fs::read_to_string(&path).is_ok_and(|hosts| remove_hosts_block(&hosts).is_some()) {
        println!("{}", (texts().network.hosts_left)(&path.display().to_string()));
    }
}

/// `i3t network status`：运行着的节点说的状态，和系统里属于虚拟网卡的每一样东西。
pub(crate) fn status(state: &StateDir, live: Option<&LiveStatus>) {
    let t = &texts().network;
    let lang = i3t_lang::current();
    if let Some(report) = live.and_then(|live| live.vnet.as_ref()) {
        println!("{}", lang.labelled(texts().ui.vnet, &crate::show::vnet_line(report)));
    }
    let mut fields = Fields::new(2);
    let cidr = state.netmap().ok().flatten().map(|netmap| netmap.network.cidr);
    let found_devices = devices(cidr);
    for (name, address) in &found_devices {
        fields.add(t.device, format!("{name} {address}"));
    }
    let platform = Platform::CURRENT;
    let survey = Survey::take(&mut Shell, platform, i3t_vnet::device::WINDOWS_NAME);
    match &survey.tables {
        Ok(tables) => {
            for table in tables {
                fields.add(t.guard_table, format!("table ip {table}"));
            }
        }
        Err(error) => fields.add(t.guard_table, unreadable(error)),
    }
    match &survey.accept {
        Ok(rules) => {
            for rule in rules {
                fields.add(t.accept_rule, (t.accept_at)(&rule.device, rule.position));
            }
        }
        Err(error) => fields.add(t.accept_rule, unreadable(error)),
    }
    let legacy = (platform == Platform::Linux).then(|| legacy::Leftover::find(&mut Shell).ok()).flatten().filter(|found| !found.is_empty());
    if let Some(found) = legacy {
        fields.add(t.legacy, lang.list(found.items()));
    }
    // 读不了的项也要列出来：那时说"什么都没装"不一定是真的。
    let unreadable = survey.tables.is_err() || survey.accept.is_err();
    let found = !found_devices.is_empty() || survey.found_any() || legacy.is_some();
    if found || unreadable {
        fields.print();
    } else {
        println!("{}", t.nothing_installed);
    }
    if live.is_none() && survey.found_any() {
        println!("{}", t.leftover);
    }
    let installed = live.and_then(|live| live.vnet.as_ref()).is_some_and(|report| matches!(report.state, VnetState::Installed { .. }));
    if !installed {
        names_left_behind();
    }
}

/// 一个命令为什么失败：程序自己的原话；没有这个程序时按代码写。
/// 一样东西读不出来时写什么。系统里没有那个程序时只说它没有：那不是权限的事。
fn unreadable(error: &CommandError) -> String {
    match error {
        CommandError::Missing(_) => said(error),
        CommandError::Failed { .. } | CommandError::Io { .. } => (texts().network.unreadable)(&said(error)),
    }
}

fn said(error: &CommandError) -> String {
    match error {
        CommandError::Missing(program) => {
            (i3t_lang::current().codes().vnet_cause)(&VnetCause::ProgramMissing { program: program.name().to_owned() })
        }
        CommandError::Failed { detail, .. } | CommandError::Io { detail, .. } => detail.clone(),
    }
}

/// `i3t hosts`：按最后一份 netmap 写出 hosts 文件的那一段；写进去时只换上次写的那一段，去掉时只去掉那一段，别的字节不动。
pub(crate) fn hosts(state: &StateDir, action: HostsAction) -> anyhow::Result<()> {
    let t = &texts().network;
    let path = hosts_file();
    let shown = path.display().to_string();
    let command = if action == HostsAction::Remove { "i3t hosts --remove" } else { "i3t hosts --apply" };
    let failed = || (t.hosts_write_failed)(&shown, command);
    if action == HostsAction::Remove {
        let existing = fs::read_to_string(&path).with_context(failed)?;
        match remove_hosts_block(&existing) {
            Some(rest) => {
                fs::write(&path, rest).with_context(failed)?;
                println!("{}", (t.hosts_removed)(&shown));
            }
            None => println!("{}", (t.hosts_no_block)(&shown)),
        }
        return Ok(());
    }
    let netmap = state.netmap()?.context(texts().ui.not_joined)?;
    let names = names_of(&netmap);
    let block = hosts_block(&names);
    if action == HostsAction::Print {
        print!("{block}");
        return Ok(());
    }
    let existing = fs::read_to_string(&path).with_context(failed)?;
    fs::write(&path, replace_hosts_block(&existing, &block)).with_context(failed)?;
    println!("{}", (t.hosts_written)(names.len(), &shown));
    Ok(())
}
