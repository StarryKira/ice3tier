//! 搬运时每一段数据都要过的一道关：计数、限速，或者叫停这条流。
//!
//! 入口用它按凭据、按监听器、按出口记字节数，凭据的流量用完时叫停；出口用它记配额，配额用完时叫停。
//! 搬运本身不知道这些规矩，只在每读到一段数据、写出去之前问一次。

use std::future::Future;

use i3t_proto::flow::Reason;

/// 一段数据是往哪个方向搬的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// 从 TCP 读出来、要写进流。入口一侧这是客户端发出的，出口一侧这是目标发回来的。
    FromTcp,
    /// 从流读出来、要写进 TCP。入口一侧这是发给客户端的，出口一侧这是发往目标的。
    FromFlow,
}

/// 搬运时的关卡。
pub trait Meter: Send + Sync {
    /// `len` 字节刚从 `direction` 读出来，还没有写出去。
    ///
    /// 返回之前可以等（限速）：这段数据等它返回之后才写出去，读的一方也停着，反压自然传回去。
    /// 返回 `Err(reason)` 时这条流用 `reason` 异常结束，这段数据不再写出去。
    fn pass(&self, direction: Direction, len: usize) -> impl Future<Output = Result<(), Reason>> + Send;
}

/// 什么都不管的关卡。
#[derive(Debug, Clone, Copy, Default)]
pub struct Unmetered;

impl Meter for Unmetered {
    async fn pass(&self, _direction: Direction, _len: usize) -> Result<(), Reason> {
        Ok(())
    }
}

impl<M: Meter + ?Sized> Meter for &M {
    fn pass(&self, direction: Direction, len: usize) -> impl Future<Output = Result<(), Reason>> + Send {
        (**self).pass(direction, len)
    }
}

impl<A: Meter, B: Meter> Meter for (A, B) {
    /// 先过第一道再过第二道；第一道叫停了就不再问第二道。
    async fn pass(&self, direction: Direction, len: usize) -> Result<(), Reason> {
        self.0.pass(direction, len).await?;
        self.1.pass(direction, len).await
    }
}
