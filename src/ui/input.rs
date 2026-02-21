use std::io;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use tokio::sync::mpsc;

use crate::config::{Config, StrategyAmount};
use crate::events::AppCommand;
use crate::ui::format::parse_percent_to_bps;
use crate::ui::state::{FocusPane, OutputLevel, PendingConfirm, UiState};

pub struct InputHandle {
    running: Arc<AtomicBool>,
}

impl InputHandle {
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

pub fn spawn_input_listener() -> (mpsc::UnboundedReceiver<Event>, InputHandle) {
    let (tx, rx) = mpsc::unbounded_channel();
    let running = Arc::new(AtomicBool::new(true));
    let running_thread = Arc::clone(&running);
    std::thread::spawn(move || {
        while running_thread.load(Ordering::SeqCst) {
            if event::poll(Duration::from_millis(200)).unwrap_or(false) {
                if let Ok(evt) = event::read() {
                    let _ = tx.send(evt);
                }
            }
        }
    });

    (rx, InputHandle { running })
}

pub fn handle_key_event(
    key: KeyEvent,
    state: &mut UiState,
    cmd_tx: &mpsc::UnboundedSender<AppCommand>,
) {
    if key.kind != KeyEventKind::Press {
        return;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        let _ = cmd_tx.send(AppCommand::Quit);
        state.push_output_reply(OutputLevel::Info, "Quitting...");
        return;
    }

    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        state.toggle_focus();
        apply_mouse_capture(state);
        return;
    }

    match state.focus {
        FocusPane::Sessions => handle_sessions_key(key, state, cmd_tx),
        FocusPane::Command => handle_command_key(key, state, cmd_tx),
    }
}

fn handle_sessions_key(
    key: KeyEvent,
    state: &mut UiState,
    cmd_tx: &mpsc::UnboundedSender<AppCommand>,
) {
    match key.code {
        KeyCode::Char('m') => {
            state.mouse_capture = !state.mouse_capture;
            apply_mouse_capture(state);
            if state.mouse_capture {
                state.push_output_reply(OutputLevel::Info, "Mouse capture enabled.");
            } else {
                state.push_output_reply(OutputLevel::Info, "Mouse capture disabled.");
            }
        }
        KeyCode::Up => state.select_prev_mint(),
        KeyCode::Down => state.select_next_mint(),
        KeyCode::Enter => {
            state.details_expanded = !state.details_expanded;
        }
        KeyCode::Char(' ') => {
            state.paused = !state.paused;
            let _ = cmd_tx.send(AppCommand::TogglePauseNewSessions);
            let msg = if state.paused {
                "Paused new sessions."
            } else {
                "Resumed new sessions."
            };
            state.push_output_reply(OutputLevel::Info, msg);
        }
        _ => {}
    }
}

fn apply_mouse_capture(state: &UiState) {
    if state.mouse_capture && matches!(state.focus, FocusPane::Sessions) {
        let _ = execute!(io::stdout(), EnableMouseCapture);
    } else {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

fn handle_command_key(
    key: KeyEvent,
    state: &mut UiState,
    cmd_tx: &mpsc::UnboundedSender<AppCommand>,
) {
    match key.code {
        KeyCode::Esc => {
            state.command_buffer.clear();
            state.command_history_cursor = None;
            state.pending_confirm = None;
        }
        KeyCode::Enter => {
            let command = state.command_buffer.trim().to_string();
            state.command_buffer.clear();
            state.command_history_cursor = None;
            if !command.is_empty() {
                state.push_history(command.clone());
                execute_command(&command, state, cmd_tx);
            }
        }
        KeyCode::Backspace => {
            state.command_buffer.pop();
            state.command_history_cursor = None;
        }
        KeyCode::Up => history_up(state),
        KeyCode::Down => history_down(state),
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                return;
            }
            state.command_buffer.push(c);
            state.command_history_cursor = None;
        }
        _ => {}
    }
}

fn history_up(state: &mut UiState) {
    if state.command_history.is_empty() {
        return;
    }
    let next_idx = match state.command_history_cursor {
        Some(idx) => idx.saturating_sub(1),
        None => state.command_history.len().saturating_sub(1),
    };
    if let Some(value) = state.command_history.get(next_idx) {
        state.command_history_cursor = Some(next_idx);
        state.command_buffer = value.clone();
    }
}

fn history_down(state: &mut UiState) {
    let Some(idx) = state.command_history_cursor else {
        return;
    };
    let next_idx = idx.saturating_add(1);
    if next_idx >= state.command_history.len() {
        state.command_history_cursor = None;
        state.command_buffer.clear();
    } else if let Some(value) = state.command_history.get(next_idx) {
        state.command_history_cursor = Some(next_idx);
        state.command_buffer = value.clone();
    }
}

