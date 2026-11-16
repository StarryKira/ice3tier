//! 接入凭据的签发和吊销。

use i3t_model::admin::{CreateCredential, CreatedCredential, Credential, NetworkId};
use i3t_model::netmap::TokenHash;

use super::{Controller, Inner, Seeds, naming};
use crate::error::{Entity, Error};
use crate::secret;
use crate::store::CredentialRecord;

impl Controller {
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn credentials(&self, network: NetworkId) -> Result<Vec<Credential>, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        Ok(inner.store.credentials(network)?.into_iter().map(|record| record.credential).collect())
    }

    /// 签发一个凭据。返回的令牌只有这一次能拿到；入口节点拿到的是它的 SHA-256。
    ///
    /// # Errors
    ///
    /// 名字是空的或者已经有了；并发上限是 0。
    pub fn create_credential(&self, network: NetworkId, request: &CreateCredential) -> Result<CreatedCredential, Error> {
        let name = naming::title("凭据", &request.name, true)?;
        if request.max_concurrent == Some(0) {
            return Err(Error::invalid("并发上限不能是 0；不限制就不要填"));
        }
        let token = secret::generate()?;
        let mut inner = self.lock();
        inner.require_network(network)?;
        let now = inner.clock.now();
        let mut record = CredentialRecord {
            credential: Credential { id: 0, name, max_concurrent: request.max_concurrent, created_at: now },
            token: TokenHash::from_bytes(secret::sha256(token.as_bytes())),
        };
        record.credential.id = inner.store.atomically(|store| {
            if store.credentials(network)?.iter().any(|existing| existing.credential.name == record.credential.name) {
                return Err(Error::NameTaken(record.credential.name.clone()));
            }
            let id = store.insert_credential(network, &record)?;
            store.log(now, Some(network), "credential.create", &format!("签发凭据 {}", record.credential.name))?;
            Ok(id)
        })?;
        let seeds = inner.nodes_requiring_credentials(network)?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(CreatedCredential { credential: record.credential, token })
    }

    /// 吊销一个凭据。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个凭据。
    pub fn delete_credential(&self, network: NetworkId, id: u32) -> Result<(), Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        inner.store.atomically(|store| {
            let record = store.credential(network, id)?.ok_or(Error::NotFound(Entity::Credential))?;
            store.delete_credential(id)?;
            store.log(now, Some(network), "credential.delete", &format!("吊销凭据 {}", record.credential.name))?;
            Ok::<_, Error>(())
        })?;
        let seeds = inner.nodes_requiring_credentials(network)?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(())
    }
}

impl Inner {
    /// 凭据只下发给有监听器要求凭据的节点，所以凭据变了只重算它们。
    fn nodes_requiring_credentials(&self, network: NetworkId) -> Result<Seeds, Error> {
        let listeners = self.store.listeners(network)?;
        Ok(Seeds::of(listeners.into_iter().filter(|listener| listener.config.require_credential).map(|listener| listener.node)))
    }
}
