use std::fmt;
use std::sync::{Arc, OnceLock};

use solana_sdk::pubkey::Pubkey;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{field::Field, Event, Subscriber};
use tracing_subscriber::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use crate::config::{SellConfig, StrategyConfig};
use crate::stream::MarketStreamState;
use crate::util::logging::scrub_sensitive;

static EVENT_TX: OnceLock<UnboundedSender<AppEvent>> = OnceLock::new();

pub fn set_sender(tx: UnboundedSender<AppEvent>) {
    let _ = EVENT_TX.set(tx);
}

pub fn emit(event: AppEvent) {
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.send(event);
    }
}

pub fn log_layer() -> EventLogLayer {
    EventLogLayer
}

pub struct EventLogLayer;

impl<S> Layer<S> for EventLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if EVENT_TX.get().is_none() {
            return;
        }

        let meta = event.metadata();
        let level = LogLevel::from_tracing(meta.level());
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        if visitor.is_hot_loop_event() && *meta.level() <= tracing::Level::INFO {
            return;
        }

        let event_name = visitor.event.clone();
        let mut parts = Vec::new();
        if let Some(name) = event_name.clone() {
            parts.push(name);
        }
        for (key, value) in visitor.fields {
            if key == "event" {
                continue;
            }
            parts.push(format!("{key}={value}"));
        }
        if parts.is_empty() {
            parts.push(meta.name().to_string());
        }

        let message = scrub_sensitive(&parts.join(" "));
        let event = event_name.as_ref().map(|value| scrub_sensitive(value));
        emit(AppEvent::LogLine {
            level,
            message,
            event,
        });
    }
}

#[derive(Default)]
struct FieldVisitor {
    event: Option<String>,
    fields: Vec<(String, String)>,
    hot_loop_event: bool,
}

impl FieldVisitor {
    fn push_field(&mut self, field: &Field, value: String) {
        if field.name() == "event" {
            self.event = Some(value.clone());
            if matches!(value.as_str(), "price_tick") {
                self.hot_loop_event = true;
            }
        }
        if !self.hot_loop_event {
            self.fields.push((field.name().to_string(), value));
        }
    }

    fn is_hot_loop_event(&self) -> bool {
        self.hot_loop_event
    }
}

impl Visit for FieldVisitor {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push_field(field, value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push_field(field, value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push_field(field, value.to_string());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.push_field(field, value.to_string());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push_field(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push_field(field, format!("{value:?}"));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn from_tracing(level: &tracing::Level) -> Self {
        match *level {
            tracing::Level::TRACE => LogLevel::Trace,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::ERROR => LogLevel::Error,
        }
    }
}

#[derive(Clone, Debug)]
pub enum AppEvent {
    Startup {
        version: String,
        devnet: bool,
        wallet_pubkey: Pubkey,
    },
    BalanceUpdate {
        lamports: u64,
    },
    Usd1BalanceUpdate {
        base_units: u64,
    },
    RpcMetric {
        duration_ms: u64,
        ok: bool,
    },
    SolanaWsStatus {
        connected: bool,
    },
    MintDetected {
        mint: Pubkey,
        token_account: Pubkey,
    },
    SessionStarted {
        mint: Pubkey,
        token_program: Pubkey,
        started_at_ms: u64,
    },
    SessionStreamState {
        mint: Pubkey,
        stream_state: Arc<dyn MarketStreamState>,
    },
    PositionTokensUpdated {
        mint: Pubkey,
        tokens: u64,
    },
    SellScheduled {
        mint: Pubkey,
        reason: String,
        profit_lamports: i64,
    },
    SellAttempt {
        mint: Pubkey,
        attempt: usize,
        slippage_bps: u16,
    },
    SellRetry {
        mint: Pubkey,
        attempt: usize,
        phase: String,
        error: String,
    },
    SellComplete {
        mint: Pubkey,
        signature: String,
        reason: String,
        slippage_bps: u16,
    },
    SessionClosed {
        mint: Pubkey,
    },
    SessionError {
        mint: Pubkey,
        error: String,
    },
    PauseState {
        paused: bool,
    },
    Heartbeat,
    LogLine {
        level: LogLevel,
        message: String,
        event: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub enum AppCommand {
    Quit,
    TogglePauseNewSessions,
    RequestExitSignal {
        mint: Pubkey,
    },
    ApplySettings {
        strategy: StrategyConfig,
        sell: SellConfig,
    },
}
