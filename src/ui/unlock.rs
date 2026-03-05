use std::io;
use std::time::Duration;

use anyhow::{anyhow, Result};
use crossterm::event::DisableMouseCapture;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Terminal;
use secrecy::SecretString;
use zeroize::{Zeroize, Zeroizing};

use crate::ui::terminal::TerminalGuard;

const SOLANA_PURPLE: Color = Color::Rgb(153, 69, 255);
const SOLANA_TEAL: Color = Color::Rgb(20, 241, 149);
const SCREEN_BG: Color = Color::Rgb(10, 12, 16);
const PANEL_BG: Color = Color::Rgb(18, 20, 24);
const INPUT_BG: Color = Color::Rgb(28, 30, 34);
const MUTED: Color = Color::Rgb(120, 130, 145);
const TEXT: Color = Color::Rgb(232, 234, 238);
const ERROR_COLOR: Color = Color::Rgb(255, 90, 90);
const SECONDARY_BTN_BG: Color = Color::Rgb(45, 50, 56);
const SECONDARY_BTN_FG: Color = Color::Rgb(210, 215, 222);
const PRIMARY_BTN_FG: Color = Color::Rgb(255, 255, 255);

pub fn prompt_passphrase(title: &str, wallet_pubkey: Option<&str>) -> Result<SecretString> {
    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let _ = execute!(io::stdout(), DisableMouseCapture);
    let mut input = Zeroizing::new(String::new());
    let mut status: Option<String> = None;

    let truncated_pubkey = wallet_pubkey.map(|pk| {
        if pk.len() > 8 {
            format!("{}...{}", &pk[..4], &pk[pk.len() - 4..])
        } else {
            pk.to_string()
        }
    });

    loop {
        let mut unlock_rect: Option<Rect> = None;
        let mut cancel_rect: Option<Rect> = None;
        let truncated_pubkey = truncated_pubkey.clone();
        terminal.draw(|f| {
            let size = f.size();

            f.render_widget(Block::default().style(Style::default().bg(SCREEN_BG)), size);

            let max_width = size.width.saturating_sub(4);
            let mut modal_width = max_width.min(72);
            if max_width >= 44 {
                modal_width = modal_width.max(44);
            }
            let max_height = size.height.saturating_sub(4);
            let modal_height = max_height.min(16);
            let modal = centered_rect(size, modal_width, modal_height);

            f.render_widget(Clear, modal);

            let version_title = Line::from(Span::styled(
                format!(" v{} ", env!("CARGO_PKG_VERSION")),
                Style::default().fg(MUTED),
            ));
            let modal_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(SOLANA_PURPLE))
                .style(Style::default().bg(PANEL_BG))
                .title_bottom(version_title)
                .title_alignment(Alignment::Right);
            let inner = modal_block.inner(modal);
            f.render_widget(modal_block, modal);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // ASCII logo
                    Constraint::Length(1), // spacer
                    Constraint::Length(1), // title
                    Constraint::Length(1), // wallet address
                    Constraint::Length(1), // spacer
                    Constraint::Length(3), // input
                    Constraint::Length(1), // status
                    Constraint::Length(1), // buttons
                    Constraint::Length(1), // spacer
                    Constraint::Length(1), // discord
                    Constraint::Min(0),   // remainder
                ])
                .split(inner);

            // ASCII logo
            let logo_line = Line::from(Span::styled(
                "L A S E R S E L L",
                Style::default()
                    .fg(SOLANA_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ));
            f.render_widget(
                Paragraph::new(logo_line).alignment(Alignment::Center),
                chunks[0],
            );

            // Title
            let title_line = Line::from(Span::styled(
                title,
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ));
            f.render_widget(
                Paragraph::new(title_line).alignment(Alignment::Center),
                chunks[2],
            );

            // Wallet address
            if let Some(ref pubkey) = truncated_pubkey {
                let addr_line = Line::from(Span::styled(
                    pubkey.as_str(),
                    Style::default().fg(MUTED),
                ));
                f.render_widget(
                    Paragraph::new(addr_line).alignment(Alignment::Center),
                    chunks[3],
                );
            }

            // Input
            let input_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(SOLANA_TEAL))
                .style(Style::default().bg(INPUT_BG));
            let input_inner = input_block.inner(chunks[5]);
            f.render_widget(input_block, chunks[5]);

            let mask_len = input.chars().count();
            let mut available = input_inner.width as usize;
            let show_cursor = available > 0;
            if show_cursor {
                available = available.saturating_sub(1);
            }
            let visible = mask_len.min(available);
            let masked: String = "•".repeat(visible);
            let mut input_spans = Vec::new();
            if !masked.is_empty() {
                input_spans.push(Span::styled(
                    masked,
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                ));
            }
            if show_cursor && input_inner.width > 0 {
                input_spans.push(Span::styled("▍", Style::default().fg(MUTED)));
            }
            let input_line = if input_spans.is_empty() {
                Line::from("")
            } else {
                Line::from(input_spans)
            };
            f.render_widget(
                Paragraph::new(input_line).alignment(Alignment::Left),
                input_inner,
            );

            // Status
            let status_widget = if let Some(message) = status.as_deref() {
                Paragraph::new(Line::from(Span::styled(
                    message,
                    Style::default()
                        .fg(ERROR_COLOR)
                        .add_modifier(Modifier::BOLD),
                )))
                .alignment(Alignment::Center)
            } else {
                Paragraph::new(Line::from(Span::styled(
                    "Enter = unlock  •  Esc = cancel",
                    Style::default().fg(MUTED),
                )))
                .alignment(Alignment::Center)
            };
            f.render_widget(status_widget, chunks[6]);

            // Buttons
            let unlock_label = " Unlock ";
            let cancel_label = " Cancel ";
            let gap = "  ";
            let buttons_line = Line::from(vec![
                Span::styled(
                    unlock_label,
                    Style::default()
                        .fg(PRIMARY_BTN_FG)
                        .bg(SOLANA_PURPLE)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(gap),
                Span::styled(
                    cancel_label,
                    Style::default()
                        .fg(SECONDARY_BTN_FG)
                        .bg(SECONDARY_BTN_BG)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            let buttons_width = unlock_label.len() + gap.len() + cancel_label.len();
            if chunks[7].height > 0 && chunks[7].width > 0 {
                let start_x = chunks[7]
                    .x
                    .saturating_add(chunks[7].width.saturating_sub(buttons_width as u16) / 2);
                unlock_rect = Some(Rect::new(
                    start_x,
                    chunks[7].y,
                    unlock_label.len() as u16,
                    1,
                ));
                cancel_rect = Some(Rect::new(
                    start_x + unlock_label.len() as u16 + gap.len() as u16,
                    chunks[7].y,
                    cancel_label.len() as u16,
                    1,
                ));
            }
            f.render_widget(
                Paragraph::new(buttons_line).alignment(Alignment::Center),
                chunks[7],
            );

            // Discord
            let discord_line = Line::from(Span::styled(
                "discord.gg/lasersell",
                Style::default().fg(SOLANA_TEAL),
            ));
            f.render_widget(
                Paragraph::new(discord_line).alignment(Alignment::Center),
                chunks[9],
            );
        })?;

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match key.code {
                        KeyCode::Esc => {
                            input.zeroize();
                            return Err(anyhow!("passphrase entry cancelled"));
                        }
                        KeyCode::Enter => {
                            if let Some(secret) = accept_passphrase(&mut input, &mut status) {
                                return Ok(secret);
                            }
                        }
                        KeyCode::Backspace => {
                            input.pop();
                        }
                        KeyCode::Char(c) => {
                            input.push(c);
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if let Some(rect) = unlock_rect {
                            if point_in_rect(mouse.column, mouse.row, rect) {
                                if let Some(secret) = accept_passphrase(&mut input, &mut status) {
                                    return Ok(secret);
                                }
                            }
                        }
                        if let Some(rect) = cancel_rect {
                            if point_in_rect(mouse.column, mouse.row, rect) {
                                input.zeroize();
                                return Err(anyhow!("passphrase entry cancelled"));
                            }
                        }
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

pub fn prompt_yes_no(question: &str) -> Result<bool> {
    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let _ = execute!(io::stdout(), DisableMouseCapture);

    loop {
        let mut yes_rect: Option<Rect> = None;
        let mut no_rect: Option<Rect> = None;
        terminal.draw(|f| {
            let size = f.size();

            f.render_widget(Block::default().style(Style::default().bg(SCREEN_BG)), size);

            let max_width = size.width.saturating_sub(4);
            let mut modal_width = max_width.min(60);
            if max_width >= 40 {
                modal_width = modal_width.max(40);
            }
            let max_height = size.height.saturating_sub(4);
            let modal_height = max_height.min(9);
            let modal = centered_rect(size, modal_width, modal_height);

            f.render_widget(Clear, modal);

            let modal_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(SOLANA_PURPLE))
                .title(Span::styled(
                    " Confirm ",
                    Style::default()
                        .fg(SOLANA_PURPLE)
                        .add_modifier(Modifier::BOLD),
                ))
                .title_alignment(Alignment::Center)
                .style(Style::default().bg(PANEL_BG));
            let inner = modal_block.inner(modal);
            f.render_widget(modal_block, modal);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2), // question
                    Constraint::Length(1), // spacer
                    Constraint::Length(1), // buttons
                    Constraint::Length(1), // hint
                    Constraint::Min(0),
                ])
                .split(inner);

            let question_widget = Paragraph::new(Line::from(Span::styled(
                question,
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
            f.render_widget(question_widget, chunks[0]);

            let yes_label = " Yes ";
            let no_label = " No ";
            let gap = "  ";
            let buttons_line = Line::from(vec![
                Span::styled(
                    yes_label,
                    Style::default()
                        .fg(PRIMARY_BTN_FG)
                        .bg(SOLANA_PURPLE)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(gap),
                Span::styled(
                    no_label,
                    Style::default()
                        .fg(SECONDARY_BTN_FG)
                        .bg(SECONDARY_BTN_BG)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            let buttons_width = yes_label.len() + gap.len() + no_label.len();
            if chunks[2].height > 0 && chunks[2].width > 0 {
                let start_x = chunks[2]
                    .x
                    .saturating_add(chunks[2].width.saturating_sub(buttons_width as u16) / 2);
                yes_rect = Some(Rect::new(start_x, chunks[2].y, yes_label.len() as u16, 1));
                no_rect = Some(Rect::new(
                    start_x + yes_label.len() as u16 + gap.len() as u16,
                    chunks[2].y,
                    no_label.len() as u16,
                    1,
                ));
            }
            f.render_widget(
                Paragraph::new(buttons_line).alignment(Alignment::Center),
                chunks[2],
            );

            let hint_line = Line::from(Span::styled(
                "y/n • Esc to cancel",
                Style::default().fg(MUTED),
            ));
            f.render_widget(
                Paragraph::new(hint_line).alignment(Alignment::Center),
                chunks[3],
            );
        })?;

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(true),
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => return Ok(false),
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if let Some(rect) = yes_rect {
                            if point_in_rect(mouse.column, mouse.row, rect) {
                                return Ok(true);
                            }
                        }
                        if let Some(rect) = no_rect {
                            if point_in_rect(mouse.column, mouse.row, rect) {
                                return Ok(false);
                            }
                        }
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

fn accept_passphrase(
    input: &mut Zeroizing<String>,
    status: &mut Option<String>,
) -> Option<SecretString> {
    if input.is_empty() {
        *status = Some("Passphrase cannot be empty.".to_string());
        return None;
    }
    let secret = SecretString::new(input.to_string());
    input.zeroize();
    Some(secret)
}

fn point_in_rect(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

fn centered_rect(size: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(size.width);
    let h = height.min(size.height);
    let x = size.x + size.width.saturating_sub(w) / 2;
    let y = size.y + size.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}
