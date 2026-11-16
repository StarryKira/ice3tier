//! `i3t-controller` 的文字目录：帮助、管理令牌、启动时的错误，两种语言各一份。少写一项编译不过。

use i3t_controller::store::StoreError;
use i3t_lang::Lang;

#[derive(Debug)]
pub struct Texts {
    pub about: &'static str,
    pub data_dir: &'static str,
    pub public_addr: &'static str,
    pub listen: &'static str,
    pub admin_listen: &'static str,
    pub static_dir: &'static str,
    pub reset_admin_token: &'static str,
    pub dir: &'static str,
    pub host_port: &'static str,
    pub address: &'static str,
    pub start_runtime: &'static str,
    pub no_index_html: fn(&str) -> String,
    pub open_data_dir: fn(&str) -> String,
    pub bad_version: &'static str,
    pub reset_token: &'static str,
    pub admin_token: &'static str,
    pub admin_token_note: &'static str,
    pub bind: &'static str,
    pub run_failed: &'static str,
    pub bad_public_addr: fn(&str) -> String,
    pub io: fn(&str) -> String,
    pub key_file: fn(&str) -> String,
    pub key_generation: &'static str,
    pub database: &'static str,
    pub newer_database: fn(u32, u32) -> String,
    pub tls: &'static str,
    pub bind_address: fn(&str) -> String,
    pub socket: &'static str,
    pub random: &'static str,
    pub internal: &'static str,
}

pub fn texts() -> &'static Texts {
    texts_in(i3t_lang::current())
}

pub const fn texts_in(lang: Lang) -> &'static Texts {
    match lang {
        Lang::ZhCn => &ZH,
        Lang::English => &EN,
    }
}

static ZH: Texts = Texts {
    about: "ice3tier controller：节点协议、管理 API 和 WebUI",
    data_dir: "数据目录：数据库和 controller 的密钥。第一次启动时创建",
    public_addr: "节点连 controller 的地址，写进每个注册令牌。主机可以是域名",
    listen: "节点协议的监听地址（TCP；同一个端口号的 UDP 回答地址查询）",
    admin_listen: "管理 API 和 WebUI 的监听地址。远程管理用 SSH 端口转发",
    static_dir: "从这个目录取 WebUI，不用程序自带的",
    reset_admin_token: "重新生成管理令牌后退出，原来的作废。要在 controller 停着时执行",
    dir: "目录",
    host_port: "主机:端口",
    address: "地址",
    start_runtime: "启动运行时",
    no_index_html: |dir| format!("静态文件目录 {dir} 里没有 index.html"),
    open_data_dir: |dir| format!("打开数据目录 {dir}"),
    bad_version: "这个程序的版本号不合法",
    reset_token: "重新生成管理令牌",
    admin_token: "管理令牌（只显示这一次）：",
    admin_token_note: "i3tctl 和 WebUI 用它登录。丢了的话停掉 controller，用 --reset-admin-token 重新生成。",
    bind: "绑定端口",
    run_failed: "controller 运行出错",
    bad_public_addr: |address| format!("--public-addr 要写成 主机:端口，如 ctl.example.com:20331，不能是 {address:?}"),
    io: |path| format!("读写 {path} 失败"),
    key_file: |path| format!("controller 的密钥文件 {path} 读不懂"),
    key_generation: "生成 controller 的密钥失败",
    database: "数据库出错",
    newer_database: |found, supported| format!("数据库是更新的版本写的（第 {found} 版，这个程序认到第 {supported} 版）"),
    tls: "准备 TLS 失败",
    bind_address: |address| format!("绑定 {address} 失败"),
    socket: "网络端口出错",
    random: "系统的随机数源不可用",
    internal: "内部出错",
};

