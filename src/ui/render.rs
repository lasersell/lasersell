use std::time::Instant;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, List, ListItem, Paragraph};
use ratatui::Frame;
use solana_sdk::pubkey::Pubkey;
use spl_token_interface::native_mint;

use crate::market::{usd1_decimals, usd1_mint};
use crate::ui::format::format_bps_percent;
use crate::ui::mouse::{body_columns, main_layout};
use crate::ui::state::{
    format_duration, short_pubkey, FocusPane, OutputEntry, OutputKind, OutputLevel, UiState,
};
use crate::util::pnl::{lamports_to_sol, lamports_to_sol_signed};

const SCREEN_BG: Color = Color::Rgb(10, 12, 16);
const PANEL_BG: Color = Color::Rgb(18, 20, 24);
const INPUT_BG: Color = Color::Rgb(28, 30, 34);
const LASER_GREEN: Color = Color::Rgb(57, 255, 20);
const MUTED: Color = Color::Rgb(140, 150, 160);
const TEXT: Color = Color::Rgb(232, 234, 238);
const ERROR: Color = Color::Rgb(255, 70, 70);

pub fn render(frame: &mut Frame, state: &UiState) {
    let area = frame.size();
    frame.render_widget(Block::default().style(Style::default().bg(SCREEN_BG)), area);

    let layout = main_layout(area);
    render_header(frame, state, layout.header);
    render_body(frame, state, layout.body);
    render_output(frame, state, layout.output);
    render_command_input(frame, state, layout.command);
}

fn render_header(frame: &mut Frame, state: &UiState, area: Rect) {
    let status_style = if state.paused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(LASER_GREEN)
            .add_modifier(Modifier::BOLD)
    };
    let status_icon = if state.paused { "⏸" } else { "▶" };
    let status_label = if state.paused { "PAUSED" } else { "RUNNING" };

    let rpc_span = match state.rpc_metrics.latest_ms {
        Some(ms) => {
            let color = if ms <= 100 {
                LASER_GREEN
            } else if ms <= 200 {
                Color::Yellow
            } else {
                ERROR
            };
            Span::styled(format!("{ms}ms"), Style::default().fg(color))
        }
        None => Span::styled("--", muted_style()),
    };

    let solana_ws = if state.solana_ws_connected {
        Span::styled("up", Style::default().fg(LASER_GREEN))
    } else {
        Span::styled("down", Style::default().fg(ERROR))
    };

    let wallet = state
        .wallet_pubkey
        .as_ref()
        .map(short_pubkey)
        .unwrap_or_else(|| "--".to_string());

    let balance = state
        .balance_lamports
        .map(|lamports| format!("{:.4} SOL", lamports_to_sol(lamports)))
        .unwrap_or_else(|| "--".to_string());

    let usd1_label = if state.devnet { "USD1 (dev)" } else { "USD1" };
    let usd1_balance = state
        .usd1_balance_base_units
        .map(|base_units| format!("{:.6}", usd1_base_to_unit(base_units as i128)))
        .unwrap_or_else(|| "--".to_string());

    let line = Line::from(vec![
        Span::raw(" "),
        Span::raw(status_icon),
        Span::raw(" "),
        Span::styled(status_label, status_style),
        Span::styled("  |  ", muted_style()),
        Span::styled("RPC ", muted_style()),
        rpc_span,
        Span::styled("  |  ", muted_style()),
        Span::styled("WS ", muted_style()),
        solana_ws,
        Span::styled("  |  ", muted_style()),
        Span::styled("Wallet ", muted_style()),
        Span::styled(wallet, Style::default().fg(TEXT)),
        Span::styled("  |  ", muted_style()),
        Span::styled("SOL ", muted_style()),
        Span::styled(balance, Style::default().fg(TEXT)),
        Span::styled("  |  ", muted_style()),
        Span::styled(format!("{usd1_label} "), muted_style()),
        Span::styled(usd1_balance, Style::default().fg(TEXT)),
    ]);

    let header = Paragraph::new(line).style(Style::default().fg(TEXT).bg(SCREEN_BG));
    frame.render_widget(header, area);
}

fn render_body(frame: &mut Frame, state: &UiState, area: Rect) {
    let (left, right) = body_columns(area);
    render_sessions_list(frame, state, left);
    render_right_panels(frame, state, right);
}

