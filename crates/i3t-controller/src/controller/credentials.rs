//! 接入凭据的签发、改上限、吊销。
//!
//! 吊销之后记录留着，状态是"已吊销"，看得到它用过多少；对一个已经吊销的凭据再删一次才把记录彻底删掉，名字才能再用。

use i3t_model::admin::{
    CreateCredential, CreatedCredential, Credential, CredentialState, CredentialUsage, LogRef, NetworkId, RefKind, UpdateCredential,
};
use i3t_model::netmap::TokenHash;

use super::{Controller, Inner, naming};
use crate::error::{Entity, Error};
use crate::secret;
use crate::store::CredentialRecord;

/// 令牌开头留给人认的字符数。令牌是 43 个 base64url 字符（256 位），露出 6 个还剩 220 位。
pub(super) const PREFIX_LEN: usize = 6;

impl Controller {
    /// 一个网络的全部凭据，包括吊销了的。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn credentials(&self, network: NetworkId) -> Result<Vec<Credential>, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        Ok(inner.store.credentials(network)?.into_iter().map(|record| inner.credential_view(network, record)).collect())
    }

    /// 签发一个凭据。返回的令牌只有这一次能拿到；入口节点拿到的是它的 SHA-256。
    ///
    /// # Errors
    ///
    /// 名字是空的或者已经有了；并发上限或者流量上限是 0。
    pub fn create_credential(&self, network: NetworkId, request: &CreateCredential) -> Result<CreatedCredential, Error> {
        let name = naming::title("凭据", &request.name, true)?;
        check_limits(request.max_concurrent, request.max_bytes)?;
        let token = secret::generate()?;
        let mut inner = self.lock();
        inner.require_network(network)?;
        let now = inner.clock.now();
        let mut record = CredentialRecord {
            credential: Credential {
                id: 0,
                name,
                max_concurrent: request.max_concurrent,
                created_at: now,
                prefix: token.chars().take(PREFIX_LEN).collect(),
                max_bytes: request.max_bytes,
                state: CredentialState::Active,
                revoked_at: None,
                usage: CredentialUsage::default(),
            },
            token: TokenHash::from_bytes(secret::sha256(token.as_bytes())),
        };
        record.credential.id = inner.store.atomically(|store| {
            if let Some(existing) = store.credentials(network)?.iter().find(|existing| existing.credential.name == record.credential.name) {
                return Err(if existing.is_revoked() {
                    Error::NameTaken(format!("{}（一个吊销了的凭据还占着这个名字，把它再删一次就能用）", record.credential.name))
                } else {
                    Error::NameTaken(record.credential.name.clone())
                });
            }
            let id = store.insert_credential(network, &record)?;
            let refs = [LogRef::new(RefKind::Credential, id)];
            store.log_refs(now, Some(network), "credential.create", &format!("签发凭据 {}", record.credential.name), &refs)?;
            Ok(id)
        })?;
        let seeds = inner.nodes_requiring_credentials(network)?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(CreatedCredential { credential: record.credential, token })
    }

    /// 改一个凭据的上限，或者把用量清零，不用重新签发令牌。新的上限随即下发给各个入口。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个凭据；上限是 0；凭据已经吊销。
    pub fn update_credential(&self, network: NetworkId, id: u32, request: &UpdateCredential) -> Result<Credential, Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let record = inner.store.atomically(|store| {
            let record = store.credential(network, id)?.ok_or(Error::NotFound(Entity::Credential))?;
            if record.is_revoked() {
                return Err(Error::invalid(format!("凭据 {} 已经吊销，改不了上限", record.credential.name)));
            }
            let max_concurrent = request.max_concurrent.unwrap_or(record.credential.max_concurrent);
            let max_bytes = request.max_bytes.unwrap_or(record.credential.max_bytes);
            check_limits(max_concurrent, max_bytes)?;
            store.set_credential_limits(id, max_concurrent, max_bytes)?;
            if request.reset_usage {
                store.reset_credential_usage(id)?;
            }
            let reset = if request.reset_usage { "，用量清零" } else { "" };
            let limit = |value: Option<u64>, unit: &str| value.map_or_else(|| "不限".to_owned(), |value| format!("{value}{unit}"));
            let detail = format!(
                "凭据 {}：并发 {}，流量 {}{reset}",
                record.credential.name,
                limit(max_concurrent.map(u64::from), ""),
                limit(max_bytes, " 字节")
            );
            store.log_refs(now, Some(network), "credential.update", &detail, &[LogRef::new(RefKind::Credential, id)])?;
            store.credential(network, id)?.ok_or(Error::NotFound(Entity::Credential))
        })?;
        let seeds = inner.nodes_requiring_credentials(network)?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(inner.credential_view(network, record))
    }

    /// 吊销一个凭据：入口不再认它，记录留着。已经吊销的再删一次，记录才被彻底删掉。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个凭据。
    pub fn delete_credential(&self, network: NetworkId, id: u32) -> Result<(), Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        inner.store.atomically(|store| {
            let record = store.credential(network, id)?.ok_or(Error::NotFound(Entity::Credential))?;
            let refs = [LogRef::new(RefKind::Credential, id)];
            if record.is_revoked() {
                store.delete_credential(id)?;
                store.log_refs(
                    now,
                    Some(network),
                    "credential.delete",
                    &format!("删除已经吊销的凭据 {}", record.credential.name),
                    &refs,
                )?;
            } else {
                store.revoke_credential(id, now)?;
                store.log_refs(now, Some(network), "credential.revoke", &format!("吊销凭据 {}", record.credential.name), &refs)?;
            }
            Ok::<_, Error>(())
        })?;
        let seeds = inner.nodes_requiring_credentials(network)?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(())
    }
}

impl Inner {
    /// 管理 API 里的凭据：存着的，加上在线的入口报的现在的连接数。
    fn credential_view(&self, network: NetworkId, record: CredentialRecord) -> Credential {
        let mut credential = record.credential;
        if credential.state != CredentialState::Revoked {
            credential.usage.connections = self.credential_connections(network, credential.id);
        }
        credential
    }
}

/// 上限要么不写（不限），要么是正数：写 0 多半是把"不限"写错了。
fn check_limits(max_concurrent: Option<u32>, max_bytes: Option<u64>) -> Result<(), Error> {
    if max_concurrent == Some(0) {
        return Err(Error::invalid("并发上限不能是 0；不限制就不要填"));
    }
    if max_bytes == Some(0) {
        return Err(Error::invalid("流量上限不能是 0；不限制就不要填"));
    }
    Ok(())
}
