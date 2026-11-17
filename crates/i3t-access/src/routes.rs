//! 流走的是哪条路：每条流结束时的那一行摘要里写它开始时和结束时这对节点的连接走的路，中间换没换过（白皮书 §13）。
//!
//! 路是连接层的事，入口和出口都不知道，所以由装配层交进来一个能问"到这个对端现在走哪条路"的东西（[`Routes`]）。
//! 不交时摘要里的路是"不知道"。
//!
//! 走独占 TCP（lane）的流不经过这对节点的共用连接，摘要里写的仍然是共用连接的路：流层看不出一条流底下是哪一种传输。

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use i3t_model::control::Route;
use i3t_proto::NodeId;

/// 到一个对端现在怎么走。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentRoute {
    pub route: Route,
    /// 上一次换路是多久之前；从来没有过路时是 `None`。
    pub since_switch: Option<Duration>,
}

/// 能回答"到 `peer` 现在走哪条路"。
pub trait Routes: Send + Sync + 'static {
    /// 到 `peer` 现在走的路；它不在配置里时是 `None`。只读手上现成的结论，不能等。
    fn current(&self, peer: NodeId) -> Option<CurrentRoute>;
}

/// 不知道路的时候用：摘要里写"不知道"。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRoutes;

impl Routes for NoRoutes {
    fn current(&self, _peer: NodeId) -> Option<CurrentRoute> {
        None
    }
}

/// 一条流开始时的路，流结束时拿来和那时的路比。
#[derive(Debug, Clone)]
pub struct RouteAtStart {
    peer: NodeId,
    route: Option<Route>,
    at: Instant,
}

impl RouteAtStart {
    /// 流开始时调一次。
    pub fn now(routes: &dyn Routes, peer: NodeId) -> Self {
        Self { peer, route: routes.current(peer).map(|current| current.route), at: Instant::now() }
    }

    /// 流结束时调：开始和结束时的路，和中间换没换过。换过又换回来的也算换过：结束时的路是在流开始之后换上的。
    #[must_use]
    pub fn ended(self, routes: &dyn Routes) -> RouteSpan {
        let now = routes.current(self.peer);
        let switched_since_start = now.as_ref().and_then(|now| now.since_switch).is_some_and(|since| since < self.at.elapsed());
        let end = now.map(|now| now.route);
        let changed = switched_since_start || end != self.route;
        RouteSpan { start: self.route, end, changed }
    }
}

/// 一条流从开始到结束这对节点的连接走的路。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpan {
    pub start: Option<Route>,
    pub end: Option<Route>,
    pub changed: bool,
}

impl RouteSpan {
    /// 写进日志的样子：`直连 203.0.113.7:20330`、`经 #3 → #5`、`不可达`，不知道是 `?`。
    #[must_use]
    pub fn start_label(&self) -> RouteLabel<'_> {
        RouteLabel(self.start.as_ref())
    }

    #[must_use]
    pub fn end_label(&self) -> RouteLabel<'_> {
        RouteLabel(self.end.as_ref())
    }
}

/// 一条路写进日志的样子。
#[derive(Debug, Clone, Copy)]
pub struct RouteLabel<'a>(Option<&'a Route>);

impl fmt::Display for RouteLabel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => f.write_str("?"),
            Some(Route::None) => f.write_str("不可达"),
            Some(Route::Direct { addr }) => write!(f, "直连 {addr}"),
            Some(Route::Relay { relays }) => {
                f.write_str("经 ")?;
                for (index, relay) in relays.iter().enumerate() {
                    let arrow = if index == 0 { "" } else { " → " };
                    write!(f, "{arrow}#{relay}")?;
                }
                Ok(())
            }
        }
    }
}

/// 装配层交进来的、各处共用的那一个。
pub type SharedRoutes = Arc<dyn Routes>;

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Fake(Mutex<Option<CurrentRoute>>);

    impl Routes for Fake {
        fn current(&self, _peer: NodeId) -> Option<CurrentRoute> {
            self.0.lock().unwrap().clone()
        }
    }

    fn peer() -> NodeId {
        NodeId::new(2).unwrap()
    }

    fn direct() -> Route {
        Route::Direct { addr: "203.0.113.7:20330".parse().unwrap() }
    }

    #[test]
    fn a_flow_that_kept_its_route_says_so() {
        let routes = Fake(Mutex::new(Some(CurrentRoute { route: direct(), since_switch: Some(Duration::from_secs(60)) })));
        let span = RouteAtStart::now(&routes, peer()).ended(&routes);
        assert_eq!(span, RouteSpan { start: Some(direct()), end: Some(direct()), changed: false });
        assert_eq!(span.start_label().to_string(), "直连 203.0.113.7:20330");
    }

    #[test]
    fn a_switch_during_the_flow_counts_even_if_it_came_back_to_the_same_route() {
        let routes = Fake(Mutex::new(Some(CurrentRoute { route: direct(), since_switch: Some(Duration::from_secs(60)) })));
        let start = RouteAtStart::now(&routes, peer());
        std::thread::sleep(Duration::from_millis(5));
        *routes.0.lock().unwrap() = Some(CurrentRoute { route: direct(), since_switch: Some(Duration::ZERO) });
        assert!(start.clone().ended(&routes).changed, "又换回来的也算换过");

        let relayed = Route::Relay { relays: vec![NodeId::new(3).unwrap(), NodeId::new(5).unwrap()] };
        *routes.0.lock().unwrap() = Some(CurrentRoute { route: relayed.clone(), since_switch: Some(Duration::ZERO) });
        let span = start.ended(&routes);
        assert_eq!((span.end.clone(), span.changed), (Some(relayed), true));
        assert_eq!(span.end_label().to_string(), "经 #3 → #5");
    }

    #[test]
    fn without_a_route_view_the_route_is_unknown() {
        let span = RouteAtStart::now(&NoRoutes, peer()).ended(&NoRoutes);
        assert_eq!(span, RouteSpan { start: None, end: None, changed: false });
        assert_eq!(span.start_label().to_string(), "?");
        assert_eq!(RouteLabel(Some(&Route::None)).to_string(), "不可达");
    }
}
