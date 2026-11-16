//! QUIC 的参数。数值的来历见白皮书 §6.3～§6.5 和实现前测试的结论。

use std::sync::Arc;
use std::time::Duration;

use i3t_crypto::NodeKey;
use i3t_crypto::tls::{self, TlsError};
use i3t_path::Paths;
use i3t_proto::{MAX_CONN_PACKET_LEN, PublicKey};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, EndpointConfig, IdleTimeout, MtuDiscoveryConfig, ServerConfig, TransportConfig, VarInt};

use crate::congestion::Algorithm;

/// 节点之间的连接在 TLS 握手里声明的协议名。
pub(crate) const ALPN: &[u8] = b"i3t/1";
/// TLS 要一个服务器名字。校验只看公钥，这个名字没有人看。
pub(crate) const SERVER_NAME: &str = "i3t";

/// 保活间隔。NAT 上的 UDP 映射通常 30 秒以上才回收。
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// 空闲超时。所有的路都断了的时候，连接在这段时间里等路恢复，上面的流不断。
const IDLE_TIMEOUT_MS: u32 = 30_000;
/// 对端最多能同时开多少条流。QUIC 库按这个数预先占内存（每条约 105 字节，16384 条是每条连接 13MB 左右）。
const MAX_STREAMS: u32 = 16_384;
/// 每条流的接收窗口。这是硬上限：QUIC 库给每条流的乱序缓冲设了 1024 个空洞的限制，超过就关掉整条连接，
/// 窗口 16MB 时在千兆链路上实测触发过，2MB 以内到不了。代价是单条流的吞吐不超过"窗口 ÷ 往返时间"。
const STREAM_WINDOW: u32 = 2 * 1024 * 1024;
/// 一条连接上所有的流加起来，写进去了、对端还没有确认的数据最多多少字节。到了这个数，哪条流都写不进去。
///
/// 它是这条连接在慢的路上最多压多少数据的上限，也决定了流的优先级管不管用：优先级排的是已经写进连接的数据，
/// 这个窗口满着的时候，高优先级的流也要先等别的流腾出位置才写得进去，和它们是一样的先后。
/// 一条不停写的流最多压住一个流窗口（2MB）的数据，QUIC 库默认的 10MB 五条这样的流就满了；
/// 取 64MB，同时有 32 条流压满才会到。实测（回环上每秒约一千个包的慢路，小消息的往返时间）：
/// 24 条流压满时高优先级 20～30ms、低优先级 64ms；64 条流压满、超过了这个窗口时，高优先级的也要一百到几百毫秒。
/// 量的办法在 `tests/priority.rs` 里。
const SEND_WINDOW: u64 = 64 * 1024 * 1024;
/// 一条连接上所有的流加起来，对端发过来了、本端的上层还没有读走的数据最多多少字节。
///
/// QUIC 库的默认值是不限。不限的话，netmap 里的任何一个对端都能开满流、每条流压满 2MB 却不让本端读得动
/// （比如在流的开头留一个空洞），把本端的内存吃光：16384 条流 × 2MB 是 32GB。有了这个上限，一条连接最多压住这么多。
/// 和 [`SEND_WINDOW`] 取一样的数：守规矩的对端本来就不会让没确认的数据超过它。
const RECEIVE_WINDOW: u32 = 64 * 1024 * 1024;
/// 对端最多能同时开多少条单向流。单向流用来送打洞信令和要求可靠送达的数据报，每个数据报一条，读完就关。
/// 这个数也是对端一次能压过来多少个这样的数据报：本端来不及收时，对端开不出新的流，就在那里等。
const UNI_STREAMS: u32 = 256;
/// 收到了、上层还没有取走的数据报最多攒多少字节，超过了丢最旧的。上层取得很快，这只是给突发留的余量。
const DATAGRAM_RECV_BUFFER: usize = 256 * 1024;
/// 等着发出去的数据报最多攒多少字节，超过了丢最旧的：这些是不保证送达的数据，攒久了也就过期了。
/// 要装得下一个切成片的最大的 UDP 报文（64KB 多一点）。
const DATAGRAM_SEND_BUFFER: usize = 256 * 1024;

