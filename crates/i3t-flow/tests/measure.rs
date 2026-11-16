//! 测量流：发起方量上行和下行，接受方按自己的上限截断；不提供测量服务的处理者一律拒绝。

#![allow(clippy::unwrap_used, reason = "整个文件都是测试代码，出错就该直接失败")]

use std::sync::Arc;
use std::time::Duration;

use i3t_conn::{RecvStream, SendStream};
use i3t_flow::measure::{MeasureLimits, MeasureRequest, measure};
use i3t_flow::memory::MemoryMesh;
use i3t_flow::{FlowError, Handler, Request};
use i3t_proto::NodeId;
use i3t_proto::flow::{MeasureEnd, MeasureMode, OpenMeasure, Reply, Status};
use tokio::sync::Mutex;

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

/// 一个测量服务：编号是 7 的测试按 `limits` 接受，别的拒绝。记下每条流量到的结果。
struct Service {
    limits: MeasureLimits,
    served: Mutex<Vec<(MeasureMode, u64, MeasureEnd)>>,
}

impl Handler for Service {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        request.refuse(&Reply::new(Status::NotEnabled, "只测量")).await;
    }

    async fn handle_measure<S: SendStream, R: RecvStream>(&self, request: MeasureRequest<S, R>) {
        if request.open().test != 7 {
            return request.refuse(&Reply::new(Status::NotEnabled, "不认识这个测试")).await;
        }
        let mode = request.open().mode;
        let served = request.serve(self.limits).await;
        self.served.lock().await.push((mode, served.bytes, served.end));
    }
}

/// 什么都不提供的处理者：测量流走默认的拒绝。
struct Plain;

impl Handler for Plain {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        request.refuse(&Reply::new(Status::NotEnabled, "没有")).await;
    }
}

fn open(mode: MeasureMode, bytes: u64, millis: u32) -> OpenMeasure {
    OpenMeasure { test: 7, mode, path: None, bytes, millis }
}

#[tokio::test]
async fn both_directions_are_measured_and_stop_at_the_byte_limit() {
    let mesh = MemoryMesh::new();
    let (connector, _) = mesh.join(node(1));
    let (_, acceptor) = mesh.join(node(2));
    let service =
        Arc::new(Service { limits: MeasureLimits { bytes: 1 << 20, duration: Duration::from_secs(5) }, served: Mutex::default() });
    let serving = tokio::spawn({
        let service = Arc::clone(&service);
        async move { i3t_flow::serve(&acceptor, service).await }
    });

    let up = measure(&connector, node(2), &open(MeasureMode::Sink, 300_000, 5000)).await.unwrap();
    assert_eq!((up.bytes, up.end), (300_000, MeasureEnd::Bytes), "接受方收满了发起方要的量：{up:?}");
    let down = measure(&connector, node(2), &open(MeasureMode::Source, 400_000, 5000)).await.unwrap();
    assert_eq!((down.bytes, down.end), (400_000, MeasureEnd::Bytes));
    assert!(down.bits_per_second().is_none_or(|rate| rate > 0.0));

    // 发起方要的比接受方肯给的多：接受方按自己的上限截断，两个方向都是。
    let greedy_up = measure(&connector, node(2), &open(MeasureMode::Sink, 50 << 20, 5000)).await.unwrap();
    assert_eq!(greedy_up.end, MeasureEnd::Bytes);
    assert!((1 << 20..(1 << 20) + 64 * 1024).contains(&greedy_up.bytes), "{greedy_up:?}");
    let greedy_down = measure(&connector, node(2), &open(MeasureMode::Source, 50 << 20, 5000)).await.unwrap();
    assert_eq!(greedy_down.bytes, 1 << 20);
    let served: Vec<(MeasureMode, MeasureEnd)> = service.served.lock().await.iter().map(|(mode, _, end)| (*mode, *end)).collect();
    assert_eq!(
        served,
        [
            (MeasureMode::Sink, MeasureEnd::Bytes),
            (MeasureMode::Source, MeasureEnd::Bytes),
            (MeasureMode::Sink, MeasureEnd::Bytes),
            (MeasureMode::Source, MeasureEnd::Bytes),
        ]
    );
    serving.abort();
}

#[tokio::test]
async fn the_service_stops_at_its_time_limit_whatever_was_asked() {
    let mesh = MemoryMesh::new();
    let (connector, _) = mesh.join(node(1));
    let (_, acceptor) = mesh.join(node(2));
    // 内存里的连接很快：给一个很小的时长上限、很大的字节上限，看它在时长到了的时候停。
    let limits = MeasureLimits { bytes: u64::MAX, duration: Duration::from_millis(300) };
    let service = Arc::new(Service { limits, served: Mutex::default() });
    let serving = tokio::spawn({
        let service = Arc::clone(&service);
        async move { i3t_flow::serve(&acceptor, service).await }
    });
    let started = tokio::time::Instant::now();
    let down = measure(&connector, node(2), &open(MeasureMode::Source, u64::MAX, 60_000)).await.unwrap();
    assert_eq!(down.end, MeasureEnd::Time, "{down:?}");
    assert!(started.elapsed() < Duration::from_secs(3), "接受方 300ms 就停了，发起方要了一分钟也没用");
    let up = measure(&connector, node(2), &open(MeasureMode::Sink, u64::MAX, 60_000)).await.unwrap();
    assert_eq!(up.end, MeasureEnd::Time, "{up:?}");
    assert!(up.elapsed <= Duration::from_millis(400), "{up:?}");
    serving.abort();
}

#[tokio::test]
async fn an_unknown_test_and_a_node_without_the_service_are_refused() {
    let mesh = MemoryMesh::new();
    let (connector, _) = mesh.join(node(1));
    let (_, acceptor) = mesh.join(node(2));
    let (_, plain) = mesh.join(node(3));
    let service =
        Arc::new(Service { limits: MeasureLimits { bytes: 1 << 20, duration: Duration::from_secs(5) }, served: Mutex::default() });
    let serving = tokio::spawn(async move { i3t_flow::serve(&acceptor, service).await });
    let plain_serving = tokio::spawn(async move { i3t_flow::serve(&plain, Arc::new(Plain)).await });

    let other_test = OpenMeasure { test: 8, ..open(MeasureMode::Source, 1000, 1000) };
    let refused = measure(&connector, node(2), &other_test).await.unwrap_err();
    assert!(matches!(refused, FlowError::Refused(reply) if reply.status == Status::NotEnabled));
    let refused = measure(&connector, node(3), &open(MeasureMode::Source, 1000, 1000)).await.unwrap_err();
    assert!(matches!(refused, FlowError::Refused(reply) if reply.detail.contains("测量服务")));
    serving.abort();
    plain_serving.abort();
}
