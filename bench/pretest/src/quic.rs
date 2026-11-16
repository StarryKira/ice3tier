//! QUIC 端点的配置。证书是启动时现签的，客户端不校验：这里只测传输，不测认证。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::ValueEnum;
use quinn::congestion::{BbrConfig, CubicConfig, NewRenoConfig};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use quinn::rustls::crypto::{CryptoProvider, ring};
use quinn::rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use quinn::rustls::{self, DigitallySignedStruct, SignatureScheme};
use quinn::{ClientConfig, Endpoint, EndpointConfig, MtuDiscoveryConfig, ServerConfig, TransportConfig, VarInt};
use serde::Serialize;

use crate::path::PathSocket;

const ALPN: &[u8] = b"i3t-pretest";
/// 给路径层的中转头留出余量。直连和经 relay 两条路必须能装下同样大的 QUIC 包，
/// 否则换到 relay 那条路时，按直连探测出来的大包会发不出去。
const MAX_QUIC_PACKET: u16 = 1400;
const KEEP_ALIVE: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Congestion {
    Cubic,
    Bbr,
    Newreno,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cipher {
    Aes128,
    Aes256,
    Chacha20,
}

#[derive(Clone, Copy, Debug)]
pub struct QuicOpts {
    pub congestion: Congestion,
    /// 每条流的接收窗口。连接的发送窗口取它的 4 倍。
    pub window_mb: u32,
    /// 连接多久收不到任何包就判定断开；0 表示不设。
    pub idle_secs: u32,
    pub segmentation_offload: bool,
    pub cipher: Cipher,
    /// 本端愿意接收的最大 UDP 包长；0 表示用库的默认值（约 64KB）。
    /// QUIC 库按"这个值 × 一次合并接收的包数 × 批量大小"预分配接收缓冲，默认值下是一百多 MB。
    pub max_udp_payload: u16,
    /// 对端最多能同时开多少条流。QUIC 库会为每条允许的流预先建好状态，所以这个数直接决定每条连接的内存。
    pub max_streams: u32,
}

impl QuicOpts {
    fn transport(self) -> anyhow::Result<Arc<TransportConfig>> {
        let window = u64::from(self.window_mb) * 1024 * 1024;
        let mut mtu = MtuDiscoveryConfig::default();
        mtu.upper_bound(MAX_QUIC_PACKET);

        let mut transport = TransportConfig::default();
        transport
            .max_concurrent_bidi_streams(VarInt::from_u32(self.max_streams))
            .stream_receive_window(VarInt::from_u64(window).context("流的接收窗口太大")?)
            .send_window(window * 4)
            .keep_alive_interval(Some(KEEP_ALIVE))
            .enable_segmentation_offload(self.segmentation_offload)
            .mtu_discovery_config(Some(mtu));
        let idle = match self.idle_secs {
            0 => None,
            secs => Some(Duration::from_secs(u64::from(secs)).try_into().context("空闲超时太长")?),
        };
        transport.max_idle_timeout(idle);
        match self.congestion {
            Congestion::Cubic => transport.congestion_controller_factory(Arc::new(CubicConfig::default())),
            Congestion::Bbr => transport.congestion_controller_factory(Arc::new(BbrConfig::default())),
            Congestion::Newreno => transport.congestion_controller_factory(Arc::new(NewRenoConfig::default())),
        };
        Ok(Arc::new(transport))
    }

    /// 只留下选中的加密套件。AES-128-GCM 总要留着：QUIC 的初始包固定用它。
    fn crypto_provider(self) -> Arc<CryptoProvider> {
        let preferred = match self.cipher {
            Cipher::Aes128 => None,
            Cipher::Aes256 => Some(ring::cipher_suite::TLS13_AES_256_GCM_SHA384),
            Cipher::Chacha20 => Some(ring::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256),
        };
        let mut provider = ring::default_provider();
        provider.cipher_suites = preferred.into_iter().chain([ring::cipher_suite::TLS13_AES_128_GCM_SHA256]).collect();
        Arc::new(provider)
    }
}

fn endpoint_config(opts: QuicOpts) -> anyhow::Result<EndpointConfig> {
    let mut config = EndpointConfig::default();
    // 路径层靠"QUIC 包首字节一定带 0x40"来区分自己的帧，所以这一位不能被随机化。
    config.grease_quic_bit(false);
    if opts.max_udp_payload > 0 {
        config.max_udp_payload_size(opts.max_udp_payload).context("最大 UDP 包长不合法")?;
    }
    Ok(config)
}

pub fn server_endpoint(socket: Arc<PathSocket>, opts: QuicOpts) -> anyhow::Result<Endpoint> {
    let certified = rcgen::generate_simple_self_signed(vec!["pretest".to_owned()])?;
    let cert = CertificateDer::from(certified.cert);
    let key = PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let mut tls = rustls::ServerConfig::builder_with_provider(opts.crypto_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];

    let mut server = ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    server.transport_config(opts.transport()?);
    let endpoint = Endpoint::new_with_abstract_socket(endpoint_config(opts)?, Some(server), socket, Arc::new(quinn::TokioRuntime))?;
    Ok(endpoint)
}

pub fn client_endpoint(socket: Arc<PathSocket>, opts: QuicOpts) -> anyhow::Result<Endpoint> {
    let provider = opts.crypto_provider();
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];

    let mut client = ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    client.transport_config(opts.transport()?);
    let mut endpoint = Endpoint::new_with_abstract_socket(endpoint_config(opts)?, None, socket, Arc::new(quinn::TokioRuntime))?;
    endpoint.set_default_client_config(client);
    Ok(endpoint)
}

/// 测试工具专用：接受任何证书，只验握手签名。产品代码里要把对端公钥和 netmap 比对。
#[derive(Debug)]
struct AcceptAnyCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// 一条 QUIC 连接结束时的统计。
#[derive(Debug, Serialize)]
pub struct QuicStats {
    rtt_ms: f64,
    cwnd: u64,
    mtu: u16,
    sent_packets: u64,
    lost_packets: u64,
    /// 丢包占发包的比例，近似重传比例。
    loss_ratio: f64,
    congestion_events: u64,
    black_holes: u64,
    /// 发出去的 QUIC 包的总字节数，不含中转头和 UDP / IP 头。
    udp_tx_bytes: u64,
    udp_rx_bytes: u64,
}

impl From<quinn::ConnectionStats> for QuicStats {
    fn from(stats: quinn::ConnectionStats) -> Self {
        let path = stats.path;
        Self {
            rtt_ms: path.rtt.as_secs_f64() * 1000.0,
            cwnd: path.cwnd,
            mtu: path.current_mtu,
            sent_packets: path.sent_packets,
            lost_packets: path.lost_packets,
            loss_ratio: if path.sent_packets == 0 { 0.0 } else { path.lost_packets as f64 / path.sent_packets as f64 },
            congestion_events: path.congestion_events,
            black_holes: path.black_holes_detected,
            udp_tx_bytes: stats.udp_tx.bytes,
            udp_rx_bytes: stats.udp_rx.bytes,
        }
    }
}
