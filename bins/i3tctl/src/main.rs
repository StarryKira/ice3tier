//! ice3tier 的管理命令行。
//!
//! 它只是管理 API 的一个客户端：每个子命令对应一两个请求。默认把结果打印成表格，`--json` 时原样输出 API 的 JSON。
//! 终端上的文字跟着系统的语言（见 `i3t_lang`），`--json` 的输出不翻译。

mod cli;
mod client;
mod commands;
mod render;
mod texts;

use std::process::ExitCode;

use i3t_lang::Lang;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let lang = i3t_lang::set(Lang::detect());
    let cli = i3t_lang::cli::parse(cli::command(lang), lang);
    match commands::run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("i3tctl: {}", lang.chain(err.chain()));
            ExitCode::FAILURE
        }
    }
}
