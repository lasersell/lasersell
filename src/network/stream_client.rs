use anyhow::{Context, Result};
use lasersell_sdk::exit_api::{ExitApiClient, ExitApiClientOptions};
use lasersell_sdk::stream::client::{
    StreamClient as SdkStreamClient, StreamConfigure, StreamSender,
};
use lasersell_sdk::stream::proto::{
    MarketContextMsg, MirrorConfigMsg, ServerMessage, StrategyConfigMsg, WatchWalletEntryMsg,
};
use lasersell_sdk::stream::session::{StreamEvent as SdkStreamEvent, StreamSession};
use secrecy::{ExposeSecret, SecretString};
use solana_sdk::signature::Keypair;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct StreamClient {
    sdk: SdkStreamClient,
    api_key: SecretString,
    local: bool,
    wallet_pubkey: String,
    strategy: StrategyConfigMsg,
    deadline_timeout_sec: u64,
    send_mode: Option<String>,
    tip_lamports: Option<u64>,
    watch_wallets: Vec<WatchWalletEntryMsg>,
    mirror_config: Option<MirrorConfigMsg>,
}

#[derive(Clone, Debug)]
pub struct StreamHandle {
    sender: StreamSender,
}

impl StreamHandle {
    pub fn request_exit_signal(&self, position_id: u64, slippage_bps: Option<u16>) -> Result<()> {
        self.sender
            .request_exit_signal(position_id, slippage_bps)
            .map_err(|err| anyhow::anyhow!("send request_exit_signal: {err}"))
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
        entry_quote_units: u64,
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
    PnlUpdate {
        mint: String,
        profit_units: i64,
        proceeds_units: u64,
    },
}

impl StreamClient {
    pub fn new(
        api_key: SecretString,
        local: bool,
        wallet_pubkey: String,
        strategy: StrategyConfigMsg,
        deadline_timeout_sec: u64,
        send_mode: Option<String>,
        tip_lamports: Option<u64>,
        watch_wallets: Vec<WatchWalletEntryMsg>,
        mirror_config: Option<MirrorConfigMsg>,
    ) -> Self {
        Self {
            sdk: SdkStreamClient::new(api_key.clone()).with_local_mode(local),
            api_key,
            local,
            wallet_pubkey,
            strategy,
            deadline_timeout_sec,
            send_mode,
            tip_lamports,
            watch_wallets,
            mirror_config,
        }
    }

    pub async fn connect(
        &self,
        keypair: &Keypair,
    ) -> Result<(StreamHandle, mpsc::UnboundedReceiver<StreamEvent>)> {
        // Register wallet ownership before connecting to stream.
        let proof = lasersell_sdk::exit_api::prove_ownership(keypair);
        let api_key_trimmed = self.api_key.expose_secret().trim().to_string();
        if !api_key_trimmed.is_empty() {
            let client = ExitApiClient::with_options(
                Some(SecretString::new(api_key_trimmed)),
                ExitApiClientOptions::default(),
            )
            .context("build LaserSell API client for wallet registration")?
            .with_local_mode(self.local);
            client
                .register_wallet(&proof, None)
                .await
                .context("register wallet ownership with LaserSell API")?;
            info!(event = "wallet_registered", wallet = %self.wallet_pubkey);
        }

        let mut configure =
            StreamConfigure::single_wallet(self.wallet_pubkey.clone(), self.strategy.clone());
        configure.deadline_timeout_sec = self.deadline_timeout_sec;
        configure.send_mode = self.send_mode.clone();
        configure.tip_lamports = self.tip_lamports;
        configure.watch_wallets = self.watch_wallets.clone();
        configure.mirror_config = self.mirror_config.clone();
        let mut session = StreamSession::connect(&self.sdk, configure)
            .await
            .context("connect to stream server")?;
        info!(event = "stream_client_authed");

        // Enable priority lanes so exit signals are never delayed by PnL updates.
        session.enable_lanes(64);

        let sender = session.sender();
        let stream_handle = StreamHandle { sender };

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let _ = event_tx.send(StreamEvent::ConnectionStatus { connected: true });

        tokio::spawn(async move {
            loop {
                let Some(evt) = session.recv().await else {
                    warn!(event = "stream_session_ended");
                    let _ = event_tx.send(StreamEvent::ConnectionStatus { connected: false });
                    break;
                };
                if let Some(mapped) = map_session_event(evt) {
                    if event_tx.send(mapped).is_err() {
                        break;
                    }
                }
            }
        });

        Ok((stream_handle, event_rx))
    }
}

fn sdk_event_label(evt: &SdkStreamEvent) -> &'static str {
    match evt {
        SdkStreamEvent::Message(_) => "message",
        SdkStreamEvent::PositionOpened { .. } => "position_opened",
        SdkStreamEvent::PositionClosed { .. } => "position_closed",
        SdkStreamEvent::ExitSignalWithTx { .. } => "exit_signal_with_tx",
        SdkStreamEvent::PnlUpdate { .. } => "pnl_update",
        SdkStreamEvent::LiquiditySnapshot { .. } => "liquidity_snapshot",
        SdkStreamEvent::TradeTick { .. } => "trade_tick",
        SdkStreamEvent::MirrorBuySignal { .. } => "mirror_buy_signal",
        SdkStreamEvent::MirrorBuyFailed { .. } => "mirror_buy_failed",
        SdkStreamEvent::MirrorWalletAutoDisabled { .. } => "mirror_wallet_auto_disabled",
    }
}

