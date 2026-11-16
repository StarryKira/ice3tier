//! ice3tier 的管理命令行。
//!
//! 它只是管理 API 的一个客户端：每个子命令对应一两个请求。默认把结果打印成表格，`--json` 时原样输出 API 的 JSON。

mod cli;
mod client;
mod commands;
mod render;
mod table;

use std::process::ExitCode;

use clap::Parser;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match commands::run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("i3tctl: {err:#}");
            ExitCode::FAILURE
        }
    }
}
