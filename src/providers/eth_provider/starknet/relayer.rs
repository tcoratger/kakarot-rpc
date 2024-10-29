use crate::{
    models::transaction::transaction_data_to_starknet_calldata,
    providers::eth_provider::{
        error::{SignatureError, TransactionError},
        provider::EthApiResult,
        starknet::kakarot_core::{starknet_address, EXECUTE_FROM_OUTSIDE},
    },
};use crate::constants::STARKNET_CHAIN_ID;
use reth_primitives::TransactionSigned;
use starknet::{
    accounts::{Account, ExecutionEncoding, ExecutionV1, SingleOwnerAccount},
    core::types::{Felt, NonZeroFelt},
    providers::Provider,
    signers::{LocalWallet, SigningKey},
};
use std::{env::var, ops::Deref, str::FromStr, sync::LazyLock};

/// Signer for all relayers
static RELAYER_SIGNER: LazyLock<LocalWallet> = LazyLock::new(|| {
    LocalWallet::from_signing_key(SigningKey::from_secret_scalar(
        Felt::from_str(&var("RELAYER_PRIVATE_KEY").expect("missing relayer private key"))
            .expect("failed to parse relayer private key"),
    ))
});

/// A relayer holding an account and a balance.
///
/// The relayer is used to sign  transactions and broadcast them on the network.
#[derive(Debug)]
pub struct Relayer<SP: Provider + Send + Sync> {
    /// The account used to sign and broadcast the transaction
    account: SingleOwnerAccount<SP, LocalWallet>,
    /// The balance of the relayer
    balance: Felt,
}

impl<SP> Relayer<SP>
where
    SP: Provider + Send + Sync,
{
    /// Create a new relayer with the provided Starknet provider, address, balance.
    pub fn new(address: Felt, balance: Felt, provider: SP) -> Self {
        let relayer =
            SingleOwnerAccount::new(provider, RELAYER_SIGNER.clone(), address, *STARKNET_CHAIN_ID, ExecutionEncoding::New);

        Self { account: relayer, balance }
    }

    /// Relay the provided Ethereum transaction on the Starknet network.
    /// The relayer nonce is directly fetched from the chain to have the most up-to-date value.
    /// This is a way to avoid nonce issues.
    ///
    /// Returns the corresponding Starknet transaction hash.
    pub async fn relay_transaction(&self, transaction: &TransactionSigned, relayer_nonce: Felt) -> EthApiResult<Felt> {
        // Transform the transaction's data to Starknet calldata
        let relayer_address = self.account.address();
        let calldata = transaction_data_to_starknet_calldata(transaction, relayer_address)?;

        // Recover the signer
        let eoa_address = transaction.recover_signer().ok_or(SignatureError::Recovery)?;
        let eoa_address = starknet_address(eoa_address);

        // Construct the call
        let call = starknet::core::types::Call { to: eoa_address, selector: *EXECUTE_FROM_OUTSIDE, calldata };
        let mut execution = ExecutionV1::new(vec![call], &self.account);
        execution = execution.nonce(relayer_nonce);

        // We set the max fee to the balance of the account / 5. This means that the account could
        // send up to 5 transactions before hitting a feeder gateway error.
        execution = execution.max_fee(self.balance.floor_div(&NonZeroFelt::from_felt_unchecked(5.into())));

        let prepared = execution.prepared().map_err(|_| SignatureError::SigningFailure)?;
        let res = prepared.send().await.map_err(|err| TransactionError::Broadcast(err.into()))?;

        Ok(res.transaction_hash)
    }

    pub fn address(&self) -> Felt {
        self.account.address()
    }
}

impl<SP> Deref for Relayer<SP>
where
    SP: Provider + Send + Sync,
{
    type Target = SingleOwnerAccount<SP, LocalWallet>;

    fn deref(&self) -> &Self::Target {
        &self.account
    }
}
