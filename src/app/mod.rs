use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use lasersell_sdk::stream::client::StrategyConfigBuilder;
use lasersell_sdk::stream::proto::{
    AutoBuyConfigMsg, MarketContextMsg, MirrorConfigMsg, StrategyConfigMsg, TakeProfitLevelMsg,
    WatchWalletEntryMsg,
};
use lasersell_sdk::tx::{SendTarget, TxSubmitError};
use parking_lot::RwLock as ParkingRwLock;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::config::{Config, SellConfig, StrategyConfig, WatchWalletConfig};
use crate::events::{emit, AppCommand, AppEvent};
use crate::market::context_from_msg::market_context_from_msg;
use crate::market::{usd1_mint, MarketContext};
use crate::network::{rpc_result, StreamClient, StreamEvent, StreamHandle};
use crate::stream::InMemoryMarketStreamState;
use crate::tx::{send_tx, sign_unsigned_tx};

const HEARTBEAT_INTERVAL_SECS: u64 = 1;
const AUTOSELL_REFRESH_TIMEOUT_MS: u64 = 1_500;
const BALANCE_POLL_SECS: u64 = 5;
const BALANCE_POLL_PUBLIC_RPC_SECS: u64 = 15;

fn balance_poll_interval(rpc_url: &str) -> Duration {
    if rpc_url.trim().contains("publicnode.com") || rpc_url.trim().contains("api.mainnet-beta.solana.com") {
        Duration::from_secs(BALANCE_POLL_PUBLIC_RPC_SECS)
    } else {
        Duration::from_secs(BALANCE_POLL_SECS)
    }
}

#[derive(Clone, Debug)]
struct PositionSnapshot {
    position_id: u64,
    token_program: Option<String>,
    tokens: u64,
}

enum LoopControl {
    Break,
    DropCommands,
}

struct AppEngine {
    runtime_sell: Arc<ParkingRwLock<SellConfig>>,
    keypair_bytes: [u8; 64],
    rpc_http: reqwest::Client,
    rpc_url: String,
    send_target: SendTarget,
    stream_handle: Arc<StreamHandle>,
    market_contexts: Arc<ParkingRwLock<HashMap<Pubkey, MarketContext>>>,
    stream_states: Arc<ParkingRwLock<HashMap<Pubkey, Arc<InMemoryMarketStreamState>>>>,
    position_snapshots: Arc<ParkingRwLock<HashMap<Pubkey, PositionSnapshot>>>,
    in_flight_auto_sells: Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<String>>>>,
}

pub async fn run(
    cfg: Config,
    keypair: Keypair,
    mut cmd_rx: Option<mpsc::UnboundedReceiver<AppCommand>>,
) -> Result<()> {
    let (mut engine, mut evt_rx) = AppEngine::new(cfg, keypair).await?;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));

    loop {
        tokio::select! {
            maybe_evt = evt_rx.recv() => {
                let Some(evt) = maybe_evt else {
                    break;
                };
                engine.handle_stream_event(evt).await?;
            }
            cmd = async {
                if let Some(rx) = cmd_rx.as_mut() {
                    rx.recv().await
                } else {
                    std::future::pending::<Option<AppCommand>>().await
                }
            } => {
                match engine.handle_user_command(cmd).await? {
                    LoopControl::Break => break,
                    LoopControl::DropCommands => {
                        cmd_rx = None;
                    }
                }
            }
            _ = heartbeat.tick() => {
                engine.handle_heartbeat();
            }
        }
    }

    Ok(())
}