pub(crate) fn transport() -> Arc<TransportConfig> {
    // 两条路（直连、经 relay）对连接报同一个最大包长，探测出来的大包换了路也发得出去。
    let mut mtu = MtuDiscoveryConfig::default();
    mtu.upper_bound(MAX_CONN_PACKET_LEN);

    let mut transport = TransportConfig::default();
    transport
        .max_concurrent_bidi_streams(VarInt::from_u32(MAX_STREAMS))
        .max_concurrent_uni_streams(VarInt::from_u32(UNI_STREAMS))
        .stream_receive_window(VarInt::from_u32(STREAM_WINDOW))
        .receive_window(VarInt::from_u32(RECEIVE_WINDOW))
        .send_window(SEND_WINDOW)
        .keep_alive_interval(Some(KEEP_ALIVE))
        .max_idle_timeout(Some(IdleTimeout::from(VarInt::from_u32(IDLE_TIMEOUT_MS))))
        .mtu_discovery_config(Some(mtu))
        // 默认用自己写的 BBR；库自带的那份不能用（窄带宽上多发一倍流量，带宽估计只升不降）。
        // 环境变量 I3T_CONGESTION=cubic 可以换回 Cubic，做对比测试时用。
        .congestion_controller_factory(Algorithm::from_env().factory())
        .datagram_receive_buffer_size(Some(DATAGRAM_RECV_BUFFER))
        .datagram_send_buffer_size(DATAGRAM_SEND_BUFFER);
    Arc::new(transport)
}

pub(crate) fn endpoint() -> Result<EndpointConfig, QuicConfigError> {
    let mut config = EndpointConfig::default();
    // 端口上靠"QUIC 包的首字节一定带 0x40 这一位"把连接包和路径层的帧分开，所以这一位不能被随机化。
    config.grease_quic_bit(false);
    // QUIC 库按"这个值 × 一次合并接收的包数 × 批量大小"分配接收缓冲，默认值下是一百多 MB。
    // 这个端口上最长的数据报就是带着中转头的连接包。
    let max_datagram = u16::try_from(Paths::max_datagram_len()).map_err(|_| QuicConfigError::DatagramLen)?;
    config.max_udp_payload_size(max_datagram).map_err(|_| QuicConfigError::DatagramLen)?;
    Ok(config)
}

/// 接受连接一方的配置。`is_allowed` 决定认不认对端的公钥，每次握手都会问。
pub(crate) fn server(
    key: &NodeKey,
    is_allowed: impl Fn(&PublicKey) -> bool + Send + Sync + 'static,
) -> Result<ServerConfig, QuicConfigError> {
    let crypto = QuicServerConfig::try_from(tls::server_config(key, is_allowed, ALPN)?).map_err(|_| QuicConfigError::CipherSuite)?;
    let mut server = ServerConfig::with_crypto(Arc::new(crypto));
    server.transport_config(transport());
    // 对端在 QUIC 库眼里的地址是固定的虚拟地址，不会变。
    server.migration(false);
    Ok(server)
}

/// 拨出一方的配置：只接受公钥是 `expected` 的对端。
pub(crate) fn client(key: &NodeKey, expected: PublicKey, transport: Arc<TransportConfig>) -> Result<ClientConfig, QuicConfigError> {
    let crypto = QuicClientConfig::try_from(tls::client_config(key, expected, ALPN)?).map_err(|_| QuicConfigError::CipherSuite)?;
    let mut client = ClientConfig::new(Arc::new(crypto));
    client.transport_config(transport);
    Ok(client)
}

/// QUIC 的配置建不出来。都是不该发生的事，发生了说明程序或者密钥有问题。
#[derive(Debug, thiserror::Error)]
pub enum QuicConfigError {
    #[error("建不出 TLS 配置")]
    Tls(#[from] TlsError),
    #[error("TLS 配置里没有 QUIC 需要的加密套件")]
    CipherSuite,
    #[error("最大数据报长度不在 QUIC 允许的范围里")]
    DatagramLen,
}
