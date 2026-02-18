use anyhow::{Context, Result};
use lasersell_sdk::tx::{
    send_via_helius_sender, send_via_rpc, sign_unsigned_tx as sdk_sign_unsigned_tx,
};
use solana_sdk::signature::Keypair;
use solana_sdk::transaction::VersionedTransaction;

pub fn sign_unsigned_tx(unsigned_tx_b64: &str, keypair: &Keypair) -> Result<VersionedTransaction> {
    sdk_sign_unsigned_tx(unsigned_tx_b64, keypair).context("sign unsigned tx")
}

pub async fn send_tx(
    http: &reqwest::Client,
    rpc_url: &str,
    tx: &VersionedTransaction,
    local_mode: bool,
) -> Result<String> {
    if local_mode {
        send_via_rpc(http, rpc_url, tx)
            .await
            .context("send tx via rpc")
    } else {
        send_via_helius_sender(http, tx)
            .await
            .context("send tx via helius sender")
    }
}