impl AppEngine {
    async fn new(
        cfg: Config,
        keypair: Keypair,
    ) -> Result<(Self, mpsc::UnboundedReceiver<StreamEvent>)> {
        let runtime_sell = Arc::new(ParkingRwLock::new(cfg.sell.clone()));
        let wallet_pubkey = cfg.wallet_pubkey(&keypair)?;
        let keypair_bytes = keypair.to_bytes();
        let rpc_http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(cfg.rpc_connect_timeout())
            .timeout(cfg.rpc_request_timeout())
            .build()?;
        let rpc_url = cfg.http_rpc_url();
        let send_target = cfg.resolve_send_target()?;

        let balance_http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()?;
        spawn_wallet_balance_poller(balance_http.clone(), rpc_url.clone(), wallet_pubkey);
        spawn_usd1_balance_poller(balance_http, rpc_url.clone(), wallet_pubkey);

        let stream_send_mode = Some(cfg.send_mode_str().to_string());
        let (watch_wallets, mirror_config) = if cfg.mirror.enabled {
            (
                build_watch_wallet_entries(&cfg.watch_wallets, &wallet_pubkey.to_string()),
                build_mirror_config(&cfg.mirror),
            )
        } else {
            (Vec::new(), None)
        };
        let stream_client = StreamClient::new(
            cfg.account.api_key.clone(),
            cfg.account.local,
            wallet_pubkey.to_string(),
            strategy_to_msg(&cfg.strategy),
            cfg.strategy.deadline_timeout_sec,
            stream_send_mode,
            cfg.account.tip_lamports,
            watch_wallets,
            mirror_config,
        );
        let (stream_handle, evt_rx) = stream_client.connect(&keypair).await?;
        let stream_handle = Arc::new(stream_handle);

        let market_contexts = Arc::new(ParkingRwLock::new(HashMap::<Pubkey, MarketContext>::new()));
        let stream_states = Arc::new(ParkingRwLock::new(HashMap::<
            Pubkey,
            Arc<InMemoryMarketStreamState>,
        >::new()));
        let position_snapshots = Arc::new(ParkingRwLock::new(
            HashMap::<Pubkey, PositionSnapshot>::new(),
        ));
        let in_flight_auto_sells = Arc::new(Mutex::new(HashMap::<
            u64,
            mpsc::UnboundedSender<String>,
        >::new()));

        Ok((
            Self {
                runtime_sell,
                keypair_bytes,
                rpc_http,
                rpc_url,
                send_target,
                stream_handle,
                market_contexts,
                stream_states,
                position_snapshots,
                in_flight_auto_sells,
            },
            evt_rx,
        ))
    }

    async fn handle_stream_event(&mut self, evt: StreamEvent) -> Result<()> {
        debug!(event = "app_stream_event", variant = stream_event_label(&evt));
        match evt {
            StreamEvent::ConnectionStatus { connected } => {
                emit(AppEvent::SolanaWsStatus { connected });
            }
            StreamEvent::BalanceUpdate {
                mint,
                token_program,
                token_account: _token_account,
                tokens,
                slot: _slot,
            } => self.handle_balance_update(mint, token_program, tokens),
            StreamEvent::PositionOpened {
                position_id,
                mint,
                token_program,
                token_account,
                tokens,
                entry_quote_units,
                slot,
                market_context,
            } => self.handle_position_opened(
                position_id,
                mint,
                token_program,
                token_account,
                tokens,
                entry_quote_units,
                slot,
                market_context,
            ),
            StreamEvent::PositionClosed {
                position_id,
                mint,
                token_account: _token_account,
                reason,
                slot,
            } => {
                self.handle_position_closed(position_id, mint, reason, slot)
                    .await;
            }
            StreamEvent::ExitSignalWithTx {
                position_id,
                mint,
                token_program,
                token_account: _token_account,
                position_tokens,
                profit_units,
                reason,
                triggered_at_ms: _triggered_at_ms,
                market_context,
                unsigned_tx_b64,
            } => {
                self.handle_exit_signal_with_tx(
                    position_id,
                    mint,
                    token_program,
                    position_tokens,
                    profit_units,
                    reason,
                    market_context,
                    unsigned_tx_b64,
                )
                .await?;
            }
            StreamEvent::PnlUpdate {
                mint,
                profit_units,
                proceeds_units,
            } => {
                if let Ok(mint) = Pubkey::from_str(&mint) {
                    emit(AppEvent::PnlUpdate {
                        mint,
                        profit_lamports: profit_units,
                        proceeds_lamports: proceeds_units,
                    });
                }
            }
        }
        Ok(())
    }

    async fn handle_user_command(&mut self, cmd: Option<AppCommand>) -> Result<LoopControl> {
        match cmd {
            Some(AppCommand::Quit) => Ok(LoopControl::Break),
            None => Ok(LoopControl::DropCommands),
        }
    }

