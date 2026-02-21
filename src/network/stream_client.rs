use anyhow::{anyhow, Context, Result};
use lasersell_sdk::stream::client::{
    StreamClient as SdkStreamClient, StreamConfigure, StreamSender,
};
use lasersell_sdk::stream::proto::{MarketContextMsg, ServerMessage, StrategyConfigMsg};
use lasersell_sdk::stream::session::{StreamEvent as SdkStreamEvent, StreamSession};
use secrecy::SecretString;
use tokio::sync::mpsc;
use tracing::{info, warn};

#[derive(Clone)]
pub struct StreamClient {
    sdk: SdkStreamClient,
    wallet_pubkey: String,
    strategy: StrategyConfigMsg,
    deadline_timeout_sec: u64,
}

#[derive(Clone, Debug)]
pub struct StreamHandle {
    sender: StreamSender,
    cmd_tx: mpsc::UnboundedSender<StreamCommand>,
}

#[derive(Debug, Clone)]
enum StreamCommand {
    UpdateStrategy {
        strategy: StrategyConfigMsg,
        deadline_timeout_sec: u64,
    },
}

impl StreamHandle {
    pub fn update_strategy(
        &self,
        strategy: StrategyConfigMsg,
        deadline_timeout_sec: u64,
    ) -> Result<()> {
        self.cmd_tx
            .send(StreamCommand::UpdateStrategy {
                strategy,
                deadline_timeout_sec,
            })
            .map_err(|err| anyhow!("queue update_strategy: {err}"))
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
        deadline_timeout_sec: u64,
    ) -> Self {
        Self {
            sdk: SdkStreamClient::new(api_key).with_local_mode(local),
            wallet_pubkey,
            strategy,
            deadline_timeout_sec,
        }
    }

    pub async fn connect(&self) -> Result<(StreamHandle, mpsc::UnboundedReceiver<StreamEvent>)> {
        let mut configure =
            StreamConfigure::single_wallet(self.wallet_pubkey.clone(), self.strategy.clone());
        configure.deadline_timeout_sec = self.deadline_timeout_sec;
        let mut session = StreamSession::connect(&self.sdk, configure)
            .await
            .context("connect to stream server")?;
        info!(event = "stream_client_authed");

        let sender = session.sender();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<StreamCommand>();
        let stream_handle = StreamHandle {
            sender: sender.clone(),
            cmd_tx,
        };

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let _ = event_tx.send(StreamEvent::ConnectionStatus { connected: true });

        tokio::spawn(async move {
            let mut commands_open = true;
            loop {
                tokio::select! {
                    maybe_cmd = cmd_rx.recv(), if commands_open => {
                        match maybe_cmd {
                            Some(StreamCommand::UpdateStrategy { strategy, deadline_timeout_sec }) => {
                                if let Err(err) = session.update_strategy_with_deadline(strategy, deadline_timeout_sec) {
                                    warn!(event = "stream_update_strategy_failed", error = %err);
                                }
                            }
                            None => {
                                commands_open = false;
                            }
                        }
                    }
                    maybe_evt = session.recv() => {
                        let Some(evt) = maybe_evt else {
                            let _ = event_tx.send(StreamEvent::ConnectionStatus { connected: false });
                            break;
                        };
                        if let Some(mapped) = map_session_event(evt) {
                            if event_tx.send(mapped).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok((stream_handle, event_rx))
    }
}

fn map_session_event(evt: SdkStreamEvent) -> Option<StreamEvent> {
    match evt {
        SdkStreamEvent::Message(msg) => map_server_event(msg),
        SdkStreamEvent::PositionOpened { message, .. } => map_server_event(message),
        SdkStreamEvent::PositionClosed { message, .. } => map_server_event(message),
        SdkStreamEvent::ExitSignalWithTx { message, .. } => map_server_event(message),
        SdkStreamEvent::PnlUpdate { message, .. } => map_server_event(message),
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
