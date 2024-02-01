use ethers::abi::AbiEncode;
use ethers::prelude::abigen;
use reth_primitives::Address;

use reth_primitives::{BlockId, U256};
use reth_rpc_types::{CallInput, CallRequest};

use crate::eth_provider::provider::EthProviderResult;
use crate::eth_provider::provider::EthereumProvider;
use crate::models::errors::ConversionError;

// abigen generates a lot of unused code, needs to be benchmarked if performances ever become a
// concern
abigen!(
    IERC20,
    r#"[
        function balanceOf(address account) external view returns (uint256)
        function allowance(address owner, address spender) external view returns (uint256)
    ]"#,
);

/// Abstraction for a Kakarot ERC20 contract.
pub struct EthereumErc20<P: EthereumProvider> {
    pub address: Address,
    pub provider: P,
}

impl<P: EthereumProvider> EthereumErc20<P> {
    pub const fn new(address: Address, provider: P) -> Self {
        Self { address, provider }
    }

    pub async fn balance_of(self, evm_address: Address, block_id: BlockId) -> EthProviderResult<U256> {
        // Prepare the calldata for the bytecode function call
        let calldata = IERC20Calls::BalanceOf(BalanceOfCall { account: evm_address.into() }).encode();

        let call = CallRequest {
            from: Some(Address::default()),
            to: Some(self.address),
            gas_price: Some(U256::from(1)),
            gas: Some(U256::from(1_000_000)),
            value: Some(U256::ZERO),
            input: CallInput { input: Some(calldata.into()), data: None },
            ..Default::default()
        };

        let ret = self.provider.call(call, Some(block_id)).await?;
        let balance = U256::try_from_be_slice(&ret)
            .ok_or_else(|| ConversionError::UintConversionError("Failed to convert call return to U256".to_string()))?;

        Ok(balance)
    }
}