fn execute_command(
    command_str: &str,
    state: &mut UiState,
    cmd_tx: &mpsc::UnboundedSender<AppCommand>,
) {
    let trimmed = command_str.trim();
    if trimmed.is_empty() {
        return;
    }
    state.push_output_command(trimmed);

    if let Some(pending) = state.pending_confirm.take() {
        let lower = trimmed.to_lowercase();
        if matches!(lower.as_str(), "y" | "yes") {
            match pending {
                PendingConfirm::RequestExitSignal { mint, .. } => {
                    let _ = cmd_tx.send(AppCommand::RequestExitSignal { mint });
                    state.push_output_reply(OutputLevel::Success, "Sell queued.");
                }
            }
            return;
        }
        if matches!(lower.as_str(), "n" | "no") {
            state.push_output_reply(OutputLevel::Info, "Cancelled.");
            return;
        }
    }

    let mut parts = trimmed.split_whitespace();
    let cmd = parts.next().unwrap_or_default().to_lowercase();
    let args: Vec<&str> = parts.collect();

    match cmd.as_str() {
        "?" | "help" => {
            for line in help_lines() {
                state.push_output_reply(OutputLevel::Info, line);
            }
        }
        "quit" | "exit" | "q" => {
            let _ = cmd_tx.send(AppCommand::Quit);
            state.push_output_reply(OutputLevel::Info, "Quitting...");
        }
        "pause" => {
            if state.paused {
                state.push_output_reply(OutputLevel::Info, "Already paused.");
            } else {
                state.paused = true;
                let _ = cmd_tx.send(AppCommand::TogglePauseNewSessions);
                state.push_output_reply(OutputLevel::Success, "Paused new sessions.");
            }
        }
        "resume" => {
            if !state.paused {
                state.push_output_reply(OutputLevel::Info, "Already running.");
            } else {
                state.paused = false;
                let _ = cmd_tx.send(AppCommand::TogglePauseNewSessions);
                state.push_output_reply(OutputLevel::Success, "Resumed new sessions.");
            }
        }
        "sell" | "s" => {
            if let Some(mint) = state.selected_mint {
                state.pending_confirm = Some(PendingConfirm::RequestExitSignal {
                    mint,
                    started: std::time::Instant::now(),
                });
                state.push_output_reply(OutputLevel::Warn, "Confirm: type y");
            } else {
                state.push_output_reply(OutputLevel::Warn, "No session selected.");
            }
        }
        "set" => handle_set_command(state, &args, cmd_tx),
        "clear" => {
            state.clear_output();
        }
        "" => {}
        _ => {
            state.push_output_reply(OutputLevel::Warn, "Unknown command.");
        }
    }
}

fn help_lines() -> Vec<&'static str> {
    vec![
        "Commands:",
        "  help, ?",
        "  quit, exit, q",
        "  pause, resume",
        "  sell, s",
        "  set tp <percent> | set sl <percent> | set slippage <percent> | set timeout <seconds>",
        "  clear",
    ]
}

fn handle_set_command(
    state: &mut UiState,
    args: &[&str],
    cmd_tx: &mpsc::UnboundedSender<AppCommand>,
) {
    if args.len() < 2 {
        state.push_output_reply(
            OutputLevel::Warn,
            "Usage: set tp <percent> | set sl <percent> | set slippage <percent> | set timeout <seconds>",
        );
        return;
    }
    let key = args[0].to_lowercase();
    let value = args[1];
    let mut cfg = match Config::load_from_path(&state.config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            state.push_output_reply(OutputLevel::Error, format!("Config load failed: {err}"));
            return;
        }
    };

    let update_result = match key.as_str() {
        "tp" => StrategyAmount::parse_str(value).map(|parsed| cfg.strategy.target_profit = parsed),
        "sl" => StrategyAmount::parse_str(value).map(|parsed| cfg.strategy.stop_loss = parsed),
        "slip" | "slippage" => {
            parse_percent_to_bps(value, "slippage").map(|parsed| cfg.sell.slippage_max_bps = parsed)
        }
        "to" | "timeout" => value
            .parse::<u64>()
            .map(|parsed| cfg.strategy.deadline_timeout_sec = parsed)
            .map_err(|err| anyhow::anyhow!(err)),
        _ => Err(anyhow::anyhow!("Unknown set target.")),
    };

    if let Err(err) = update_result {
        state.push_output_reply(OutputLevel::Error, format!("{err}"));
        return;
    }

    if let Err(err) = cfg.validate() {
        state.push_output_reply(OutputLevel::Error, format!("{err}"));
        return;
    }

    if let Err(err) = cfg.write_to_path(&state.config_path) {
        state.push_output_reply(OutputLevel::Error, format!("Config save failed: {err}"));
        return;
    }

    state.config = Arc::new(cfg.clone());
    let _ = cmd_tx.send(AppCommand::ApplySettings {
        strategy: cfg.strategy.clone(),
        sell: cfg.sell.clone(),
    });
    state.push_output_reply(OutputLevel::Success, "Settings applied.");
}
