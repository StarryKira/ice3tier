//! 加入网络：`i3t join` 做的事。

use anyhow::Context;
use i3t_model::Netmap;
use i3t_model::control::{EnrollToken, Hello};

use crate::config::{Capability, Config};
use crate::control;
use crate::state::{ControllerInfo, StateDir};

/// 加入时在命令行上给出的本地声明。没有给的保持配置文件里原来的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JoinOptions {
    pub can_exit: bool,
    pub can_relay: bool,
    pub can_forward: bool,
}

/// 用注册令牌加入网络，把结果存进状态目录，返回 controller 给的第一份 netmap。
///
/// 顺序是先写本地的东西（密钥、配置文件、声明），再去找 controller。注册成功之后才记下 controller 是谁，
/// 所以失败了可以原样重来；令牌是一次性的，注册成功之后再跑一遍会被 controller 拒绝。
///
/// # Errors
///
/// 令牌不对、状态目录写不了、连不上 controller 或者 controller 拒绝。
pub async fn join(state: &StateDir, token: &str, options: JoinOptions) -> anyhow::Result<Netmap> {
    let token: EnrollToken = token.parse().context("这不是一个注册令牌。令牌以 i3t- 开头，在 WebUI 的“添加节点”里生成")?;
    let key = state.key_or_generate()?;

    let config_path = state.config_path();
    Config::write_template_if_missing(&config_path)?;
    let declared = [(Capability::Exit, options.can_exit), (Capability::Relay, options.can_relay), (Capability::Forward, options.can_forward)];
    for (capability, _) in declared.into_iter().filter(|&(_, on)| on) {
        Config::set_declared(&config_path, capability, true)?;
    }
    let config = Config::load(&config_path)?;

    let controller = ControllerInfo { address: token.controller.clone(), key: token.key };
    let netmap = control::enroll(&controller, &key, &token.secret, hello(&config)).await?;
    state.save_controller(&controller)?;
    state.save_netmap(&netmap)?;
    Ok(netmap)
}

/// 这个节点现在的自我介绍。
#[must_use]
pub fn hello(config: &Config) -> Hello {
    Hello {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        declared: config.declare.into(),
        port: config.port,
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::control::{ToController, ToNode};

    use super::*;
    use crate::control::testing::{FakeController, expect, netmap};
    use crate::control::write_message;

    fn token_for(controller: &FakeController) -> String {
        EnrollToken { controller: controller.info.address.clone(), key: controller.info.key, secret: "s3cret".to_owned() }.to_string()
    }

    #[tokio::test]
    async fn join_enrolls_declares_and_persists() {
        let controller = FakeController::start().await;
        let token = token_for(&controller);
        let expected_controller = controller.info.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = controller.accept().await;
            let ToController::Enroll { token, hello } = expect(&mut stream).await else { panic!("第一条消息不是 enroll") };
            write_message(&mut stream, &ToNode::Snapshot(Box::new(netmap(1)))).await.unwrap();
            let _: ToController = expect(&mut stream).await;
            (token, hello)
        });

        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        let options = JoinOptions { can_exit: true, ..JoinOptions::default() };
        let received = join(&state, &token, options).await.unwrap();
        let (secret, hello) = server.await.unwrap();

        assert_eq!(received.version, 1);
        assert_eq!(secret, "s3cret");
        assert!(hello.declared.exit && !hello.declared.relay);
        assert_eq!(hello.port, 20330);
        assert_eq!(state.controller().unwrap(), Some(expected_controller));
        assert_eq!(state.netmap().unwrap().unwrap().version, 1);
        assert!(Config::load(&state.config_path()).unwrap().declare.exit);
    }

    #[tokio::test]
    async fn a_refused_join_leaves_no_controller_behind() {
        let controller = FakeController::start().await;
        let token = token_for(&controller);
        tokio::spawn(async move {
            let (mut stream, _) = controller.accept().await;
            let _: ToController = expect(&mut stream).await;
            write_message(&mut stream, &ToNode::Rejected { reason: "令牌过期".into() }).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        let error = join(&state, &token, JoinOptions::default()).await.unwrap_err();
        assert!(format!("{error:#}").contains("令牌过期"));
        assert_eq!(state.controller().unwrap(), None);
        assert!(state.netmap().unwrap().is_none());
    }

    #[tokio::test]
    async fn text_that_is_not_a_token_is_explained() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        let error = join(&state, "hello", JoinOptions::default()).await.unwrap_err();
        assert!(format!("{error:#}").contains("i3t-"));
    }
}
