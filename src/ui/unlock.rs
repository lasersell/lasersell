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

const LASER_GREEN: Color = Color::Rgb(57, 255, 20);

pub fn prompt_passphrase(title: &str) -> Result<SecretString> {
    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let _ = execute!(io::stdout(), DisableMouseCapture);
    let mut input = Zeroizing::new(String::new());
    let mut status: Option<String> = None;

    loop {
        let mut unlock_rect: Option<Rect> = None;
        let mut cancel_rect: Option<Rect> = None;
        terminal.draw(|f| {
            let size = f.size();
            let screen_bg = Color::Rgb(10, 12, 16);
            let panel_bg = Color::Rgb(18, 20, 24);
            let input_bg = Color::Rgb(28, 30, 34);
            let muted = Color::Rgb(140, 150, 160);
            let text = Color::Rgb(232, 234, 238);
            let error = Color::Rgb(255, 90, 90);
            let secondary_bg = Color::Rgb(45, 50, 56);
            let secondary_fg = Color::Rgb(210, 215, 222);
            let primary_fg = Color::Rgb(8, 10, 12);

            f.render_widget(Block::default().style(Style::default().bg(screen_bg)), size);

            let max_width = size.width.saturating_sub(4);
            let mut modal_width = max_width.min(72);
            if max_width >= 44 {
                modal_width = modal_width.max(44);
            }
            let max_height = size.height.saturating_sub(4);
            let modal_height = max_height.min(13);
            let modal = centered_rect(size, modal_width, modal_height);

            f.render_widget(Clear, modal);

            let brand_line = Line::from(Span::styled(
                format!("⚡ LASERSELL v{}", env!("CARGO_PKG_VERSION")),
                Style::default()
                    .fg(LASER_GREEN)
                    .add_modifier(Modifier::BOLD),
            ));
            let modal_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(LASER_GREEN))
                .style(Style::default().bg(panel_bg))
                .title(brand_line)
                .title_alignment(Alignment::Center);
            let inner = modal_block.inner(modal);
            f.render_widget(modal_block, modal);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .split(inner);

            let title_line = Line::from(Span::styled(
                title,
                Style::default().fg(text).add_modifier(Modifier::BOLD),
            ));
            f.render_widget(
                Paragraph::new(title_line).alignment(Alignment::Center),
                chunks[0],
            );

            let help_line = Line::from(Span::styled(
                "Enter your passphrase to unlock the keystore.",
                Style::default().fg(muted),
            ));
            f.render_widget(
                Paragraph::new(help_line)
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true }),
                chunks[1],
            );

            let label_line = Line::from(vec![
                Span::styled("🔒", Style::default().fg(LASER_GREEN)),
                Span::raw(" "),
                Span::styled(
                    "Passphrase",
                    Style::default().fg(text).add_modifier(Modifier::BOLD),
                ),
            ]);
            f.render_widget(
                Paragraph::new(label_line).alignment(Alignment::Left),
                chunks[2],
            );

            let input_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(LASER_GREEN).add_modifier(Modifier::DIM))
                .style(Style::default().bg(input_bg));
            let input_inner = input_block.inner(chunks[3]);
            f.render_widget(input_block, chunks[3]);

            let mask_len = input.chars().count();
            let mut available = input_inner.width as usize;
            let show_cursor = available > 0;
            if show_cursor {
                available = available.saturating_sub(1);
            }
            let visible = mask_len.min(available);
            let masked: String = std::iter::repeat('•').take(visible).collect();
            let mut input_spans = Vec::new();
            if !masked.is_empty() {
                input_spans.push(Span::styled(
                    masked,
                    Style::default().fg(text).add_modifier(Modifier::BOLD),
                ));
            }
            if show_cursor && input_inner.width > 0 {
                input_spans.push(Span::styled("▍", Style::default().fg(muted)));
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

            let status_widget = if let Some(message) = status.as_deref() {
                Paragraph::new(Line::from(Span::styled(
                    message,
                    Style::default().fg(error).add_modifier(Modifier::BOLD),
                )))
                .alignment(Alignment::Center)
            } else {
                Paragraph::new(Line::from(Span::styled(
                    "Enter = unlock  •  Esc = cancel",
                    Style::default().fg(muted),
                )))
                .alignment(Alignment::Center)
            };
            f.render_widget(status_widget, chunks[4]);

            let unlock_label = " Unlock ";
            let cancel_label = " Cancel ";
            let gap = "  ";
            let buttons_line = Line::from(vec![
                Span::styled(
                    unlock_label,
                    Style::default()
                        .fg(primary_fg)
                        .bg(LASER_GREEN)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(gap),
                Span::styled(
                    cancel_label,
                    Style::default()
                        .fg(secondary_fg)
                        .bg(secondary_bg)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            let buttons_width = unlock_label.len() + gap.len() + cancel_label.len();
            if chunks[5].height > 0 && chunks[5].width > 0 {
                let start_x = chunks[5]
                    .x
                    .saturating_add(chunks[5].width.saturating_sub(buttons_width as u16) / 2);
                unlock_rect = Some(Rect::new(
                    start_x,
                    chunks[5].y,
                    unlock_label.len() as u16,
                    1,
                ));
                cancel_rect = Some(Rect::new(
                    start_x + unlock_label.len() as u16 + gap.len() as u16,
                    chunks[5].y,
                    cancel_label.len() as u16,
                    1,
                ));
            }
            f.render_widget(
                Paragraph::new(buttons_line).alignment(Alignment::Center),
                chunks[5],
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
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(5), Constraint::Length(3)])
                .margin(2)
                .split(size);
            let buttons_line = Line::from(vec![
                Span::styled("[Yes]", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("[No]", Style::default().add_modifier(Modifier::BOLD)),
            ]);
            let body_lines = vec![Line::from(question), Line::from(""), buttons_line];
            let body_block = Block::default().borders(Borders::ALL).title("Confirm");
            let inner = body_block.inner(chunks[0]);
            if inner.height > 2 {
                let line_y = inner.y + 2;
                let yes_width = "[Yes]".len() as u16;
                let no_width = "[No]".len() as u16;
                let line_width = yes_width + 2 + no_width;
                let start_x = inner.x + inner.width.saturating_sub(line_width) / 2;
                yes_rect = Some(Rect::new(start_x, line_y, yes_width, 1));
                no_rect = Some(Rect::new(start_x + yes_width + 2, line_y, no_width, 1));
            }
            let body = Paragraph::new(body_lines)
                .block(body_block)
                .alignment(Alignment::Center);
            f.render_widget(body, chunks[0]);

            let footer = Paragraph::new("Esc to cancel").alignment(Alignment::Center);
            f.render_widget(footer, chunks[1]);
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