    fn handle_heartbeat(&self) {
        emit(AppEvent::Heartbeat);
    }

    fn handle_balance_update(&self, mint: String, token_program: Option<String>, tokens: u64) {
        debug!(event = "app_balance_update", mint = %mint, tokens);
        if let Ok(mint) = Pubkey::from_str(&mint) {
            {
                let mut snapshots = self.position_snapshots.write();
                let entry = snapshots.entry(mint).or_insert(PositionSnapshot {
                    position_id: 0,
                    token_program: None,
                    tokens: 0,
                });
                if token_program.is_some() {
                    entry.token_program = token_program;
                }
                entry.tokens = tokens;
            }
            if let Some(stream_state) = self.stream_states.read().get(&mint).cloned() {
                stream_state.set_position_tokens(Some(tokens));
            }
            emit(AppEvent::PositionTokensUpdated { mint, tokens });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_position_opened(
        &self,
        position_id: u64,
        mint: String,
        token_program: Option<String>,
        _token_account: String,
        tokens: u64,
        entry_quote_units: u64,
        _slot: u64,
        market_context: Option<MarketContextMsg>,
    ) {
        info!(event = "app_position_opened", position_id, mint = %mint, tokens);
        if let Ok(mint) = Pubkey::from_str(&mint) {
            let parsed_context = apply_market_context_update(
                mint,
                market_context,
                self.market_contexts.as_ref(),
            );
            let context_for_state = parsed_context
                .or_else(|| self.market_contexts.read().get(&mint).copied());
            upsert_market_stream_state(
                self.stream_states.as_ref(),
                mint,
                context_for_state.as_ref(),
                Some(tokens),
            );
            self.position_snapshots.write().insert(
                mint,
                PositionSnapshot {
                    position_id,
                    token_program,
                    tokens,
                },
            );
            emit(AppEvent::SessionStarted { mint });
            emit(AppEvent::MintDetected { mint });
            emit(AppEvent::PositionTokensUpdated { mint, tokens });
            if entry_quote_units > 0 {
                emit(AppEvent::CostBasisSet {
                    mint,
                    cost_basis_lamports: entry_quote_units,
                });
            }
        }
    }

    async fn handle_position_closed(
        &self,
        position_id: u64,
        mint: String,
        reason: String,
        slot: u64,
    ) {
        if let Ok(mint) = Pubkey::from_str(&mint) {
            {
                let mut snapshots = self.position_snapshots.write();
                let should_remove = snapshots
                    .get(&mint)
                    .map(|snapshot| {
                        snapshot.position_id == position_id || snapshot.position_id == 0
                    })
                    .unwrap_or(false);
                if should_remove {
                    snapshots.remove(&mint);
                }
            }
            self.market_contexts.write().remove(&mint);
            self.stream_states.write().remove(&mint);
            self.in_flight_auto_sells.lock().await.remove(&position_id);
            debug!(
                event = "position_closed",
                mint = %mint,
                position_id,
                reason = %reason,
                slot
            );
            emit(AppEvent::SessionClosed { mint });
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_exit_signal_with_tx(
        &self,
        position_id: u64,
        mint: String,
        token_program: Option<String>,
        position_tokens: u64,
        profit_units: i64,
        reason: String,
        market_context: Option<MarketContextMsg>,
        unsigned_tx_b64: String,
    ) -> Result<()> {
        process_exit_signal_with_tx(
            false, // CLI does not support pause/resume
            position_id,
            mint,
            token_program,
            position_tokens,
            profit_units,
            reason,
            market_context,
            unsigned_tx_b64,
            self.stream_handle.clone(),
            self.rpc_http.clone(),
            self.keypair_bytes,
            self.rpc_url.clone(),
            self.send_target.clone(),
            self.runtime_sell.clone(),
            self.in_flight_auto_sells.clone(),
            self.market_contexts.clone(),
            self.stream_states.clone(),
            self.position_snapshots.clone(),
        )
        .await
    }

}

fn stream_event_label(evt: &StreamEvent) -> &'static str {
    match evt {
        StreamEvent::ConnectionStatus { .. } => "connection_status",
        StreamEvent::BalanceUpdate { .. } => "balance_update",
        StreamEvent::PositionOpened { .. } => "position_opened",
        StreamEvent::PositionClosed { .. } => "position_closed",
        StreamEvent::ExitSignalWithTx { .. } => "exit_signal_with_tx",
        StreamEvent::PnlUpdate { .. } => "pnl_update",
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_exit_signal_with_tx(
    paused: bool,
    position_id: u64,
    mint: String,
    token_program: Option<String>,
    position_tokens: u64,
    profit_units: i64,
    reason: String,
    market_context_msg: Option<MarketContextMsg>,
    unsigned_tx_b64: String,
    stream_handle: Arc<StreamHandle>,
    rpc_http: reqwest::Client,
    keypair_bytes: [u8; 64],
    rpc_url: String,
    send_target: SendTarget,
    runtime_sell: Arc<ParkingRwLock<SellConfig>>,
    in_flight_auto_sells: Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<String>>>>,
    market_contexts: Arc<ParkingRwLock<HashMap<Pubkey, MarketContext>>>,
    stream_states: Arc<ParkingRwLock<HashMap<Pubkey, Arc<InMemoryMarketStreamState>>>>,
    position_snapshots: Arc<ParkingRwLock<HashMap<Pubkey, PositionSnapshot>>>,
) -> Result<()> {
    info!(
        event = "app_exit_signal_processing",
        position_id,
        mint = %mint,
        reason = %reason,
        position_tokens
    );

    let mint_pubkey = match Pubkey::from_str(&mint) {
        Ok(value) => value,
        Err(_) => {
            warn!(event = "exit_signal_invalid_mint", mint = %mint);
            return Ok(());
        }
    };

    let parsed_context = apply_market_context_update(
        mint_pubkey,
        market_context_msg,
        market_contexts.as_ref(),
    );
    let context_for_state = parsed_context
        .or_else(|| market_contexts.read().get(&mint_pubkey).copied());
    upsert_market_stream_state(
        stream_states.as_ref(),
        mint_pubkey,
        context_for_state.as_ref(),
        Some(position_tokens),
    );

    position_snapshots.write().insert(
        mint_pubkey,
        PositionSnapshot {
            position_id,
            token_program: token_program.clone(),
            tokens: position_tokens,
        },
    );

    if paused {
        debug!(event = "app_exit_signal_skipped_paused", mint = %mint);
        return Ok(());
    }

    let mut in_flight = in_flight_auto_sells.lock().await;
    if let Some(existing_tx) = in_flight.get(&position_id) {
        debug!(event = "app_exit_signal_refreshing_inflight", position_id);
        let _ = existing_tx.send(unsigned_tx_b64);
        return Ok(());
    }
    let (refresh_tx, refresh_rx) = mpsc::unbounded_channel::<String>();
    in_flight.insert(position_id, refresh_tx);
    drop(in_flight);

    let runtime_sell = runtime_sell.clone();
    let rpc_http = rpc_http.clone();
    let rpc_url = rpc_url.clone();
    let in_flight_auto_sells = in_flight_auto_sells.clone();
    let position_snapshots = position_snapshots.clone();
    let market_contexts = market_contexts.clone();
    let stream_states = stream_states.clone();
    let stream_handle = stream_handle.clone();
    tokio::spawn(async move {
        let sell_reason = canonical_sell_reason(&reason).to_string();

        emit(AppEvent::SessionStarted { mint: mint_pubkey });
        emit(AppEvent::PositionTokensUpdated {
            mint: mint_pubkey,
            tokens: position_tokens,
        });
        info!(
            event = "sell_scheduled",
            mint = %mint_pubkey,
            reason = %sell_reason,
            profit_lamports = profit_units
        );
        emit(AppEvent::SellScheduled {
            mint: mint_pubkey,
            reason: sell_reason.clone(),
            profit_lamports: profit_units,
        });

        let sell_cfg = runtime_sell.read().clone();
        let result = execute_auto_sell_with_refresh(
            stream_handle,
            refresh_rx,
            rpc_http,
            keypair_bytes,
            rpc_url,
            send_target,
            mint_pubkey,
            position_id,
            sell_cfg,
            unsigned_tx_b64,
        )
        .await;

        match result {
            Ok((signature, slippage_bps)) => {
                info!(
                    event = "sell_complete",
                    mint = %mint_pubkey,
                    signature = %signature,
                    reason = %sell_reason,
                    slippage_bps
                );
                emit(AppEvent::SellComplete {
                    mint: mint_pubkey,
                    signature,
                    reason: sell_reason,
                    slippage_bps,
                });
                emit(AppEvent::SessionClosed { mint: mint_pubkey });
                position_snapshots.write().remove(&mint_pubkey);
                market_contexts.write().remove(&mint_pubkey);
                stream_states.write().remove(&mint_pubkey);
            }
            Err(err) => {
                warn!(
                    event = "autosell_failed",
                    mint = %mint_pubkey,
                    position_id,
                    error = format!("{err:#}")
                );
                warn!(event = "session_error", mint = %mint_pubkey, error = format!("{err:#}"));
                emit(AppEvent::SessionError {
                    mint: mint_pubkey,
                    error: format!("{err:#}"),
                });
            }
        }

        in_flight_auto_sells.lock().await.remove(&position_id);
    });

    Ok(())
}

async fn execute_auto_sell_with_refresh(
    stream_handle: Arc<StreamHandle>,
    mut refresh_rx: mpsc::UnboundedReceiver<String>,
    rpc_http: reqwest::Client,
    keypair_bytes: [u8; 64],
    rpc_url: String,
    send_target: SendTarget,
    mint: Pubkey,
    position_id: u64,
    sell_cfg: SellConfig,
    initial_unsigned_tx_b64: String,
) -> Result<(String, u16)> {
    let keypair = Keypair::try_from(&keypair_bytes[..]).context("decode keypair")?;
    let mut unsigned_tx_b64 = initial_unsigned_tx_b64;
    let mut attempt = 1usize;
    let mut refreshes_used = 0usize;
    let mut slippage_bps = sell_cfg.slippage_pad_bps;

    loop {
        debug!(event = "app_autosell_attempt", mint = %mint, attempt, slippage_bps);
        emit(AppEvent::SellAttempt {
            mint,
            attempt,
            slippage_bps,
        });

        let send_result = async {
            let signed_tx = sign_unsigned_tx(&unsigned_tx_b64, &keypair)?;
            send_tx(
                &rpc_http,
                &rpc_url,
                &signed_tx,
                &send_target,
                Duration::from_secs(sell_cfg.confirm_timeout_sec),
            )
            .await
        }
        .await;

        match send_result {
            Ok(signature) => return Ok((signature, slippage_bps)),
            Err(err) => {
                warn!(event = "app_autosell_attempt_failed", mint = %mint, attempt, error = format!("{err:#}"));
                if refreshes_used >= sell_cfg.max_retries {
                    return Err(anyhow!(
                        "autosell failed for position_id {position_id} after {attempt} attempts: {err:#}"
                    ));
                }
                emit(AppEvent::SellRetry {
                    mint,
                    attempt,
                    phase: classify_sell_retry_phase(&err).to_string(),
                    error: format!("{err:#}"),
                });

                slippage_bps = bumped_slippage_bps(slippage_bps, refreshes_used, &sell_cfg);
                refreshes_used += 1;
                debug!(event = "app_autosell_refresh_requested", mint = %mint, position_id, new_slippage_bps = slippage_bps);
                stream_handle
                    .request_exit_signal(position_id, Some(slippage_bps))
                    .context("request sell refresh over stream")?;
                unsigned_tx_b64 = recv_refreshed_sell_tx(&mut refresh_rx, position_id).await?;
                attempt += 1;
            }
        }
    }
}

fn classify_sell_retry_phase(err: &anyhow::Error) -> &'static str {
    if err.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<TxSubmitError>(),
            Some(TxSubmitError::ConfirmTimeout { .. } | TxSubmitError::TxFailed { .. })
        )
    }) {
        "tx_confirm"
    } else {
        "tx_send"
    }
}

fn canonical_sell_reason(reason: &str) -> &str {
    match reason {
        "target" | "profit" | "target_profit" => "target",
        "stop_loss" => "stop_loss",
        "timeout" | "deadline_timeout" | "deadline" | "sell_now" => "timeout",
        "manual" | "manual_sell" => "manual",
        _ => reason,
    }
}

async fn recv_refreshed_sell_tx(
    refresh_rx: &mut mpsc::UnboundedReceiver<String>,
    position_id: u64,
) -> Result<String> {
    loop {
        let next = tokio::time::timeout(
            Duration::from_millis(AUTOSELL_REFRESH_TIMEOUT_MS),
            refresh_rx.recv(),
        )
        .await
        .map_err(|_| {
            anyhow!("timed out waiting for refreshed sell tx (position_id={position_id})")
        })?;
        let payload =
            next.ok_or_else(|| anyhow!("sell refresh channel closed (position_id={position_id})"))?;
        let trimmed = payload.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
}

fn bumped_slippage_bps(current: u16, refreshes_used: usize, cfg: &SellConfig) -> u16 {
    let bump = if refreshes_used == 0 {
        cfg.slippage_retry_bump_bps_first
    } else {
        cfg.slippage_retry_bump_bps_next
    };
    current.saturating_add(bump).min(cfg.slippage_max_bps)
}

fn strategy_to_msg(strategy: &StrategyConfig) -> StrategyConfigMsg {
    let mut builder = StrategyConfigBuilder::new()
        .target_profit_pct(strategy.target_profit.percent_value())
        .stop_loss_pct(strategy.stop_loss.percent_value())
        .trailing_stop_pct(strategy.trailing_stop.percent_value())
        .sell_on_graduation(strategy.sell_on_graduation)
        .liquidity_guard(strategy.liquidity_guard)
        .breakeven_trail_pct(strategy.breakeven_trail.percent_value());

    if !strategy.take_profit_levels.is_empty() {
        builder = builder.take_profit_levels(
            strategy
                .take_profit_levels
                .iter()
                .map(|l| TakeProfitLevelMsg {
                    profit_pct: l.profit_pct,
                    sell_pct: l.sell_pct,
                    trailing_stop_pct: l.trailing_stop_pct,
                })
                .collect(),
        );
    }

    builder.build()
}

fn apply_market_context_update(
    mint: Pubkey,
    market_context: Option<MarketContextMsg>,
    market_contexts: &ParkingRwLock<HashMap<Pubkey, MarketContext>>,
) -> Option<MarketContext> {
    let msg = market_context?;
    let context = market_context_from_msg(&msg);
    market_contexts.write().insert(mint, context);
    Some(context)
}

fn upsert_market_stream_state(
    stream_states: &ParkingRwLock<HashMap<Pubkey, Arc<InMemoryMarketStreamState>>>,
    mint: Pubkey,
    market_context: Option<&MarketContext>,
    position_tokens: Option<u64>,
) {
    let mut states = stream_states.write();

    let state = match market_context {
        Some(context) => {
            let replace = states
                .get(&mint)
                .map(|state| state.market_type_value() != context.market_type)
                .unwrap_or(true);
            if replace {
                let created = Arc::new(InMemoryMarketStreamState::new(context.market_type));
                states.insert(mint, created.clone());
                created
            } else {
                states
                    .get(&mint)
                    .cloned()
                    .expect("stream state exists after replace check")
            }
        }
        None => {
            let Some(existing) = states.get(&mint).cloned() else {
                return;
            };
            existing
        }
    };

    if let Some(tokens) = position_tokens {
        state.set_position_tokens(Some(tokens));
    }
}

/// Derive the Associated Token Account address for a wallet + mint.
fn derive_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    // ATA PDA: seeds = [wallet, token_program, mint], program = ATA program
    const ATA_PROGRAM: Pubkey = solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
    let (ata, _bump) = Pubkey::find_program_address(
        &[
            wallet.as_ref(),
            spl_token::id().as_ref(),
            mint.as_ref(),
        ],
        &ATA_PROGRAM,
    );
    ata
}

fn spawn_wallet_balance_poller(rpc_http: reqwest::Client, rpc_url: String, wallet_pubkey: Pubkey) {
    let poll = balance_poll_interval(&rpc_url);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll);
        loop {
            interval.tick().await;
            match fetch_wallet_balance(&rpc_http, &rpc_url, &wallet_pubkey).await {
                Ok(lamports) => {
                    emit(AppEvent::BalanceUpdate { lamports });
                }
                Err(err) => {
                    warn!(event = "wallet_balance_fetch_error", error = %err);
                }
            }
        }
    });
}

