//! 按公钥互相认证的 TLS 1.3 配置。
//!
//! 每个节点用自己的节点密钥自签一张证书。校验对端时不看证书链、有效期和名字，只做两件事：
//!
//! 1. 取出证书里的公钥，和期望的公钥比对（拨出的一方），或者问调用方认不认（接受的一方）；
//! 2. 验证握手签名，证明对端确实持有这把公钥对应的私钥。
//!
//! 所以证书只是公钥的容器，信任来自 netmap 里登记的公钥。

use std::sync::Arc;

use i3t_proto::PublicKey;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, ring};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme};

use crate::key::NodeKey;

/// Ed25519 公钥的 SubjectPublicKeyInfo 编码里，公钥本身之前的固定部分（RFC 8410）。
const ED25519_SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

/// 证书里的名字。校验时不看它，只是证书格式要求有一个。
const CERT_NAME: &str = "i3t";

/// 拨出一方的配置：只接受公钥是 `expected` 的对端，并向对端出示自己的证书。
///
/// # Errors
///
/// 用节点密钥签证书失败。
pub fn client_config(key: &NodeKey, expected: PublicKey, alpn: &[u8]) -> Result<ClientConfig, TlsError> {
    let provider = provider();
    let verifier = PinnedServer { expected, algorithms: provider.signature_verification_algorithms };
    let (certificate, private_key) = identity(key)?;
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(vec![certificate], private_key)?;
    config.alpn_protocols = vec![alpn.to_vec()];
    Ok(config)
}

/// 接受一方的配置：对端必须出示证书，`is_allowed` 决定认不认它的公钥。
///
/// `is_allowed` 在每次握手时调用，所以它可以去查会变的东西（例如当前的 netmap）。
/// 握手完成之后用 [`peer_key`] 取出对端的公钥，就知道连进来的是谁。
///
/// # Errors
///
/// 用节点密钥签证书失败。
pub fn server_config(
    key: &NodeKey,
    is_allowed: impl Fn(&PublicKey) -> bool + Send + Sync + 'static,
    alpn: &[u8],
) -> Result<ServerConfig, TlsError> {
    let provider = provider();
    let verifier = AllowedClients { is_allowed: Box::new(is_allowed), algorithms: provider.signature_verification_algorithms };
    let (certificate, private_key) = identity(key)?;
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(verifier))
        .with_single_cert(vec![certificate], private_key)?;
    config.alpn_protocols = vec![alpn.to_vec()];
    Ok(config)
}

/// 取出对端证书里的 Ed25519 公钥。`end_entity` 是握手时对端出示的第一张证书。
///
/// # Errors
///
/// 证书解析不了，或者里面的公钥不是 Ed25519。
pub fn peer_key(end_entity: &CertificateDer<'_>) -> Result<PublicKey, TlsError> {
    let certificate = ParsedCertificate::try_from(end_entity).map_err(|_| TlsError::BadCertificate)?;
    let spki = certificate.subject_public_key_info();
    let key = spki.strip_prefix(&ED25519_SPKI_PREFIX).ok_or(TlsError::NotEd25519)?;
    let key: [u8; PublicKey::LEN] = key.try_into().map_err(|_| TlsError::NotEd25519)?;
    Ok(PublicKey::from_bytes(key))
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// 用节点密钥自签一张证书，连同私钥一起交给 TLS。
fn identity(key: &NodeKey) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), TlsError> {
    let pkcs8 = PrivatePkcs8KeyDer::from(key.pkcs8_der().to_vec());
    let signer = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8, &rcgen::PKCS_ED25519)?;
    let certificate = rcgen::CertificateParams::new(vec![CERT_NAME.to_owned()])?.self_signed(&signer)?;
    Ok((certificate.der().clone(), PrivateKeyDer::Pkcs8(pkcs8)))
}

fn key_of(end_entity: &CertificateDer<'_>) -> Result<PublicKey, rustls::Error> {
    peer_key(end_entity).map_err(|_| rustls::Error::InvalidCertificate(CertificateError::BadEncoding))
}

fn rejected() -> rustls::Error {
    rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
}

/// 握手签名只接受 Ed25519：对端的密钥只可能是这一种。
fn schemes() -> Vec<SignatureScheme> {
    vec![SignatureScheme::ED25519]
}

#[derive(Debug)]
struct PinnedServer {
    expected: PublicKey,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if key_of(end_entity)? == self.expected { Ok(ServerCertVerified::assertion()) } else { Err(rejected()) }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        schemes()
    }
}

struct AllowedClients {
    is_allowed: Box<dyn Fn(&PublicKey) -> bool + Send + Sync>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl std::fmt::Debug for AllowedClients {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AllowedClients").finish_non_exhaustive()
    }
}