fn map_session_event(evt: SdkStreamEvent) -> Option<StreamEvent> {
    info!(event = "stream_session_event_received", variant = sdk_event_label(&evt));
    match evt {
        SdkStreamEvent::Message(msg) => map_server_event(msg),
        SdkStreamEvent::PositionOpened { handle, message } => {
            debug!(
                event = "stream_position_opened_mapped",
                position_id = handle.position_id,
                mint = %handle.mint,
                tokens = handle.tokens
            );
            map_server_event(message)
        }
        SdkStreamEvent::PositionClosed { handle, message } => {
            debug!(
                event = "stream_position_closed_mapped",
                position_id = handle.as_ref().map(|h| h.position_id).unwrap_or(0),
                mint = %handle.as_ref().map(|h| h.mint.as_str()).unwrap_or("unknown")
            );
            map_server_event(message)
        }
        SdkStreamEvent::ExitSignalWithTx { handle, message } => {
            if let Some(h) = &handle {
                info!(
                    event = "stream_exit_signal_mapped",
                    position_id = h.position_id,
                    mint = %h.mint
                );
            }
            map_server_event(message)
        }
        SdkStreamEvent::PnlUpdate { handle, message } => {
            if let (Some(h), ServerMessage::PnlUpdate { profit_units, proceeds_units, .. }) =
                (handle, &message)
            {
                Some(StreamEvent::PnlUpdate {
                    mint: h.mint.clone(),
                    profit_units: *profit_units,
                    proceeds_units: *proceeds_units,
                })
            } else {
                None
            }
        }
        SdkStreamEvent::LiquiditySnapshot { handle, message } => {
            if let (Some(h), ServerMessage::LiquiditySnapshot { liquidity_trend, bands, .. }) =
                (&handle, &message)
            {
                info!(
                    event = "liquidity_snapshot",
                    position_id = h.position_id,
                    mint = %h.mint,
                    trend = %liquidity_trend,
                    bands = bands.len(),
                );
            }
            None
        }
        SdkStreamEvent::TradeTick { .. } => None,
        SdkStreamEvent::MirrorBuySignal { message } => map_server_event(message),
        SdkStreamEvent::MirrorBuyFailed { message } => map_server_event(message),
        SdkStreamEvent::MirrorWalletAutoDisabled { message } => map_server_event(message),
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
            entry_quote_units,
            market_context,
            slot,
            ..
        } => Some(StreamEvent::PositionOpened {
            position_id,
            mint,
            token_program,
            token_account,
            tokens,
            entry_quote_units,
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
        ServerMessage::LiquiditySnapshot { position_id, liquidity_trend, bands, .. } => {
            info!(
                event = "liquidity_snapshot",
                position_id,
                trend = %liquidity_trend,
                bands = bands.len(),
            );
            None
        }
        ServerMessage::TradeTick { .. } => None,
        ServerMessage::Error { code, message } => {
            warn!(event = "stream_server_error", code = %code, message = %message);
            None
        }
        ServerMessage::MirrorBuySignal { watched_wallet, mint, user_wallet, amount_quote_units, .. } => {
            info!(
                event = "mirror_buy_signal",
                watched_wallet = %watched_wallet,
                mint = %mint,
                user_wallet = %user_wallet,
                amount = amount_quote_units,
            );
            None
        }
        ServerMessage::MirrorBuyFailed { watched_wallet, mint, reason } => {
            warn!(
                event = "mirror_buy_failed",
                watched_wallet = %watched_wallet,
                mint = %mint,
                reason = %reason,
            );
            None
        }
        ServerMessage::MirrorWalletAutoDisabled { watched_wallet, reason, loss_count } => {
            warn!(
                event = "mirror_wallet_auto_disabled",
                watched_wallet = %watched_wallet,
                reason = %reason,
                loss_count,
            );
            None
        }
    }
}