fn spawn_usd1_balance_poller(rpc_http: reqwest::Client, rpc_url: String, wallet_pubkey: Pubkey) {
    let poll = balance_poll_interval(&rpc_url);
    let usd1_ata = derive_ata(&wallet_pubkey, &usd1_mint());
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll);
        loop {
            interval.tick().await;
            match fetch_usd1_balance(&rpc_http, &rpc_url, &usd1_ata).await {
                Ok(base_units) => {
                    emit(AppEvent::Usd1BalanceUpdate { base_units });
                }
                Err(err) => {
                    warn!(event = "usd1_balance_fetch_error", error = %err);
                }
            }
        }
    });
}

async fn fetch_wallet_balance(
    client: &reqwest::Client,
    rpc_url: &str,
    wallet_pubkey: &Pubkey,
) -> Result<u64> {
    let result = rpc_result(
        client,
        rpc_url,
        "getBalance",
        serde_json::json!([
            wallet_pubkey.to_string(),
            {
                "commitment": "processed"
            }
        ]),
    )
    .await?;

    result
        .get("value")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow!("wallet balance missing"))
}

async fn fetch_usd1_balance(
    client: &reqwest::Client,
    rpc_url: &str,
    ata: &Pubkey,
) -> Result<u64> {
    let result = rpc_result(
        client,
        rpc_url,
        "getTokenAccountBalance",
        serde_json::json!([ata.to_string()]),
    )
    .await;

    match result {
        Ok(value) => {
            // getTokenAccountBalance returns { value: { amount: "123", decimals: 6, ... } }
            value
                .get("value")
                .and_then(|v| v.get("amount"))
                .and_then(|a| a.as_str())
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| anyhow!("usd1 token balance missing"))
        }
        Err(e) if e.to_string().contains("could not find account") => Ok(0),
        Err(e) => Err(e),
    }
}