fn render_sessions_list(frame: &mut Frame, state: &UiState, area: Rect) {
    let focused = matches!(state.focus, FocusPane::Sessions);
    let block = panel_block("📈 Sessions", focused);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let summary_value_style = Style::default().fg(LASER_GREEN);
    let summary_line = Line::from(vec![
        Span::styled("Active ", muted_style()),
        Span::styled(state.sessions_active.to_string(), summary_value_style),
        Span::styled("  Pending ", muted_style()),
        Span::styled(state.pending_sessions().to_string(), summary_value_style),
        Span::styled("  Detected ", muted_style()),
        Span::styled(state.detected_mints.to_string(), summary_value_style),
        Span::styled("  Filtered ", muted_style()),
        Span::styled(state.filtered_mints.to_string(), summary_value_style),
    ]);

    let mints = state.mint_keys_sorted();
    let has_mints = !mints.is_empty();
    let hint_line = if !focused {
        Some(Line::from(Span::styled(
            "Tab to focus Sessions panel",
            muted_style(),
        )))
    } else if has_mints {
        Some(Line::from(Span::styled(
            "Mouse-select a mint to see details",
            muted_style(),
        )))
    } else {
        None
    };

    let mut constraints = vec![Constraint::Length(1), Constraint::Min(1)];
    if hint_line.is_some() {
        constraints.push(Constraint::Length(1));
    }
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    frame.render_widget(
        Paragraph::new(summary_line).style(Style::default().fg(MUTED)),
        layout[0],
    );

    let items: Vec<ListItem> = mints
        .iter()
        .filter_map(|mint| state.mint_activity.get(mint))
        .map(|row| {
            let symbol = row
                .symbol
                .as_ref()
                .filter(|value| !value.is_empty())
                .cloned()
                .unwrap_or_else(|| "(unknown)".to_string());
            let short_mint = short_pubkey(&row.mint);
            let status = row.status.label();
            let note = truncate_text(&row.note, 24);
            let line = if note.is_empty() {
                format!("{symbol} {short_mint} [{status}]")
            } else {
                format!("{symbol} {short_mint} [{status}] {note}")
            };
            ListItem::new(line)
        })
        .collect();

    let mut list_state = ratatui::widgets::ListState::default();
    if let Some(selected) = state.selected_mint {
        if let Some(idx) = mints.iter().position(|mint| *mint == selected) {
            list_state.select(Some(idx));
        }
    }

    let highlight_style = if focused {
        Style::default()
            .bg(Color::Rgb(28, 40, 30))
            .fg(TEXT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::Rgb(24, 26, 30)).fg(TEXT)
    };
    let list = List::new(items)
        .style(Style::default().fg(TEXT))
        .highlight_style(highlight_style)
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(list, layout[1], &mut list_state);

    if let Some(line) = hint_line {
        frame.render_widget(
            Paragraph::new(line).style(Style::default().fg(MUTED)),
            layout[2],
        );
    }
}

fn render_right_panels(frame: &mut Frame, state: &UiState, area: Rect) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(6)])
        .split(area);
    render_strategy_panel(frame, state, layout[0]);
    render_selected_detail(frame, state, layout[1]);
}

fn render_strategy_panel(frame: &mut Frame, state: &UiState, area: Rect) {
    let block = panel_block("🎯 Strategy", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let tp = UiState::format_strategy_amount(&state.config.strategy.target_profit);
    let sl = UiState::format_strategy_amount(&state.config.strategy.stop_loss);
    let timeout = state.config.strategy.deadline_timeout_sec;
    let timeout_label = if timeout == 0 {
        "OFF".to_string()
    } else {
        format!("{timeout}s")
    };
    let value_style = Style::default().fg(LASER_GREEN);

    let slippage = format_bps_percent(state.config.sell.slippage_max_bps);
    let lines = vec![Line::from(vec![
        Span::styled("TP ", muted_style()),
        Span::styled(tp, value_style),
        Span::styled("  SL ", muted_style()),
        Span::styled(sl, value_style),
        Span::styled("  DEADLINE ", muted_style()),
        Span::styled(timeout_label, value_style),
        Span::styled("  SLIPPAGE ", muted_style()),
        Span::styled(slippage, value_style),
    ])];

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(TEXT).bg(PANEL_BG)),
        inner,
    );
}

