use std::io;
use std::time::Duration;

use i3t_conn::OpenError;
use i3t_proto::flow::{DecodeError, Reply};

/// 建流这一步（开流、写头、读头）失败的原因。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FlowError {
    #[error("到对端开流失败")]
    Connect(#[from] OpenError),
    #[error("头没有写出去")]
    Write(#[source] io::Error),
    #[error("读头的时候流出错了")]
    Read(#[source] io::Error),
    #[error("头还没读完，流就结束了")]
    Truncated,
    #[error("头不合法")]
    Malformed(#[from] DecodeError),
    #[error("{0:?} 之内没有等到头")]
    Deadline(Duration),
    #[error("对端拒绝了建流请求：{:?}（{}）", .0.status, .0.detail)]
    Refused(Reply),
    #[error("对端在一条流上要的是数据报上下文，这里只接受流")]
    NotAStream,
    #[error("对端给的上下文号 {0} 已经在用")]
    ContextInUse(u32),
}
