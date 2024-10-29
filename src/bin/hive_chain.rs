#![allow(clippy::significant_drop_tightening)]

use alloy_primitives::bytes::{Buf, BytesMut};
use alloy_rlp::Decodable;
use clap::Parser;
use kakarot_rpc::{
    into_via_try_wrapper,
    providers::{eth_provider::starknet::relayer::Relayer, sn_provider::StarknetProvider},
};
use reth_primitives::{Block, BlockBody};
use starknet::{
    core::types::{BlockId, BlockTag, Felt},
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider},
};
use std::{path::PathBuf, str::FromStr};
use tokio::{fs::File, io::AsyncReadExt};
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, FramedRead};
use url::Url;

struct BlockFileCodec;

impl Decoder for BlockFileCodec {
    type Item = Block;
    type Error = eyre::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }
        let buf_slice = &mut src.as_ref();
        let body = Block::decode(buf_slice)?;
        src.advance(src.len() - buf_slice.len());
        Ok(Some(body))
    }
}

/// The inputs to the binary.
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to the chain file for the hive test.
    #[clap(short, long)]
    chain_path: PathBuf,
    /// The relayer address.
    #[clap(long)]
    relayer_address: Felt,
    /// The relayer private key.
    #[clap(long)]
    relayer_pk: Felt,
}

const STARKNET_RPC_URL: &str = "http://0.0.0.0:5050";
const MAX_FELTS_IN_CALLDATA: &str = "30000";

/// Inspired by the Import command from Reth.
/// https://github.com/paradigmxyz/reth/blob/main/bin/reth/src/commands/import.rs
#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();

    // Get the provider
    let provider = JsonRpcClient::new(HttpTransport::new(Url::from_str(STARKNET_RPC_URL)?));
    let starknet_provider = StarknetProvider::new(provider);

    // Set the env
    std::env::set_var("RELAYER_PRIVATE_KEY", format!("0x{:x}", args.relayer_pk));
    std::env::set_var("MAX_FELTS_IN_CALLDATA", MAX_FELTS_IN_CALLDATA);
    std::env::set_var("STARKNET_NETWORK", STARKNET_RPC_URL);

    // Prepare the relayer
    let relayer_balance = starknet_provider.balance_at(args.relayer_address, BlockId::Tag(BlockTag::Latest)).await?;
    let relayer_balance = into_via_try_wrapper!(relayer_balance)?;

    let mut current_nonce = Felt::ZERO;
    let relayer = Relayer::new(
        args.relayer_address,
        relayer_balance,
        JsonRpcClient::new(HttpTransport::new(Url::from_str(STARKNET_RPC_URL)?)),
    );

    // Read the rlp file
    let mut file = File::open(args.chain_path).await?;

    let metadata = file.metadata().await?;
    let file_len = metadata.len();

    // Read the entire file into memory
    let mut reader = vec![];
    file.read_to_end(&mut reader).await?;
    let mut stream = FramedRead::with_capacity(&reader[..], BlockFileCodec, file_len as usize);

    // Extract the block
    let mut bodies: Vec<BlockBody> = Vec::new();
    while let Some(block_res) = stream.next().await {
        let block = block_res?;
        bodies.push(block.into());
    }

    for (block_number, body) in bodies.into_iter().enumerate() {
        while starknet_provider.block_number().await? < block_number as u64 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        for transaction in &body.transactions {
            relayer.relay_transaction(transaction, current_nonce).await?;
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Increase the relayer's nonce
            current_nonce += Felt::ONE;
        }
    }

    Ok(())
}