fn render_selected_detail(frame: &mut Frame, state: &UiState, area: Rect) {
    let block = panel_block("🔎 Selected", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(3)])
        .split(inner);

    let mut lines = Vec::new();
    if let Some(selected) = state.selected_mint {
        let mint_row = state.mint_activity.get(&selected);
        let session = state.sessions.get(&selected);
        let active_session = session.filter(|entry| !entry.closed);
        let mint_short = short_pubkey(&selected);
        let identity = if let Some(session) = session {
            format_identity(
                session.symbol.as_deref(),
                session.name.as_deref(),
                &mint_short,
            )
        } else if let Some(row) = mint_row {
            format_identity(row.symbol.as_deref(), row.name.as_deref(), &mint_short)
        } else {
            mint_short.clone()
        };
        lines.push(Line::from(Span::styled(
            format!("Token: {identity} ({mint_short})"),
            Style::default().fg(TEXT),
        )));

        let state_label = if let Some(session) = session {
            session.phase.label()
        } else if let Some(row) = mint_row {
            row.status.label()
        } else {
            "UNKNOWN"
        };
        lines.push(Line::from(Span::styled(
            format!("State: {state_label}"),
            Style::default().fg(MUTED),
        )));

        if active_session.is_none() {
            lines.push(Line::from(Span::styled(
                "No active session for this mint",
                Style::default().fg(MUTED),
            )));
        }

        if let Some(session) = active_session {
            let quote_mint = session.quote_mint.unwrap_or_else(native_mint::id);
            let cost_basis = session
                .cost_basis_lamports
                .map(|amount| format_quote_amount(amount, Some(quote_mint)))
                .unwrap_or_else(|| "--".to_string());
            let requested = session
                .requested_lamports
                .map(|amount| format_quote_amount(amount, Some(quote_mint)))
                .map(|amount| format!(" | Requested: {amount}"))
                .unwrap_or_default();
            lines.push(Line::from(Span::raw(format!(
                "Cost basis: {cost_basis}{requested}"
            ))));

            let tokens = session
                .position_tokens
                .map(|value| value.to_string())
                .unwrap_or_else(|| "--".to_string());
            lines.push(Line::from(Span::raw(format!("Tokens: {tokens}"))));

            let market = session.market_label.as_deref().unwrap_or("--");
            let quote = session.quote_label.as_deref().unwrap_or("--");
            lines.push(Line::from(Span::raw(format!(
                "Market: {market} | Quote: {quote}"
            ))));

            let proceeds = session
                .last_proceeds_lamports
                .map(|amount| format_quote_amount(amount, Some(quote_mint)))
                .unwrap_or_else(|| "--".to_string());
            let pnl_pct = match (session.last_pnl_lamports, session.cost_basis_lamports) {
                (Some(pnl_lamports), Some(cost)) if cost > 0 => {
                    Some((pnl_lamports as f64) / (cost as f64) * 100.0)
                }
                _ => None,
            };
            let pnl_span = if let Some(value) = session.last_pnl_lamports {
                let style = if value >= 0 {
                    Style::default().fg(LASER_GREEN)
                } else {
                    Style::default().fg(ERROR)
                };
                let pct = pnl_pct.map(|v| format!(" ({v:.2}%)")).unwrap_or_default();
                let amount = format_quote_amount_signed(value, Some(quote_mint));
                Span::styled(format!("{amount}{pct}"), style)
            } else {
                Span::styled("--", Style::default().fg(MUTED))
            };
            lines.push(Line::from(vec![
                Span::raw(format!("Proceeds: {proceeds} | PnL: ")),
                pnl_span,
            ]));

            let timeout_left = if state.config.strategy.deadline_timeout_sec == 0 {
                "OFF".to_string()
            } else {
                state
                    .session_timeout_remaining(session)
                    .map(format_duration)
                    .unwrap_or_else(|| "--".to_string())
            };
            lines.push(Line::from(Span::raw(format!(
                "Deadline remaining: {timeout_left}"
            ))));
        }

        if let Some(session) = session {
            if session.last_sell_attempt.is_some()
                || matches!(
                    session.phase,
                    crate::ui::state::SessionPhase::Selling
                        | crate::ui::state::SessionPhase::Closed
                )
            {
                let attempt = session
                    .last_sell_attempt
                    .map(|value| value.saturating_add(1).to_string())
                    .unwrap_or_else(|| "--".to_string());
                let slip = session
                    .last_sell_attempt_slippage_bps
                    .map(format_bps_percent)
                    .unwrap_or_else(|| "--".to_string());
                lines.push(Line::from(Span::raw(format!(
                    "Sell: attempt {attempt}, slippage {slip}, retries {}",
                    session.retry_count
                ))));
            }

            if state.details_expanded {
                let program = short_pubkey(&session.token_program);
                let account = session
                    .token_account
                    .map(|value| short_pubkey(&value))
                    .unwrap_or_else(|| "--".to_string());
                lines.push(Line::from(Span::raw(format!(
                    "Token program: {program} | Token account: {account}"
                ))));
                lines.push(Line::from(Span::raw(format!(
                    "Fee bps: {} | P95 down/slot: {}",
                    session.fee_bps, session.p95_down_per_slot
                ))));
                let complete = session
                    .curve_complete
                    .map(|value| if value { "yes" } else { "no" })
                    .unwrap_or("--");
                lines.push(Line::from(Span::raw(format!("Curve complete: {complete}"))));
                let age = session
                    .last_session_update
                    .map(|at| format_duration(Instant::now().duration_since(at)))
                    .unwrap_or_else(|| "--".to_string());
                lines.push(Line::from(Span::raw(format!("Last update age: {age}"))));
                if let Some(error) = session.last_retry_error.as_deref() {
                    lines.push(Line::from(Span::raw(format!(
                        "Last sell error: {}",
                        truncate_text(error, 48)
                    ))));
                }
                if let Some(signature) = session.last_signature.as_deref() {
                    lines.push(Line::from(Span::raw(format!(
                        "Last signature: {}",
                        shorten_id(signature)
                    ))));
                }
            }
        } else if state.details_expanded {
            let program = mint_row
                .and_then(|row| row.token_program)
                .map(|value| short_pubkey(&value))
                .unwrap_or_else(|| "--".to_string());
            let account = mint_row
                .and_then(|row| row.token_account)
                .map(|value| short_pubkey(&value))
                .unwrap_or_else(|| "--".to_string());
            lines.push(Line::from(Span::raw(format!(
                "Token program: {program} | Token account: {account}"
            ))));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No mint selected",
            Style::default().fg(MUTED),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(TEXT).bg(PANEL_BG)),
        layout[0],
    );

    let progress = selected_tp_progress(state);
    let gauge = Gauge::default()
        .block(
            Block::default()
                .title(Span::styled("Progress to TP", muted_style()))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(MUTED)),
        )
        .gauge_style(Style::default().fg(LASER_GREEN).bg(PANEL_BG))
        .percent(progress);
    frame.render_widget(gauge, layout[1]);
}