impl ClientCertVerifier for AllowedClients {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if (self.is_allowed)(&key_of(end_entity)?) { Ok(ClientCertVerified::assertion()) } else { Err(rejected()) }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        schemes()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("用节点密钥签证书失败：{0}")]
    Certificate(#[from] rcgen::Error),
    #[error("TLS 配置不被接受：{0}")]
    Config(#[from] rustls::Error),
    #[error("对端的证书解析不了")]
    BadCertificate,
    #[error("对端证书里的公钥不是 Ed25519")]
    NotEd25519,
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use rustls::{ClientConnection, ServerConnection};

    use super::*;

    const ALPN: &[u8] = b"test/1";

    /// 在内存里把一次握手跑完，返回两边各自看到的结果。
    fn handshake(
        client: ClientConfig,
        server: ServerConfig,
    ) -> (Result<ClientConnection, rustls::Error>, Result<ServerConnection, rustls::Error>) {
        let name = ServerName::try_from(CERT_NAME).unwrap();
        let mut client = ClientConnection::new(Arc::new(client), name).unwrap();
        let mut server = ServerConnection::new(Arc::new(server)).unwrap();
        let (mut client_error, mut server_error) = (None, None);
        for _ in 0..10 {
            let mut to_server = Vec::new();
            while client.wants_write() {
                client.write_tls(&mut to_server).unwrap();
            }
            if !to_server.is_empty() && server_error.is_none() {
                server.read_tls(&mut to_server.as_slice()).unwrap();
                server_error = server.process_new_packets().err();
            }
            let mut to_client = Vec::new();
            while server.wants_write() {
                server.write_tls(&mut to_client).unwrap();
            }
            if !to_client.is_empty() && client_error.is_none() {
                client.read_tls(&mut to_client.as_slice()).unwrap();
                client_error = client.process_new_packets().err();
            }
            if !client.is_handshaking() && !server.is_handshaking() {
                break;
            }
        }
        (client_error.map_or(Ok(client), Err), server_error.map_or(Ok(server), Err))
    }

    #[test]
    fn both_sides_learn_the_peer_key_and_can_exchange_data() {
        let (alice, bob) = (NodeKey::generate().unwrap(), NodeKey::generate().unwrap());
        let alice_public = alice.public();
        let client = client_config(&alice, bob.public(), ALPN).unwrap();
        let server = server_config(&bob, move |key| *key == alice_public, ALPN).unwrap();
        let (client, server) = handshake(client, server);
        let (mut client, mut server) = (client.unwrap(), server.unwrap());

        assert_eq!(peer_key(&client.peer_certificates().unwrap()[0]).unwrap(), bob.public());
        assert_eq!(peer_key(&server.peer_certificates().unwrap()[0]).unwrap(), alice.public());
        assert_eq!(client.alpn_protocol(), Some(ALPN));
        assert_eq!(client.protocol_version(), Some(rustls::ProtocolVersion::TLSv1_3));

        client.writer().write_all(b"ping").unwrap();
        let mut wire = Vec::new();
        client.write_tls(&mut wire).unwrap();
        server.read_tls(&mut wire.as_slice()).unwrap();
        server.process_new_packets().unwrap();
        let mut received = [0; 4];
        server.reader().read_exact(&mut received).unwrap();
        assert_eq!(&received, b"ping");
    }

    #[test]
    fn client_refuses_a_server_with_another_key() {
        let (alice, bob, mallory) = (NodeKey::generate().unwrap(), NodeKey::generate().unwrap(), NodeKey::generate().unwrap());
        let client = client_config(&alice, bob.public(), ALPN).unwrap();
        let server = server_config(&mallory, |_| true, ALPN).unwrap();
        let (client, _server) = handshake(client, server);
        assert!(matches!(client, Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))));
    }

    #[test]
    fn server_refuses_a_client_it_does_not_know() {
        let (alice, bob) = (NodeKey::generate().unwrap(), NodeKey::generate().unwrap());
        let client = client_config(&alice, bob.public(), ALPN).unwrap();
        let server = server_config(&bob, |_| false, ALPN).unwrap();
        let (_client, server) = handshake(client, server);
        assert!(matches!(server, Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))));
    }

    #[test]
    fn a_certificate_with_another_kind_of_key_is_not_accepted() {
        let other = rcgen::generate_simple_self_signed(vec!["x".to_owned()]).unwrap();
        assert!(matches!(peer_key(other.cert.der()), Err(TlsError::NotEd25519)));
        assert!(matches!(peer_key(&CertificateDer::from(vec![1, 2, 3])), Err(TlsError::BadCertificate)));
    }
}
