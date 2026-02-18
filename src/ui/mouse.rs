use std::io;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders};
use tokio::sync::mpsc;

use crate::events::AppCommand;
use crate::ui::state::{FocusPane, UiState};

const HEADER_HEIGHT: u16 = 1;
const COMMAND_HEIGHT: u16 = 3;
const MIN_BODY_HEIGHT: u16 = 1;

pub struct MainLayout {
    pub header: Rect,
    pub body: Rect,
    pub output: Rect,
    pub command: Rect,
}

pub fn main_layout(area: Rect) -> MainLayout {
    let output_height = output_panel_height(area.height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Min(MIN_BODY_HEIGHT),
            Constraint::Length(output_height),
            Constraint::Length(COMMAND_HEIGHT),
        ])
        .split(area);
    MainLayout {
        header: chunks[0],
        body: chunks[1],
        output: chunks[2],
        command: chunks[3],
    }
}

pub fn body_columns(area: Rect) -> (Rect, Rect) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);
    (panes[0], panes[1])
}

pub fn sessions_list_area(area: Rect) -> Rect {
    let inner = pane_inner(area);
    if inner.height == 0 {
        return inner;
    }
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    layout[1]
}

pub fn handle_mouse_event(
    mouse: MouseEvent,
    state: &mut UiState,
    cmd_tx: &mpsc::UnboundedSender<AppCommand>,
    terminal_size: Rect,
) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            handle_left_click(mouse, state, cmd_tx, terminal_size);
        }
        MouseEventKind::ScrollUp => handle_scroll(-1, state),
        MouseEventKind::ScrollDown => handle_scroll(1, state),
        _ => {}
    }
}

fn handle_left_click(
    mouse: MouseEvent,
    state: &mut UiState,
    cmd_tx: &mpsc::UnboundedSender<AppCommand>,
    terminal_size: Rect,
) {
    let layout = main_layout(terminal_size);
    let x = mouse.column;
    let y = mouse.row;

    if handle_status_click(x, y, state, cmd_tx, layout.header) {
        return;
    }

    if point_in_rect(x, y, layout.command) {
        set_focus(state, FocusPane::Command);
        return;
    }

    if point_in_rect(x, y, layout.output) {
        set_focus(state, FocusPane::Command);
        return;
    }

    if point_in_rect(x, y, layout.body) {
        let (left, _) = body_columns(layout.body);
        if point_in_rect(x, y, left) {
            handle_sessions_click(x, y, state, left);
            return;
        }
    }
}

fn handle_status_click(
    x: u16,
    y: u16,
    state: &mut UiState,
    cmd_tx: &mpsc::UnboundedSender<AppCommand>,
    area: Rect,
) -> bool {
    if area.height == 0 {
        return false;
    }
    let width = area.width.min(12);
    let rect = Rect::new(area.x, area.y, width, 1);
    if point_in_rect(x, y, rect) {
        state.paused = !state.paused;
        let _ = cmd_tx.send(AppCommand::TogglePauseNewSessions);
        return true;
    }
    false
}

fn handle_sessions_click(x: u16, y: u16, state: &mut UiState, area: Rect) {
    let list_area = sessions_list_area(area);
    if !point_in_rect(x, y, list_area) {
        set_focus(state, FocusPane::Sessions);
        return;
    }
    let idx = y.saturating_sub(list_area.y) as usize;
    let mints = state.mint_keys_sorted();
    if let Some(mint) = mints.get(idx).copied() {
        state.selected_mint = Some(mint);
        set_focus(state, FocusPane::Sessions);
    }
}

fn handle_scroll(direction: i32, state: &mut UiState) {
    if !matches!(state.focus, FocusPane::Sessions) {
        return;
    }
    if direction < 0 {
        state.select_prev_mint();
    } else {
        state.select_next_mint();
    }
}

fn set_focus(state: &mut UiState, focus: FocusPane) {
    if state.focus != focus {
        state.focus = focus;
        apply_mouse_capture(state);
    }
}

fn apply_mouse_capture(state: &UiState) {
    if state.mouse_capture && matches!(state.focus, FocusPane::Sessions) {
        let _ = execute!(io::stdout(), EnableMouseCapture);
    } else {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

fn output_panel_height(total_height: u16) -> u16 {
    let desired = if total_height >= 30 { 10 } else { 8 };
    let max_output = total_height.saturating_sub(HEADER_HEIGHT + COMMAND_HEIGHT + MIN_BODY_HEIGHT);
    let min_output = max_output.min(4);
    desired.min(max_output).max(min_output)
}

fn pane_inner(area: Rect) -> Rect {
    Block::default().borders(Borders::ALL).inner(area)
}

fn point_in_rect(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}
