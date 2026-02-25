use anyhow::{Context, Result};
use lasersell_sdk::tx::{
    confirm_signature_via_rpc, send_transaction, SendTarget,
    sign_unsigned_tx as sdk_sign_unsigned_tx,
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
    send_target: &SendTarget,
    confirm_timeout: std::time::Duration,
) -> Result<String> {
    let signature = send_transaction(http, send_target, tx)
        .await
        .context("send tx")?;
    confirm_signature_via_rpc(http, rpc_url, &signature, confirm_timeout)
        .await
        .context("confirm tx via rpc")?;
    Ok(signature)
}
