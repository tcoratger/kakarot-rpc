use std::collections::HashMap;

use crate::eth_provider::{
    error::{EthApiError, EthereumDataFormatError},
    provider::EthereumProvider,
};
use reth_primitives::{Address, B256, U256};
use reth_revm::{
    db::{AccountState, CacheDB, DbAccount},
    primitives::{Account, AccountInfo, Bytecode},
    Database, DatabaseCommit,
};
use reth_rpc_types::{serde_helpers::JsonStorageKey, BlockId, BlockNumberOrTag};
use tokio::runtime::Handle;

pub(crate) struct EthDatabaseSnapshot<P: EthereumProvider + Send + Sync> {
    cache: CacheDB<P>,
    block_id: BlockId,
}

impl<P: EthereumProvider + Send + Sync> EthDatabaseSnapshot<P> {
    pub(crate) fn new(provider: P, block_id: BlockId) -> Self {
        Self { cache: CacheDB::new(provider), block_id }
    }
}

impl<P: EthereumProvider + Send + Sync> Database for EthDatabaseSnapshot<P> {
    type Error = EthApiError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let cache = &self.cache;
        if let Some(account) = cache.accounts.get(&address) {
            return Ok(Some(account.info.clone()));
        }

        let account_info = Handle::current().block_on(async {
            let bytecode = cache.db.get_code(address, Some(self.block_id)).await?;
            let bytecode = Bytecode::new_raw(bytecode);
            let code_hash = bytecode.hash_slow();

            let nonce = cache.db.transaction_count(address, Some(self.block_id)).await?.to();
            let balance = cache.db.balance(address, Some(self.block_id)).await?;

            Result::<_, EthApiError>::Ok(AccountInfo { nonce, balance, code: Some(bytecode), code_hash })
        })?;

        self.cache.insert_account_info(address, account_info.clone());
        Ok(Some(account_info))
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self.cache.contracts.get(&code_hash).cloned().unwrap_or_default())
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let cache = &self.cache;
        if let Some(account) = cache.accounts.get(&address) {
            if let Some(storage) = account.storage.get(&index) {
                return Ok(*storage);
            }
        }

        let storage = Handle::current().block_on(async {
            let value = cache
                .db
                .storage_at(address, JsonStorageKey(B256::from_slice(&index.to_be_bytes::<32>())), Some(self.block_id))
                .await?;
            Result::<_, EthApiError>::Ok(value)
        })?;
        let storage = U256::from_be_bytes(storage.0);

        self.cache.accounts.entry(address).or_default().storage.insert(index, storage);
        Ok(storage)
    }

    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        let cache = &self.cache;
        if let Some(hash) = cache.block_hashes.get(&number) {
            return Ok(*hash);
        }

        let block_number = number.try_into().map_err(|_| EthereumDataFormatError::PrimitiveError)?;
        let hash = Handle::current().block_on(async {
            let hash = cache
                .db
                .block_by_number(BlockNumberOrTag::Number(block_number), false)
                .await?
                .ok_or(EthApiError::UnknownBlock)?
                .header
                .hash
                .unwrap_or_default();
            Result::<_, EthApiError>::Ok(hash)
        })?;

        self.cache.block_hashes.insert(number, hash);
        Ok(hash)
    }
}

impl<P: EthereumProvider + Send + Sync> DatabaseCommit for EthDatabaseSnapshot<P> {
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        changes.into_iter().for_each(|(address, account)| {
            let db_account = DbAccount {
                info: account.info.clone(),
                storage: account.storage.into_iter().map(|(k, v)| (k, v.present_value)).collect(),
                account_state: AccountState::None,
            };
            self.cache.accounts.insert(address, db_account);
        });
    }
}
