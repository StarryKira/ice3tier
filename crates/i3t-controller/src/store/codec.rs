//! 列的值和 Rust 类型之间的转换。
//!
//! SQLite 只有整数、文本和 BLOB 几种值。这里的包装类型把"这一列怎么存"写在一处，各张表的读写代码只管用。

use std::fmt::Display;
use std::net::Ipv4Addr;
use std::str::FromStr;

use i3t_model::admin::NetworkId;
use i3t_proto::{NodeId, PublicKey};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// 按 JSON 形式存在 TEXT 列里的值：枚举、结构体、列表。
pub(super) struct Json<T>(pub T);

impl<T: Serialize> ToSql for Json<T> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let text = serde_json::to_string(&self.0).map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        Ok(ToSqlOutput::from(text))
    }
}

impl<T: DeserializeOwned> FromSql for Json<T> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        serde_json::from_str(value.as_str()?).map(Self).map_err(FromSqlError::other)
    }
}

/// 按文本形式存的值：地址、地址段。
pub(super) struct Text<T>(pub T);

impl<T: Display> ToSql for Text<T> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_string()))
    }
}

impl<T: FromStr> FromSql for Text<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_str()?.parse().map(Self).map_err(FromSqlError::other)
    }
}

/// SQLite 的整数是有符号 64 位的，版本号和时间用不到最高位。
pub(super) struct Unsigned(pub u64);

impl ToSql for Unsigned {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let value = i64::try_from(self.0).map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        Ok(ToSqlOutput::from(value))
    }
}

impl FromSql for Unsigned {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let raw = value.as_i64()?;
        u64::try_from(raw).map(Self).map_err(|_| FromSqlError::OutOfRange(raw))
    }
}

pub(super) struct SqlNode(pub NodeId);

impl ToSql for SqlNode {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.get()))
    }
}

impl FromSql for SqlNode {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let raw = value.as_i64()?;
        u32::try_from(raw).ok().and_then(NodeId::new).map(Self).ok_or(FromSqlError::OutOfRange(raw))
    }
}

pub(super) struct SqlNetwork(pub NetworkId);

impl ToSql for SqlNetwork {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.0))
    }
}

impl FromSql for SqlNetwork {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        u32::column_result(value).map(|raw| Self(NetworkId(raw)))
    }
}

pub(super) struct SqlKey(pub PublicKey);

impl ToSql for SqlKey {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.as_bytes().as_slice()))
    }
}

impl FromSql for SqlKey {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        <[u8; PublicKey::LEN]>::column_result(value).map(|bytes| Self(PublicKey::from_bytes(bytes)))
    }
}

/// 虚拟 IP 存成整数，这样唯一约束和排序都按数值来。
pub(super) struct SqlIp(pub Ipv4Addr);

impl ToSql for SqlIp {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(u32::from(self.0)))
    }
}

impl FromSql for SqlIp {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        u32::column_result(value).map(|raw| Self(Ipv4Addr::from(raw)))
    }
}

/// 表里自增的编号都从 1 起，放得进 `u32`；放不进说明文件被改坏了。
pub(super) fn row_id(raw: i64) -> rusqlite::Result<u32> {
    u32::try_from(raw).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, raw))
}
