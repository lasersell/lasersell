use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::{Config, StrategyAmount};
use crate::events::{AppEvent, LogLevel};
use crate::stream::MarketStreamState;
use crate::util::pnl::{lamports_to_sol, lamports_to_sol_signed};
use crate::util::support;
use solana_sdk::pubkey::Pubkey;

const OUTPUT_CAPACITY: usize = 500;
const COMMAND_HISTORY_CAPACITY: usize = 120;
const MINT_ACTIVITY_CAPACITY: usize = 200;
const PNL_HISTORY_CAPACITY: usize = 60;
const RPC_LATENCY_CAPACITY: usize = 200;
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusPane {
    Sessions,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    Detecting,
    Holding,
    SellScheduled,
    Selling,
    Closed,
    Error,
}

impl SessionPhase {
    pub fn label(&self) -> &'static str {
        match self {
            SessionPhase::Detecting => "DETECTING",
            SessionPhase::Holding => "HOLDING",
            SessionPhase::SellScheduled => "SELL SCHEDULED",
            SessionPhase::Selling => "SELLING",
            SessionPhase::Closed => "CLOSED",
            SessionPhase::Error => "ERROR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MintStatus {
    Detected,
    Active,
    SellScheduled,
    Selling,
    Closed,
    Error,
}

impl MintStatus {
    pub fn label(&self) -> &'static str {
        match self {
            MintStatus::Detected => "DETECTED",
            MintStatus::Active => "ACTIVE",
            MintStatus::SellScheduled => "SELL SCHED",
            MintStatus::Selling => "SELLING",
            MintStatus::Closed => "CLOSED",
            MintStatus::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MintRow {
    pub mint: Pubkey,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub status: MintStatus,
    pub note: String,
    pub is_active: bool,
    pub last_updated: Instant,
    pub token_program: Option<Pubkey>,
    pub token_account: Option<Pubkey>,
}

impl MintRow {
    fn new(mint: Pubkey) -> Self {
        Self {
            mint,
            name: None,
            symbol: None,
            status: MintStatus::Detected,
            note: String::new(),
            is_active: false,
            last_updated: Instant::now(),
            token_program: None,
            token_account: None,
        }
    }
}

pub struct SessionView {
    #[allow(dead_code)]
    pub mint: Pubkey,
    pub token_program: Pubkey,
    pub token_account: Option<Pubkey>,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub started_at: Option<SystemTime>,
    pub last_session_update: Option<Instant>,
    pub cost_basis_lamports: Option<u64>,
    pub requested_lamports: Option<u64>,
    pub position_tokens: Option<u64>,
    pub fee_bps: u64,
    pub p95_down_per_slot: u64,
    pub curve_complete: Option<bool>,
    pub last_proceeds_lamports: Option<u64>,
    pub last_pnl_lamports: Option<i128>,
    pub pnl_history: VecDeque<i64>,
    pub scheduled_profit_lamports: Option<i64>,
    pub sell_reason: Option<String>,
    pub phase: SessionPhase,
    pub last_sell_attempt_slippage_bps: Option<u16>,
    pub last_sell_attempt: Option<usize>,
    pub retry_count: usize,
    pub last_retry_error: Option<String>,
    pub last_signature: Option<String>,
    pub last_error: Option<String>,
    pub market_label: Option<String>,
    pub quote_mint: Option<Pubkey>,
    pub quote_label: Option<String>,
    pub stream_state: Option<Arc<dyn MarketStreamState>>,
    pub closed: bool,
}

impl SessionView {
    fn new(mint: Pubkey, token_program: Pubkey, started_at: Option<SystemTime>) -> Self {
        Self {
            mint,
            token_program,
            token_account: None,
            name: None,
            symbol: None,
            started_at,
            last_session_update: Some(Instant::now()),
            cost_basis_lamports: None,
            requested_lamports: None,
            position_tokens: None,
            fee_bps: 0,
            p95_down_per_slot: 0,
            curve_complete: None,
            last_proceeds_lamports: None,
            last_pnl_lamports: None,
            pnl_history: VecDeque::with_capacity(PNL_HISTORY_CAPACITY),
            scheduled_profit_lamports: None,
            sell_reason: None,
            phase: SessionPhase::Detecting,
            last_sell_attempt_slippage_bps: None,
            last_sell_attempt: None,
            retry_count: 0,
            last_retry_error: None,
            last_signature: None,
            last_error: None,
            market_label: None,
            quote_mint: None,
            quote_label: None,
            stream_state: None,
            closed: false,
        }
    }

    #[allow(dead_code)]
    pub fn pnl_sol(&self) -> Option<f64> {
        self.last_pnl_lamports
            .map(|value| lamports_to_sol_signed(value))
    }

    #[allow(dead_code)]
    pub fn proceeds_sol(&self) -> Option<f64> {
        self.last_proceeds_lamports.map(lamports_to_sol)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputLevel {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub enum OutputKind {
    Command,
    Reply(OutputLevel),
}

#[derive(Clone, Debug)]
pub struct OutputEntry {
    pub kind: OutputKind,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct RpcMetrics {
    pub total: u64,
    pub errors: u64,
    pub latest_ms: Option<u64>,
    pub sum_ms: u128,
    pub samples: VecDeque<u64>,
}

impl RpcMetrics {
    fn record(&mut self, duration_ms: u64, ok: bool) {
        self.total = self.total.saturating_add(1);
        if !ok {
            self.errors = self.errors.saturating_add(1);
        }
        self.latest_ms = Some(duration_ms);
        self.sum_ms = self.sum_ms.saturating_add(duration_ms as u128);
        self.samples.push_back(duration_ms);
        if self.samples.len() > RPC_LATENCY_CAPACITY {
            self.samples.pop_front();
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExitCounts {
    pub target: u64,
    pub stop_loss: u64,
    pub timeout: u64,
    pub manual: u64,
    pub other: u64,
}

#[derive(Debug)]
pub enum PendingConfirm {
    RequestExitSignal { mint: Pubkey, started: Instant },
}

pub struct UiState {
    pub config: Arc<Config>,
    pub config_path: PathBuf,
    pub last_activity: Instant,
    pub version: Option<String>,
    pub devnet: bool,
    pub wallet_pubkey: Option<Pubkey>,
    pub balance_lamports: Option<u64>,
    pub usd1_balance_base_units: Option<u64>,
    pub paused: bool,
    pub rpc_metrics: RpcMetrics,
    pub solana_ws_connected: bool,
    pub detected_mints: u64,
    pub filtered_mints: u64,
    pub sessions_started: u64,
    pub sessions_active: u64,
    pub sessions_closed: u64,
    pub sells_completed: u64,
    pub exit_counts: ExitCounts,
    pub expected_pnl_lamports: i128,
    pub sessions: HashMap<Pubkey, SessionView>,
    pub session_order: Vec<Pubkey>,
    pub mint_activity: HashMap<Pubkey, MintRow>,
    pub selected_mint: Option<Pubkey>,
    pub output: VecDeque<OutputEntry>,
    pub mouse_capture: bool,
    pub focus: FocusPane,
    pub command_buffer: String,
    pub command_history: VecDeque<String>,
    pub command_history_cursor: Option<usize>,
    pub pending_confirm: Option<PendingConfirm>,
    pub should_quit: bool,
    pub details_expanded: bool,
}

impl UiState {
    pub fn new(config: Arc<Config>, config_path: PathBuf) -> Self {
        let now = Instant::now();
        Self {
            config,
            config_path,
            last_activity: now,
            version: None,
            devnet: false,
            wallet_pubkey: None,
            balance_lamports: None,
            usd1_balance_base_units: None,
            paused: false,
            rpc_metrics: RpcMetrics::default(),
            solana_ws_connected: false,
            detected_mints: 0,
            filtered_mints: 0,
            sessions_started: 0,
            sessions_active: 0,
            sessions_closed: 0,
            sells_completed: 0,
            exit_counts: ExitCounts::default(),
            expected_pnl_lamports: 0,
            sessions: HashMap::new(),
            session_order: Vec::new(),
            mint_activity: HashMap::new(),
            selected_mint: None,
            output: VecDeque::with_capacity(OUTPUT_CAPACITY),
            mouse_capture: true,
            focus: FocusPane::Command,
            command_buffer: String::new(),
            command_history: VecDeque::with_capacity(COMMAND_HISTORY_CAPACITY),
            command_history_cursor: None,
            pending_confirm: None,
            should_quit: false,
            details_expanded: false,
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::Sessions => FocusPane::Command,
            FocusPane::Command => FocusPane::Sessions,
        };
    }

    pub fn push_output_command(&mut self, cmd: &str) {
        let message = cmd.trim();
        if message.is_empty() {
            return;
        }
        self.push_output(OutputEntry {
            kind: OutputKind::Command,
            message: message.to_string(),
        });
    }

    pub fn push_output_reply(&mut self, level: OutputLevel, msg: impl Into<String>) {
        let message = msg.into();
        let message = if level == OutputLevel::Error {
            support::with_support_hint(message)
        } else {
            message
        };
        self.push_output(OutputEntry {
            kind: OutputKind::Reply(level),
            message,
        });
    }

    pub fn clear_output(&mut self) {
        self.output.clear();
    }

    fn push_output(&mut self, entry: OutputEntry) {
        self.output.push_back(entry);
        if self.output.len() > OUTPUT_CAPACITY {
            self.output.pop_front();
        }
    }

    pub fn push_history(&mut self, entry: String) {
        if entry.trim().is_empty() {
            return;
        }
        if self
            .command_history
            .back()
            .map(|v| v == &entry)
            .unwrap_or(false)
        {
            return;
        }
        self.command_history.push_back(entry);
        if self.command_history.len() > COMMAND_HISTORY_CAPACITY {
            self.command_history.pop_front();
        }
    }

    pub fn mint_keys_sorted(&self) -> Vec<Pubkey> {
        let mut rows: Vec<&MintRow> = self.mint_activity.values().collect();
        rows.sort_by(|a, b| match (a.is_active, b.is_active) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => b.last_updated.cmp(&a.last_updated),
        });
        rows.iter().map(|row| row.mint).collect()
    }

    pub fn select_next_mint(&mut self) {
        let mints = self.mint_keys_sorted();
        if mints.is_empty() {
            self.selected_mint = None;
            return;
        }
        let idx = self
            .selected_mint
            .and_then(|mint| mints.iter().position(|m| *m == mint))
            .unwrap_or(0);
        let next_idx = (idx + 1).min(mints.len() - 1);
        self.selected_mint = Some(mints[next_idx]);
    }

    pub fn select_prev_mint(&mut self) {
        let mints = self.mint_keys_sorted();
        if mints.is_empty() {
            self.selected_mint = None;
            return;
        }
        let idx = self
            .selected_mint
            .and_then(|mint| mints.iter().position(|m| *m == mint))
            .unwrap_or(0);
        let prev_idx = idx.saturating_sub(1);
        self.selected_mint = Some(mints[prev_idx]);
    }

    fn ensure_selection(&mut self) {
        let mints = self.mint_keys_sorted();
        if mints.is_empty() {
            self.selected_mint = None;
            return;
        }
        if let Some(selected) = self.selected_mint {
            if mints.iter().any(|mint| *mint == selected) {
                return;
            }
        }
        self.selected_mint = Some(mints[0]);
    }

    pub fn pending_sessions(&self) -> u64 {
        let detected = self.detected_mints as i64;
        let filtered = self.filtered_mints as i64;
        let started = self.sessions_started as i64;
        let pending = detected - filtered - started;
        pending.max(0) as u64
    }

    fn update_mint_row<F>(&mut self, mint: Pubkey, updater: F)
    where
        F: FnOnce(&mut MintRow),
    {
        let entry = self
            .mint_activity
            .entry(mint)
            .or_insert_with(|| MintRow::new(mint));
        entry.last_updated = Instant::now();
        updater(entry);
        if self.selected_mint.is_none() {
            self.selected_mint = Some(mint);
        }
        self.prune_mint_activity();
    }

    fn prune_mint_activity(&mut self) {
        if self.mint_activity.len() <= MINT_ACTIVITY_CAPACITY {
            return;
        }

        let mut rows: Vec<(Pubkey, Instant)> = self
            .mint_activity
            .iter()
            .map(|(mint, row)| (*mint, row.last_updated))
            .collect();
        rows.sort_by(|a, b| a.1.cmp(&b.1));

        let excess = self.mint_activity.len() - MINT_ACTIVITY_CAPACITY;
        for (mint, _) in rows.into_iter().take(excess) {
            self.mint_activity.remove(&mint);
        }

        if let Some(selected) = self.selected_mint {
            if !self.mint_activity.contains_key(&selected) {
                self.ensure_selection();
            }
        }
    }

    pub fn apply_event(&mut self, event: AppEvent) {
        self.last_activity = Instant::now();
        match event {
            AppEvent::Startup {
                version,
                devnet,
                wallet_pubkey,
            } => {
                self.version = Some(version);
                self.devnet = devnet;
                self.wallet_pubkey = Some(wallet_pubkey);
            }
            AppEvent::BalanceUpdate { lamports } => {
                self.balance_lamports = Some(lamports);
            }
            AppEvent::Usd1BalanceUpdate { base_units } => {
                self.usd1_balance_base_units = Some(base_units);
            }
            AppEvent::RpcMetric { duration_ms, ok } => {
                self.rpc_metrics.record(duration_ms, ok);
            }
            AppEvent::SolanaWsStatus { connected } => {
                self.solana_ws_connected = connected;
            }
            AppEvent::MintDetected {
                mint,
                token_account,
                ..
            } => {
                self.detected_mints = self.detected_mints.saturating_add(1);
                if let Some(session) = self.sessions.get_mut(&mint) {
                    session.token_account = Some(token_account);
                }
                self.update_mint_row(mint, |row| {
                    row.status = MintStatus::Detected;
                    row.note = "Detected".to_string();
                    row.is_active = false;
                    row.token_account = Some(token_account);
                });
            }
            AppEvent::SessionStarted {
                mint,
                token_program,
                started_at_ms,
            } => {
                if !self.sessions.contains_key(&mint) {
                    self.sessions_started = self.sessions_started.saturating_add(1);
                    self.sessions_active = self.sessions_active.saturating_add(1);
                    self.session_order.push(mint);
                }
                let started_at = UNIX_EPOCH.checked_add(Duration::from_millis(started_at_ms));
                let session = self
                    .sessions
                    .entry(mint)
                    .or_insert_with(|| SessionView::new(mint, token_program, started_at));
                session.token_program = token_program;
                session.started_at = started_at;
                session.phase = SessionPhase::Detecting;
                session.closed = false;
                session.last_session_update = Some(Instant::now());
                session.last_sell_attempt = None;
                session.last_sell_attempt_slippage_bps = None;
                session.retry_count = 0;
                session.last_retry_error = None;
                session.last_signature = None;
                if session.token_account.is_none() {
                    session.token_account = self
                        .mint_activity
                        .get(&mint)
                        .and_then(|row| row.token_account);
                }
                self.update_mint_row(mint, |row| {
                    row.status = MintStatus::Active;
                    row.note = "Session started".to_string();
                    row.is_active = true;
                    row.token_program = Some(token_program);
                });
                self.ensure_selection();
            }
            AppEvent::SessionStreamState { mint, stream_state } => {
                if let Some(session) = self.sessions.get_mut(&mint) {
                    let market_label = stream_state.market_type().as_str().to_string();
                    session.stream_state = Some(stream_state);
                    session.market_label = Some(market_label);
                }
            }
            AppEvent::PositionTokensUpdated { mint, tokens } => {
                if let Some(session) = self.sessions.get_mut(&mint) {
                    session.position_tokens = Some(tokens);
                    if tokens > 0 && matches!(session.phase, SessionPhase::Detecting) {
                        session.phase = SessionPhase::Holding;
                    }
                    session.last_session_update = Some(Instant::now());
                }
                self.update_mint_row(mint, |row| {
                    row.note = "Position updated".to_string();
                });
            }
            AppEvent::SellScheduled {
                mint,
                reason,
                profit_lamports,
            } => {
                if let Some(session) = self.sessions.get_mut(&mint) {
                    session.phase = SessionPhase::SellScheduled;
                    session.sell_reason = Some(reason.clone());
                    session.scheduled_profit_lamports = Some(profit_lamports);
                    session.last_session_update = Some(Instant::now());
                }
                self.update_mint_row(mint, |row| {
                    row.status = MintStatus::SellScheduled;
                    row.note = reason;
                    row.is_active = true;
                });
                self.expected_pnl_lamports += profit_lamports as i128;
            }
            AppEvent::SellAttempt {
                mint,
                attempt,
                slippage_bps,
            } => {
                if let Some(session) = self.sessions.get_mut(&mint) {
                    session.phase = SessionPhase::Selling;
                    session.last_sell_attempt = Some(attempt);
                    session.last_sell_attempt_slippage_bps = Some(slippage_bps);
                    session.last_session_update = Some(Instant::now());
                }
                self.update_mint_row(mint, |row| {
                    row.status = MintStatus::Selling;
                    row.note = format!("Attempt {attempt}");
                    row.is_active = true;
                });
            }
            AppEvent::SellRetry {
                mint,
                attempt,
                phase,
                error,
            } => {
                let error = support::with_support_hint(error);
                if let Some(session) = self.sessions.get_mut(&mint) {
                    session.phase = SessionPhase::Selling;
                    session.last_sell_attempt = Some(attempt);
                    session.retry_count = session.retry_count.saturating_add(1);
                    session.last_retry_error = Some(format!("{phase}: {error}"));
                    session.last_session_update = Some(Instant::now());
                }
                self.update_mint_row(mint, |row| {
                    row.status = MintStatus::Selling;
                    row.note = format!("Retry {attempt}");
                    row.is_active = true;
                });
            }
            AppEvent::SellComplete {
                mint,
                reason,
                slippage_bps,
                signature,
                ..
            } => {
                let mut closed = false;
                if let Some(session) = self.sessions.get_mut(&mint) {
                    session.phase = SessionPhase::Closed;
                    session.sell_reason = Some(reason.clone());
                    session.last_sell_attempt_slippage_bps = Some(slippage_bps);
                    session.last_signature = Some(signature);
                    session.last_retry_error = None;
                    session.last_session_update = Some(Instant::now());
                    if !session.closed {
                        session.closed = true;
                        closed = true;
                    }
                }
                self.update_mint_row(mint, |row| {
                    row.status = MintStatus::Closed;
                    row.note = reason.clone();
                    row.is_active = false;
                });
                if closed {
                    self.sessions_active = self.sessions_active.saturating_sub(1);
                    self.sessions_closed = self.sessions_closed.saturating_add(1);
                    self.ensure_selection();
                }
                self.sells_completed = self.sells_completed.saturating_add(1);
                self.track_exit_reason(&reason);
            }
            AppEvent::SessionClosed { mint } => {
                let mut closed = false;
                if let Some(session) = self.sessions.get_mut(&mint) {
                    session.phase = SessionPhase::Closed;
                    session.last_session_update = Some(Instant::now());
                    if !session.closed {
                        session.closed = true;
                        closed = true;
                    }
                }
                self.update_mint_row(mint, |row| {
                    row.status = MintStatus::Closed;
                    row.note = "Closed".to_string();
                    row.is_active = false;
                });
                if closed {
                    self.sessions_active = self.sessions_active.saturating_sub(1);
                    self.sessions_closed = self.sessions_closed.saturating_add(1);
                    self.ensure_selection();
                }
            }
            AppEvent::SessionError { mint, error } => {
                let error = support::with_support_hint(error);
                let mut closed = false;
                if let Some(session) = self.sessions.get_mut(&mint) {
                    session.phase = SessionPhase::Error;
                    session.last_error = Some(error.clone());
                    session.last_session_update = Some(Instant::now());
                    if !session.closed {
                        session.closed = true;
                        closed = true;
                    }
                }
                self.update_mint_row(mint, |row| {
                    row.status = MintStatus::Error;
                    row.note = error;
                    row.is_active = false;
                });
                if closed {
                    self.sessions_active = self.sessions_active.saturating_sub(1);
                    self.sessions_closed = self.sessions_closed.saturating_add(1);
                    self.ensure_selection();
                }
            }
            AppEvent::PauseState { paused } => {
                self.paused = paused;
            }
            AppEvent::Heartbeat => {}
            AppEvent::LogLine {
                level,
                message,
                event,
            } => {
                if should_surface_log(level, event.as_deref()) {
                    let output_level = match level {
                        LogLevel::Error => OutputLevel::Error,
                        LogLevel::Warn => OutputLevel::Warn,
                        _ => OutputLevel::Info,
                    };
                    self.push_output_reply(output_level, message);
                }
            }
        }
    }

    fn track_exit_reason(&mut self, reason: &str) {
        match reason {
            "target" | "profit" => self.exit_counts.target += 1,
            "stop_loss" => self.exit_counts.stop_loss += 1,
            "timeout" => self.exit_counts.timeout += 1,
            "manual" => self.exit_counts.manual += 1,
            _ => self.exit_counts.other += 1,
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        if let Some(confirm) = &self.pending_confirm {
            let expired = match confirm {
                PendingConfirm::RequestExitSignal { started, .. } => {
                    now.duration_since(*started) > CONFIRM_TIMEOUT
                }
            };
            if expired {
                self.pending_confirm = None;
                self.push_output_reply(OutputLevel::Warn, "Confirmation timed out.");
            }
        }
    }

    pub async fn update_selected_metrics(&mut self) {
        let Some(mint) = self.selected_mint else {
            return;
        };
        let stream_state = match self
            .sessions
            .get(&mint)
            .and_then(|session| session.stream_state.clone())
        {
            Some(stream_state) => stream_state,
            None => return,
        };

        let curve = stream_state.latest_curve().await;
        let fee_bps = stream_state.latest_fee_bps();
        let p95_down = stream_state.p95_down_per_slot();
        let tokens = stream_state.position_tokens().await.unwrap_or(0);

        let Some(session) = self.sessions.get_mut(&mint) else {
            return;
        };

        session.fee_bps = fee_bps;
        session.p95_down_per_slot = p95_down;
        session.curve_complete = curve.as_ref().map(|value| value.complete);
        if tokens > 0 {
            session.position_tokens = Some(tokens);
            if matches!(session.phase, SessionPhase::Detecting) {
                session.phase = SessionPhase::Holding;
            }
        }

        if tokens > 0 {
            if let Some(proceeds) = stream_state.quote_sell_proceeds(tokens).await {
                session.last_proceeds_lamports = Some(proceeds);
                if let Some(cost) = session.cost_basis_lamports {
                    let pnl = proceeds as i128 - cost as i128;
                    session.last_pnl_lamports = Some(pnl);
                    let entry = pnl.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
                    session.pnl_history.push_back(entry);
                    if session.pnl_history.len() > PNL_HISTORY_CAPACITY {
                        session.pnl_history.pop_front();
                    }
                }
            }
        }
    }

    pub fn session_timeout_remaining(&self, session: &SessionView) -> Option<Duration> {
        if self.config.strategy.deadline_timeout_sec == 0 {
            return None;
        }
        let started_at = session.started_at?;
        let timeout = Duration::from_secs(self.config.strategy.deadline_timeout_sec);
        let now = SystemTime::now();
        let elapsed = now.duration_since(started_at).ok()?;
        if elapsed >= timeout {
            Some(Duration::from_secs(0))
        } else {
            Some(timeout - elapsed)
        }
    }

    pub fn format_strategy_amount(amount: &StrategyAmount) -> String {
        match amount {
            StrategyAmount::Percent(value) => format!("{value:.2}%"),
        }
    }
}

fn should_surface_log(level: LogLevel, event: Option<&str>) -> bool {
    if matches!(level, LogLevel::Warn | LogLevel::Error) {
        return true;
    }
    let Some(event) = event else {
        return false;
    };
    matches!(
        event,
        "solana_ws_connected" | "mint_detected" | "cost_basis" | "sell_scheduled" | "sell_complete"
    )
}

pub fn short_pubkey(key: &Pubkey) -> String {
    let s = key.to_string();
    if s.len() <= 8 {
        return s;
    }
    let prefix = &s[..4];
    let suffix = &s[s.len() - 4..];
    format!("{prefix}...{suffix}")
}

pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}