fn build_watch_wallet_entries(
    watch_wallets: &[WatchWalletConfig],
    own_wallet_pubkey: &str,
) -> Vec<WatchWalletEntryMsg> {
    watch_wallets
        .iter()
        .filter(|w| w.enabled)
        .map(|w| {
            let auto_buy = w.auto_buy.as_ref().and_then(|ab| {
                let sol_units = (ab.amount * 1e9) as u64;
                let usd1_units = if ab.amount_usd1 > 0.0 {
                    Some((ab.amount_usd1 * 1e6) as u64)
                } else {
                    None
                };
                if sol_units == 0 && usd1_units.is_none() {
                    return None;
                }
                Some(AutoBuyConfigMsg {
                    wallet_pubkey: own_wallet_pubkey.to_string(),
                    amount_quote_units: sol_units,
                    amount_usd1_units: usd1_units,
                })
            });
            WatchWalletEntryMsg {
                pubkey: w.pubkey.clone(),
                auto_buy,
                mirror_sell: false,
            }
        })
        .collect()
}

fn build_mirror_config(cfg: &crate::config::MirrorConfig) -> Option<MirrorConfigMsg> {
    Some(MirrorConfigMsg {
        max_positions_per_wallet: cfg.max_positions_per_wallet,
        cooldown_sec: cfg.cooldown_sec,
        skip_creator_tokens: cfg.skip_creator_tokens,
        max_active_sol: cfg.max_active_sol,
        buy_slippage_bps: cfg.buy_slippage_bps,
        min_liquidity_sol: cfg.min_liquidity_sol,
        max_entry_drift_pct: cfg.max_entry_drift_pct,
        max_consecutive_losses: cfg.max_consecutive_losses,
    })
}

#[cfg(test)]
mod tests {
    use super::canonical_sell_reason;

    #[test]
    fn canonical_sell_reason_normalizes_deadline_to_timeout() {
        assert_eq!(canonical_sell_reason("deadline"), "timeout");
        assert_eq!(canonical_sell_reason("deadline_timeout"), "timeout");
        assert_eq!(canonical_sell_reason("timeout"), "timeout");
    }
}
