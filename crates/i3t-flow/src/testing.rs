//! 给测试用的 TLS 握手消息：让 ice3proxy 上的流不再加密第二次。

/// 一条最小的、格式正确的 TLS 1.3 ClientHello 记录：作为首批数据时 [`i3t_conn::Payload::for_first_data`] 选 passthrough。
///
/// `tag` 放在 random 的开头，几条流要互相区分时用（比如目标靠它认出是哪一条流）。
#[must_use]
pub fn client_hello(tag: &[u8]) -> Vec<u8> {
    assert!(tag.len() <= 32, "random 只有 32 字节");
    let mut random = [0; 32];
    random[..tag.len()].copy_from_slice(tag);
    let mut record = vec![0x16, 0x03, 0x01, 0x00, 0x36, 0x01, 0x00, 0x00, 0x32, 0x03, 0x03];
    record.extend_from_slice(&random);
    // 空 session_id；一个密码套件（TLS_AES_128_GCM_SHA256）；不压缩；扩展只有 supported_versions = TLS 1.3。
    record.extend_from_slice(&[0x00, 0x00, 0x02, 0x13, 0x01, 0x01, 0x00, 0x00, 0x07, 0x00, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x04]);
    record
}

/// 一条最小的、格式正确的 TLS 1.3 ServerHello 记录：目标先回它，出口往回的方向在它之后才跟着不再加密（[`i3t_conn::ServerHelloWatch`]）。
#[must_use]
pub fn server_hello() -> Vec<u8> {
    let mut record = vec![0x16, 0x03, 0x03, 0x00, 0x32, 0x02, 0x00, 0x00, 0x2e, 0x03, 0x03];
    record.extend_from_slice(&[0x5e; 32]);
    // 空 session_id；TLS_AES_128_GCM_SHA256；不压缩；扩展只有 supported_versions = TLS 1.3。
    record.extend_from_slice(&[0x00, 0x13, 0x01, 0x00, 0x00, 0x06, 0x00, 0x2b, 0x00, 0x02, 0x03, 0x04]);
    record
}

#[cfg(test)]
mod tests {
    use i3t_conn::{Payload, ServerHelloVerdict, ServerHelloWatch};

    use super::*;

    #[test]
    fn they_let_both_directions_go_unencrypted() {
        assert_eq!(Payload::for_first_data(&client_hello(b"")), Payload::Passthrough);
        assert_eq!(Payload::for_first_data(&client_hello(&[7; 32])), Payload::Passthrough);
        assert_ne!(client_hello(b"a"), client_hello(b"b"));
        assert_eq!(ServerHelloWatch::new().feed(&server_hello()), ServerHelloVerdict::PassthroughAfter(server_hello().len()));
    }
}
