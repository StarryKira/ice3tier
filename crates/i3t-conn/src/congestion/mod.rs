//! 拥塞控制：连接用哪一种，以及自己写的那一种。
//!
//! QUIC 库自带 Cubic 和一份实验性的 BBR。Cubic 把每一次丢包都当作拥塞，在有随机丢包的长距离链路上速度很低；
//! 自带的 BBR 带宽估计只升不降，不能用。[`bbr`] 是按公开的草案自己写的一份。

mod bbr;
mod meter;

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use quinn::congestion::{ControllerFactory, CubicConfig};

use self::bbr::BbrConfig;
use self::meter::Metered;

/// 连接用哪种拥塞控制。
///
/// 默认用 [`bbr`]，依据是 `bench/congestion` 在本机实验环境里的对比（起真实的节点经出口下载）：往返 50ms、
/// 随机丢包 1% 的 100 Mbit/s 链路上 Cubic 只有 2～3 Mbit/s，BBR 是 83～90；没有丢包时两者的吞吐只差 3%，
/// Cubic 却把瓶颈处的缓冲排满（往返多出 80～100ms，自己丢 1.8%），BBR 不丢包、只多几毫秒；带宽骤降、换路、
/// 经 TCP 中转的那一段丢包时，BBR 几秒之内跟上，小消息的往返都在一秒之内。不如 Cubic 的地方：和 Cubic 共用一个
/// 缓冲很深的瓶颈时只拿到约四分之一。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Algorithm {
    /// QUIC 库自带的 Cubic。
    Cubic,
    /// 本 crate 的 [`bbr`]。
    #[default]
    Bbr,
}

impl Algorithm {
    /// 临时指定拥塞控制的环境变量，取值 `cubic` 或 `bbr`。做对比测试时用；正式的配置入口还没有。
    pub(crate) const ENV: &str = "I3T_CONGESTION";

    /// 环境变量 [`Self::ENV`] 指定的那一种；没有设置或者值看不懂时用默认的。
    pub(crate) fn from_env() -> Self {
        let Ok(value) = std::env::var(Self::ENV) else { return Self::default() };
        value.parse().unwrap_or_else(|_| {
            let fallback = Self::default();
            tracing::warn!(%value, %fallback, "环境变量 {} 的值不是 cubic 或 bbr，改用默认的拥塞控制", Self::ENV);
            fallback
        })
    }

    /// 交给 QUIC 库传输配置的工厂。
    pub(crate) fn factory(self) -> Arc<dyn ControllerFactory + Send + Sync> {
        match self {
            Self::Cubic => Arc::new(Metered::new(self, Arc::new(CubicConfig::default()))),
            Self::Bbr => Arc::new(Metered::new(self, Arc::new(BbrConfig))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Cubic => "cubic",
            Self::Bbr => "bbr",
        }
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 不认识的拥塞控制名字。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("不认识的拥塞控制 {0:?}，可以是 cubic 或 bbr")]
pub(crate) struct UnknownAlgorithm(String);

impl FromStr for Algorithm {
    type Err = UnknownAlgorithm;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        [Self::Cubic, Self::Bbr].into_iter().find(|algorithm| algorithm.as_str() == name).ok_or_else(|| UnknownAlgorithm(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for algorithm in [Algorithm::Cubic, Algorithm::Bbr] {
            assert_eq!(algorithm.to_string().parse(), Ok(algorithm));
        }
        assert_eq!("reno".parse::<Algorithm>(), Err(UnknownAlgorithm("reno".to_owned())));
    }
}
