//! QUIC 的参数。数值的来历见白皮书 §6.3～§6.5 和实现前测试的结论。

use std::sync::Arc;
use std::time::Duration;

use i3t_crypto::NodeKey;
use i3t_crypto::tls::{self, TlsError};
use i3t_path::Paths;
use i3t_proto::{MAX_CONN_PACKET_LEN, PublicKey};
use quinn::congestion::CubicConfig;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, EndpointConfig, IdleTimeout, MtuDiscoveryConfig, ServerConfig, TransportConfig, VarInt};

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
/// 单向流只用来在连接里送打洞信令，同时有几条就够了。
const SIGNAL_STREAMS: u32 = 16;

pub(crate) fn transport() -> Arc<TransportConfig> {
    // 两条路（直连、经 relay）对连接报同一个最大包长，探测出来的大包换了路也发得出去。
    let mut mtu = MtuDiscoveryConfig::default();
    mtu.upper_bound(MAX_CONN_PACKET_LEN);

    let mut transport = TransportConfig::default();
    transport
        .max_concurrent_bidi_streams(VarInt::from_u32(MAX_STREAMS))
        .max_concurrent_uni_streams(VarInt::from_u32(SIGNAL_STREAMS))
        .stream_receive_window(VarInt::from_u32(STREAM_WINDOW))
        .keep_alive_interval(Some(KEEP_ALIVE))
        .max_idle_timeout(Some(IdleTimeout::from(VarInt::from_u32(IDLE_TIMEOUT_MS))))
        .mtu_discovery_config(Some(mtu))
        // 库自带的 BBR 不能用（窄带宽上多发一倍流量，带宽估计只升不降），所以现在只有 Cubic。
        .congestion_controller_factory(Arc::new(CubicConfig::default()))
        // 数据报还没有人用，关着。
        .datagram_receive_buffer_size(None);
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
