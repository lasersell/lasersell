use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::events::{AppCommand, AppEvent};
use crate::ui::input::{handle_key_event, spawn_input_listener};
use crate::ui::mouse::handle_mouse_event;
use crate::ui::render::render;
use crate::ui::state::{FocusPane, UiState};
use crate::ui::terminal::TerminalGuard;

pub mod format;
pub mod input;
pub mod mouse;
pub mod onboarding;
pub mod render;
pub mod state;
pub mod terminal;
pub mod unlock;

pub async fn run_tui(
    cfg: std::sync::Arc<Config>,
    config_path: PathBuf,
    mut event_rx: mpsc::UnboundedReceiver<AppEvent>,
    cmd_tx: mpsc::UnboundedSender<AppCommand>,
    update_available: Option<String>,
) -> Result<()> {
    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    terminal.hide_cursor()?;
    let mut last_size = terminal.size()?;

    let (mut input_rx, input_handle) = spawn_input_listener();
    let mut state = UiState::new(cfg, config_path);
    state.update_available = update_available;
    if !(state.mouse_capture && matches!(state.focus, FocusPane::Sessions)) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut should_draw = true;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                state.tick();
                state.update_selected_metrics().await;
                should_draw = true;
                if state.should_quit {
                    break;
                }
            }
            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(evt) => {
                        state.apply_event(evt);
                        should_draw = true;
                        if state.should_quit {
                            break;
                        }
                    }
                    None => break,
                }
            }
            maybe_input = input_rx.recv() => {
                if let Some(evt) = maybe_input {
                    match evt {
                        crossterm::event::Event::Key(key) => {
                            handle_key_event(key, &mut state, &cmd_tx);
                            should_draw = true;
                        }
                        crossterm::event::Event::Mouse(mouse) => {
                            if state.mouse_capture && matches!(state.focus, FocusPane::Sessions) {
                                handle_mouse_event(mouse, &mut state, &cmd_tx, last_size);
                                should_draw = true;
                            }
                        }
                        crossterm::event::Event::Resize(width, height) => {
                            last_size = Rect::new(0, 0, width, height);
                            should_draw = true;
                        }
                        _ => {}
                    }
                }
            }
        }

        if should_draw {
            terminal.draw(|f| render(f, &state))?;
            should_draw = false;
        }
    }

    input_handle.stop();
    Ok(())
}
