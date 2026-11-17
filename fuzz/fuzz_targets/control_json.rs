//! 节点和 controller 之间的消息（`i3t_model::control`）：controller 读任何连上来的节点（包括还在用注册令牌加入的）发的 JSON，
//! 节点读 controller 发的 JSON。帧头的 4 字节长度只决定读多少，内容就是这里的一个 JSON 对象。
//! 另外是 `i3t join` 从标准输入读的注册令牌。
//!
//! 除了不能 panic，还查：读得出来的消息写回 JSON 再读，得到的是同一条消息（两边对同一段 JSON 的理解一致）。

#![no_main]

use std::fmt::Debug;

use i3t_model::control::{EnrollToken, ToController, ToNode};
use libfuzzer_sys::fuzz_target;
use serde::Serialize;
use serde::de::DeserializeOwned;

fuzz_target!(|data: &[u8]| {
    round_trip::<ToController>(data);
    round_trip::<ToNode>(data);
    if let Some(token) = std::str::from_utf8(data).ok().and_then(|text| text.parse::<EnrollToken>().ok()) {
        let again: EnrollToken = token.to_string().parse().expect("自己写出来的令牌读不回来");
        assert!(again == token, "注册令牌写出来再读，内容变了");
    }
});

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(data: &[u8]) {
    let Ok(message) = serde_json::from_slice::<T>(data) else { return };
    let json = serde_json::to_vec(&message).expect("读得出来的消息一定写得回去");
    let again: T = serde_json::from_slice(&json)
        .unwrap_or_else(|error| panic!("{} 写回的 JSON 读不回来：{error}\n{}", std::any::type_name::<T>(), String::from_utf8_lossy(&json)));
    assert_eq!(again, message, "{} 写回 JSON 再读，内容变了", std::any::type_name::<T>());
}
