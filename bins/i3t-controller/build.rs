//! 把构建好的 WebUI（`web/dist`）打进 controller 的可执行文件。
//!
//! 这里只是把 `web/dist` 里的文件列成一张表（`OUT_DIR/webui_assets.rs`），内容由 `include_bytes!` 在编译时读进来。
//! `web/dist` 不存在时表是空的，controller 这时回一个"这次构建没有带 WebUI"的说明页。所以构建 controller
//! 不需要 Node.js：想带上 WebUI 的人先在 `web/` 里构建，再编译 controller。
//!
//! 什么时候重新跑：
//!
//! - `web/dist` 存在：盯着它，里面的文件变了就重新打包。
//! - `web/dist` 不存在：盯着 `web/`，这样之后构建出 `dist` 时能发现。不能直接盯一个不存在的路径，
//!   cargo 会因此每次都重新编译这个包。
//! - 连 `web/` 都没有（只拿了 Rust 部分的源码）：只在这个脚本自己变了的时候跑。

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn main() -> io::Result<()> {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo 在运行构建脚本时一定会设 CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo 在运行构建脚本时一定会设 OUT_DIR"));
    let web = manifest_dir.join("../../web");
    let dist = web.join("dist");

    println!("cargo:rerun-if-changed=build.rs");
    let mut files = Vec::new();
    if dist.join("index.html").is_file() {
        println!("cargo:rerun-if-changed={}", dist.display());
        collect(&dist, &mut files)?;
    } else if web.is_dir() {
        println!("cargo:rerun-if-changed={}", web.display());
    }
    files.sort();

    let mut table = String::from("&[\n");
    for file in &files {
        let relative = file.strip_prefix(&dist).expect("这些文件都是从 dist 目录里列出来的");
        // 网址里的路径一律用斜杠，不管构建的机器用什么分隔符。
        let url_path = relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        let source = file.canonicalize()?;
        // 写进 String 不会失败。
        let _ = writeln!(table, "    ::i3t_controller::webui::Asset {{ path: {url_path:?}, bytes: include_bytes!({source:?}) }},");
    }
    table.push_str("]\n");
    fs::write(out_dir.join("webui_assets.rs"), table)
}

/// 递归列出 `dir` 下面的全部文件。
fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}
