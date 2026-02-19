use anyhow::{anyhow, Context, Result};
use lasersell_sdk::stream::client::{
    StreamClient as SdkStreamClient, StreamConfigure, StreamConnectionStatus, StreamSender,
};
use lasersell_sdk::stream::proto::{MarketContextMsg, ServerMessage, StrategyConfigMsg};
use secrecy::SecretString;
use tokio::sync::mpsc;
use tracing::{info, warn};

#[derive(Clone)]
pub struct StreamClient {
    sdk: SdkStreamClient,
    wallet_pubkey: String,
    strategy: StrategyConfigMsg,
}

#[derive(Clone, Debug)]
pub struct StreamHandle {
    sender: StreamSender,
}

impl StreamHandle {
    pub fn update_strategy(&self, strategy: StrategyConfigMsg) -> Result<()> {
        self.sender
            .update_strategy(strategy)
            .map_err(|err| anyhow!("send update_strategy: {err}"))
    }

    pub fn request_exit_signal(&self, position_id: u64, slippage_bps: Option<u16>) -> Result<()> {
        self.sender
            .request_exit_signal(position_id, slippage_bps)
            .map_err(|err| anyhow!("send request_exit_signal: {err}"))
    }
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    ConnectionStatus {
        connected: bool,
    },
    BalanceUpdate {
        mint: String,
        token_program: Option<String>,
        token_account: Option<String>,
        tokens: u64,
        slot: u64,
    },
    PositionOpened {
        position_id: u64,
        mint: String,
        token_program: Option<String>,
        token_account: String,
        tokens: u64,
        slot: u64,
        market_context: Option<MarketContextMsg>,
    },
    PositionClosed {
        position_id: u64,
        mint: String,
        token_account: Option<String>,
        reason: String,
        slot: u64,
    },
    ExitSignalWithTx {
        position_id: u64,
        mint: String,
        token_program: Option<String>,
        token_account: Option<String>,
        position_tokens: u64,
        profit_units: i64,
        reason: String,
        triggered_at_ms: u64,
        market_context: Option<MarketContextMsg>,
        unsigned_tx_b64: String,
    },
}

impl StreamClient {
    pub fn new(
        api_key: SecretString,
        local: bool,
        wallet_pubkey: String,
        strategy: StrategyConfigMsg,
    ) -> Self {
        Self {
            sdk: SdkStreamClient::new(api_key).with_local_mode(local),
            wallet_pubkey,
            strategy,
        }
    }

    pub async fn connect(&self) -> Result<(StreamHandle, mpsc::UnboundedReceiver<StreamEvent>)> {
        let connection = self
            .sdk
            .connect(StreamConfigure::single_wallet(
                self.wallet_pubkey.clone(),
                self.strategy.clone(),
            ))
            .await
            .context("connect to stream server")?;
        info!(event = "stream_client_authed");

        let (sender, mut inbound_rx, mut status_rx) = connection.split_with_status();
        let stream_handle = StreamHandle {
            sender: sender.clone(),
        };

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let status_tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(status) = status_rx.recv().await {
                let connected = matches!(status, StreamConnectionStatus::Connected);
                if status_tx
                    .send(StreamEvent::ConnectionStatus { connected })
                    .is_err()
                {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            while let Some(server_msg) = inbound_rx.recv().await {
                if let Some(event) = map_server_event(server_msg) {
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
            }
        });

        Ok((stream_handle, event_rx))
    }
}

fn map_server_event(msg: ServerMessage) -> Option<StreamEvent> {
    match msg {
        ServerMessage::BalanceUpdate {
            mint,
            token_program,
            token_account,
            tokens,
            slot,
            ..
        } => Some(StreamEvent::BalanceUpdate {
            mint,
            token_program,
            token_account,
            tokens,
            slot,
        }),
        ServerMessage::PositionOpened {
            position_id,
            mint,
            token_account,
            token_program,
            tokens,
            market_context,
            slot,
            ..
        } => Some(StreamEvent::PositionOpened {
            position_id,
            mint,
            token_program,
            token_account,
            tokens,
            slot,
            market_context,
        }),
        ServerMessage::PositionClosed {
            position_id,
            mint,
            token_account,
            reason,
            slot,
            ..
        } => Some(StreamEvent::PositionClosed {
            position_id,
            mint,
            token_account,
            reason,
            slot,
        }),
        ServerMessage::ExitSignalWithTx {
            position_id,
            mint,
            token_account,
            token_program,
            position_tokens,
            profit_units,
            reason,
            triggered_at_ms,
            market_context,
            unsigned_tx_b64,
            ..
        } => Some(StreamEvent::ExitSignalWithTx {
            position_id,
            mint,
            token_program,
            token_account,
            position_tokens,
            profit_units,
            reason,
            triggered_at_ms,
            market_context,
            unsigned_tx_b64,
        }),
        ServerMessage::HelloOk { .. } | ServerMessage::Pong { .. } => None,
        ServerMessage::PnlUpdate { .. } => None,
        ServerMessage::Error { code, message } => {
            warn!(event = "stream_server_error", code = %code, message = %message);
            None
        }
    }
}
