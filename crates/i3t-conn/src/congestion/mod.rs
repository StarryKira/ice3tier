//! 拥塞控制：连接用哪一种，以及自己写的那一种。
//!
//! QUIC 库自带 Cubic 和一份实验性的 BBR。Cubic 把每一次丢包都当作拥塞，在有随机丢包的长距离链路上速度很低；
//! 自带的 BBR 带宽估计只升不降，不能用。[`bbr`] 是按公开的草案自己写的一份。

mod bbr;
mod meter;

use std::sync::Arc;

use i3t_model::netmap::Congestion;
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
    /// 指定节点默认的拥塞控制的环境变量，取值 `cubic` 或 `bbr`。做对比测试时用；正式的配置是节点对的设置
    /// （`i3tctl pair set … --congestion`），它盖过这里。
    pub(crate) const ENV: &str = "I3T_CONGESTION";

    /// 环境变量 [`Self::ENV`] 指定的那一种（写法同 [`Congestion`]）；没有设置或者值看不懂时用默认的。
    pub(crate) fn from_env() -> Self {
        let Ok(value) = std::env::var(Self::ENV) else { return Self::default() };
        Self::from_env_value(&value)
    }

    /// 环境变量 [`Self::ENV`] 的值是 `value` 时用哪一种：看不懂时用默认的，记一行日志。
    fn from_env_value(value: &str) -> Self {
        value.parse::<Congestion>().map(Self::from).unwrap_or_else(|_| {
            let fallback = Self::default();
            tracing::warn!(%value, fallback = %Congestion::from(fallback), "环境变量 {} 的值不是 cubic 或 bbr，改用默认的拥塞控制", Self::ENV);
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
}

impl From<Congestion> for Algorithm {
    fn from(congestion: Congestion) -> Self {
        match congestion {
            Congestion::Bbr => Self::Bbr,
            Congestion::Cubic => Self::Cubic,
        }
    }
}

/// 日志和对外报告里按 [`Congestion`] 的写法说是哪一种。
impl From<Algorithm> for Congestion {
    fn from(algorithm: Algorithm) -> Self {
        match algorithm {
            Algorithm::Bbr => Self::Bbr,
            Algorithm::Cubic => Self::Cubic,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_like_the_netmap_setting_and_an_unknown_one_falls_back_to_the_default() {
        for congestion in [Congestion::Cubic, Congestion::Bbr] {
            assert_eq!(Congestion::from(Algorithm::from_env_value(congestion.as_str())), congestion);
        }
        assert_eq!(Algorithm::from_env_value("reno"), Algorithm::default());
        assert_eq!(Congestion::from(Algorithm::default()), Congestion::Bbr);
    }
}