static EN: Texts = Texts {
    about: "ice3tier controller: node protocol, admin API and WebUI",
    data_dir: "Data directory: database and the controller key. Created on first start",
    public_addr: "Address nodes use to reach the controller, written into every enrollment token. The host may be a domain name",
    listen: "Node protocol listen address (TCP; UDP on the same port answers address queries)",
    admin_listen: "Admin API and WebUI listen address. For remote admin use SSH port forwarding",
    static_dir: "Serve the WebUI from this directory instead of the built-in one",
    reset_admin_token: "Issue a new admin token and exit; the old one stops working. Run while the controller is stopped",
    dir: "DIR",
    host_port: "HOST:PORT",
    address: "ADDR",
    start_runtime: "Starting the runtime",
    no_index_html: |dir| format!("No index.html in the static directory {dir}"),
    open_data_dir: |dir| format!("Opening the data directory {dir}"),
    bad_version: "This program's version is invalid",
    reset_token: "Issuing a new admin token",
    admin_token: "Admin token (shown only once):",
    admin_token_note: "i3tctl and the WebUI sign in with it. If lost, stop the controller and run it with --reset-admin-token.",
    bind: "Binding ports",
    run_failed: "Controller failed",
    bad_public_addr: |address| format!("--public-addr takes HOST:PORT, e.g. ctl.example.com:20331, not {address:?}"),
    io: |path| format!("Couldn't read or write {path}"),
    key_file: |path| format!("Controller key file {path} unreadable"),
    key_generation: "Generating the controller key failed",
    database: "Database error",
    newer_database: |found, supported| {
        format!("The database was written by a newer version (schema {found}; this program knows up to {supported})")
    },
    tls: "Preparing TLS failed",
    bind_address: |address| format!("Couldn't bind {address}"),
    socket: "Network socket error",
    random: "System random source unavailable",
    internal: "Internal error",
};

/// 启动时的错误写成一行：这个程序自己加的说明按语言写，controller 库的错误按种类换成这种语言，系统的原话照抄。
pub fn describe(error: &anyhow::Error) -> String {
    let t = texts();
    i3t_lang::current().chain(error.chain().map(|cause| {
        if let Some(error) = cause.downcast_ref::<i3t_controller::Error>() {
            return controller_error(error, t);
        }
        if let Some(store) = cause.downcast_ref::<StoreError>() {
            return store_error(store, t);
        }
        cause.to_string()
    }))
}

fn controller_error(error: &i3t_controller::Error, t: &Texts) -> String {
    use i3t_controller::Error;
    match error {
        Error::Io { path, .. } => (t.io)(&path.display().to_string()),
        Error::Key { path, .. } => (t.key_file)(&path.display().to_string()),
        Error::KeyGeneration(_) => t.key_generation.to_owned(),
        Error::Store(store) => store_error(store, t),
        Error::Tls(_) => t.tls.to_owned(),
        Error::Bind { address, .. } => (t.bind_address)(&address.to_string()),
        Error::Socket(_) => t.socket.to_owned(),
        Error::Random => t.random.to_owned(),
        // 启动时不会遇到的种类（管理请求的错误）：只说内部出错，细节在日志里。
        _ => t.internal.to_owned(),
    }
}

fn store_error(error: &StoreError, t: &Texts) -> String {
    match error {
        StoreError::Sqlite(_) => t.database.to_owned(),
        StoreError::NewerSchema { found, supported } => (t.newer_database)(*found, *supported),
        // 以后的版本新加的种类：说是数据库出错，细节在后面的来源里。
        _ => t.database.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use i3t_lang::cli::{CLAP_ENGLISH, all_help, has_chinese, try_parse_from};

    use super::*;

    #[test]
    fn the_help_is_written_in_each_language_without_the_other() {
        for lang in Lang::ALL {
            i3t_lang::cli::localize(crate::command(lang), lang).debug_assert();
        }
        for (path, help) in all_help(crate::command(Lang::English), Lang::English) {
            assert!(!has_chinese(&help), "{path}：\n{help}");
        }
        for (path, help) in all_help(crate::command(Lang::ZhCn), Lang::ZhCn) {
            for english in CLAP_ENGLISH {
                assert!(!help.contains(english), "{path} 的中文帮助里有 {english}：\n{help}");
            }
        }
        let args = ["i3t-controller", "--data-dir", "/tmp/x", "--public-addr", "ctl:20331"].map(OsString::from);
        assert!(try_parse_from::<crate::Args>(crate::command(Lang::English), Lang::English, args).is_ok());
    }

    #[test]
    fn startup_errors_from_the_library_are_written_in_the_language() {
        let error = anyhow::Error::new(i3t_controller::Error::Bind {
            address: "127.0.0.1:20332".parse().unwrap(),
            source: std::io::Error::from(std::io::ErrorKind::AddrInUse),
        })
        .context((EN.open_data_dir)("/var/lib/i3t-controller"));
        let said = describe(&error);
        if i3t_lang::current() == Lang::English {
            assert!(!has_chinese(&said), "{said}");
        }
        assert!(said.contains("127.0.0.1:20332"), "{said}");
    }
}