fn render_output(frame: &mut Frame, state: &UiState, area: Rect) {
    let block = panel_block("", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let max_lines = inner.height as usize;
    let start = state.output.len().saturating_sub(max_lines);
    let lines: Vec<Line> = state
        .output
        .iter()
        .skip(start)
        .map(format_output_line)
        .collect();

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(TEXT).bg(PANEL_BG)),
        inner,
    );
}

fn render_command_input(frame: &mut Frame, state: &UiState, area: Rect) {
    let focused = matches!(state.focus, FocusPane::Command);
    let block = input_block("❯ Command", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut spans = Vec::new();
    if state.command_buffer.is_empty() {
        spans.push(Span::styled(
            "Type a command… (? for help) ",
            Style::default().fg(MUTED),
        ));
    } else {
        spans.push(Span::styled(
            state.command_buffer.clone(),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ));
    }
    if focused {
        spans.push(Span::styled("▍", Style::default().fg(LASER_GREEN)));
    }

    let paragraph = Paragraph::new(Line::from(spans)).style(
        Style::default()
            .fg(TEXT)
            .bg(INPUT_BG)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(paragraph, inner);
}

fn format_output_line(entry: &OutputEntry) -> Line<'_> {
    match &entry.kind {
        OutputKind::Command => Line::from(vec![
            Span::styled("❯ ", Style::default().fg(LASER_GREEN)),
            Span::styled(
                entry.message.clone(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        OutputKind::Reply(level) => match level {
            OutputLevel::Success => Line::from(vec![
                Span::styled("✓ ", Style::default().fg(LASER_GREEN)),
                Span::styled(entry.message.clone(), Style::default().fg(MUTED)),
            ]),
            OutputLevel::Warn => Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
                Span::styled(entry.message.clone(), Style::default().fg(Color::Yellow)),
            ]),
            OutputLevel::Error => Line::from(vec![
                Span::styled("✖ ", Style::default().fg(ERROR)),
                Span::styled(entry.message.clone(), Style::default().fg(ERROR)),
            ]),
            OutputLevel::Info => Line::from(vec![
                Span::styled("↳ ", Style::default().fg(MUTED)),
                Span::styled(entry.message.clone(), Style::default().fg(MUTED)),
            ]),
        },
    }
}

fn selected_tp_progress(state: &UiState) -> u16 {
    let Some(selected) = state.selected_mint else {
        return 0;
    };
    let Some(session) = state.sessions.get(&selected) else {
        return 0;
    };
    let Some(cost) = session.cost_basis_lamports else {
        return 0;
    };
    let Some(proceeds) = session.last_proceeds_lamports else {
        return 0;
    };
    let Some(tp) = state
        .config
        .strategy
        .target_profit_units(Some(cost))
        .ok()
        .flatten()
    else {
        return 0;
    };
    if tp == 0 {
        return 0;
    }
    let profit = proceeds as i128 - cost as i128;
    let pct = ((profit as f64) / (tp as f64) * 100.0).max(0.0).min(100.0);
    pct.round() as u16
}

fn accent_style() -> Style {
    Style::default()
        .fg(LASER_GREEN)
        .add_modifier(Modifier::BOLD)
}

fn muted_style() -> Style {
    Style::default().fg(MUTED)
}

fn panel_block<T: Into<String>>(title: T, focused: bool) -> Block<'static> {
    let border_style = if focused {
        accent_style()
    } else {
        muted_style()
    };
    Block::default()
        .title(Span::styled(title.into(), border_style))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .style(Style::default().bg(PANEL_BG).fg(TEXT))
}

fn input_block<T: Into<String>>(title: T, focused: bool) -> Block<'static> {
    let border_style = if focused {
        accent_style()
    } else {
        muted_style()
    };
    Block::default()
        .title(Span::styled(title.into(), border_style))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .style(Style::default().bg(INPUT_BG).fg(TEXT))
}

fn format_identity(symbol: Option<&str>, name: Option<&str>, fallback: &str) -> String {
    let symbol = symbol.unwrap_or("").trim();
    let name = name.unwrap_or("").trim();
    if !symbol.is_empty() && !name.is_empty() {
        format!("{symbol} - {name}")
    } else if !symbol.is_empty() {
        symbol.to_string()
    } else if !name.is_empty() {
        name.to_string()
    } else {
        fallback.to_string()
    }
}

fn truncate_text(value: &str, max: usize) -> String {
    let count = value.chars().count();
    if count <= max {
        return value.to_string();
    }
    if max <= 3 {
        return value.chars().take(max).collect();
    }
    let mut out: String = value.chars().take(max - 3).collect();
    out.push_str("...");
    out
}

fn shorten_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 12 {
        return trimmed.to_string();
    }
    let prefix = &trimmed[..4];
    let suffix = &trimmed[trimmed.len() - 4..];
    format!("{prefix}...{suffix}")
}

fn usd1_base_to_unit(amount: i128) -> f64 {
    let scale = 10_f64.powi(usd1_decimals() as i32);
    (amount as f64) / scale
}

fn format_quote_amount(amount: u64, quote_mint: Option<Pubkey>) -> String {
    let mint = quote_mint.unwrap_or_else(native_mint::id);
    if mint == native_mint::id() {
        return format!("{:.4} SOL", lamports_to_sol(amount));
    }
    if mint == usd1_mint() {
        return format!("{:.6} USD1", usd1_base_to_unit(amount as i128));
    }
    amount.to_string()
}

fn format_quote_amount_signed(amount: i128, quote_mint: Option<Pubkey>) -> String {
    let mint = quote_mint.unwrap_or_else(native_mint::id);
    if mint == native_mint::id() {
        return format!("{:.4} SOL", lamports_to_sol_signed(amount));
    }
    if mint == usd1_mint() {
        return format!("{:.6} USD1", usd1_base_to_unit(amount));
    }
    amount.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_quote_amount_usd1() {
        let mint = usd1_mint();
        assert_eq!(format_quote_amount(5_000_000, Some(mint)), "5.000000 USD1");
        assert_eq!(format_quote_amount(74_720, Some(mint)), "0.074720 USD1");
    }
}
