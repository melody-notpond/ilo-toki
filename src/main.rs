use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use crossterm::{
    event::{Event, KeyCode},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use matrix_sdk::{
    config::SyncSettings,
    reqwest::Url,
    ruma::{
        events::{room::message::{RoomMessageEventContent, SyncRoomMessageEvent}, SyncMessageLikeEvent},
        UserId,
    },
    Client, Session,
};
use tokio::sync::Mutex;
use tui::{backend::CrosstermBackend, layout, widgets, Terminal, text::{Spans, Span, Text}};

struct Message {
    user: String,
    content: String,
}

enum Mode {
    Insert,
    Normal,
}

struct AppState {
    messages: Vec<Message>,

    input_text: String,
    input_char_pos: usize,
    input_byte_pos: usize,

    client: Arc<Client>,
    mode: Mode,
}

static RUNNING: AtomicBool = AtomicBool::new(true);

#[tokio::main]
async fn main() -> Result<(), io::Error> {
    let credentials_file = std::fs::read_to_string(".credentials").unwrap();
    let credentials = credentials_file.split('\n').collect::<Vec<_>>();
    let client = Client::new(Url::parse(credentials[0]).unwrap())
        .await
        .unwrap();
    let client = Arc::new(client);
    client
        .restore_login(Session {
            user_id: UserId::parse(credentials[1]).unwrap(),
            access_token: credentials[2].to_string(),
            device_id: credentials[3].into(),
            refresh_token: None,
        })
        .await
        .unwrap();
    let state = AppState {
        messages: vec![],
        input_text: String::new(),
        input_char_pos: 0,
        input_byte_pos: 0,
        client: client.clone(),
        mode: Mode::Insert,
    };
    let state = Arc::new(Mutex::new(state));
    {
        let lock = state.lock().await;

        let state2 = state.clone();
        lock.client
            .add_event_handler(move |event: SyncRoomMessageEvent| {
                let state = state2.clone();
                async move {
                    let mut lock = state.lock().await;
                    match event {
                        SyncMessageLikeEvent::Original(message) => {
                            lock.messages.push(Message {
                                user: message.sender.to_string(),
                                content: message.content.body().to_string(),
                            });
                        }

                        SyncMessageLikeEvent::Redacted(_) => (),
                    }
                }
            });
    }

    client.sync_once(SyncSettings::default()).await.unwrap();
    tokio::task::spawn(async move {
        client.sync(SyncSettings::default()).await.unwrap();
    });
    tokio::task::spawn(ui_events(state.clone()));
    main_ui(state).await
}

async fn main_ui(state: Arc<Mutex<AppState>>) -> Result<(), io::Error> {
    let stdout = io::stdout();
    crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut stdout = io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    terminal.clear()?;

    while RUNNING.load(Ordering::Acquire) {
        let state = state.lock().await;
        terminal.draw(|f| {
            let vertical = layout::Layout::default()
                .direction(layout::Direction::Vertical)
                .constraints([
                    layout::Constraint::Min(3),
                    layout::Constraint::Length(3),
                    layout::Constraint::Length(1),
                ])
                .split(f.size());

            let messages = widgets::Block::default().borders(widgets::Borders::ALL);
            let messages_list: Vec<_> = state.messages.iter().rev().map(|v| {
                vec![Spans::from(vec![Span::raw(&v.user)]), Spans::from(vec![Span::raw(&v.content)])]
            })
            .map(|v| widgets::ListItem::new(Text::from(v))).collect();
            let messages = widgets::List::new(messages_list)
                .block(messages)
                .start_corner(layout::Corner::BottomLeft);
            f.render_stateful_widget(messages, vertical[0], &mut widgets::ListState::default());

            let input = widgets::Block::default().borders(widgets::Borders::ALL);
            let input = widgets::Paragraph::new(state.input_text.as_str()).block(input);
            f.render_widget(input, vertical[1]);

            let status = {
                match state.mode {
                    Mode::Insert => Span::raw("INSERT"),
                    Mode::Normal => Span::raw("NORMAL"),
                }
            };
            let status = widgets::Paragraph::new(status);
            f.render_widget(status, vertical[2]);

            match state.mode {
                Mode::Insert => {
                    use crossterm::cursor::{CursorShape, SetCursorShape};
                    crossterm::execute!(stdout, SetCursorShape(CursorShape::Line)).unwrap();
                    let m = state.input_char_pos as u16 % (vertical[1].width - 2);
                    if m == 0 && state.input_char_pos != 0 {
                        f.set_cursor(
                            vertical[1].x + vertical[1].width - 1,
                            vertical[1].y
                                + (state.input_char_pos as u16 - 1) / (vertical[1].width - 2)
                                + 1,
                        );
                    } else {
                        f.set_cursor(
                            vertical[1].x + m + 1,
                            vertical[1].y + state.input_char_pos as u16 / (vertical[1].width - 2) + 1,
                        );
                    }
                }

                Mode::Normal => {
                    use crossterm::cursor::{CursorShape, SetCursorShape};
                    crossterm::execute!(stdout, SetCursorShape(CursorShape::Block)).unwrap();
                    let m = state.input_char_pos as u16 % (vertical[1].width - 2);
                    if m == 0 && state.input_char_pos != 0 {
                        f.set_cursor(
                            vertical[1].x + vertical[1].width - 1,
                            vertical[1].y
                                + (state.input_char_pos as u16 - 1) / (vertical[1].width - 2)
                                + 1,
                        );
                    } else {
                        f.set_cursor(
                            vertical[1].x + m + 1,
                            vertical[1].y + state.input_char_pos as u16 / (vertical[1].width - 2) + 1,
                        );
                    }
                }
            }
        })?;

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    terminal.clear()?;
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    terminal.set_cursor(0, 0)?;

    Ok(())
}

async fn ui_events(state: Arc<Mutex<AppState>>) {
    let room = state.lock().await.client.joined_rooms().swap_remove(0);

    while let Ok(Ok(event)) = tokio::task::spawn_blocking(crossterm::event::read).await {
        let mut state = state.lock().await;
        match state.mode {
            Mode::Insert => {
                match event {
                    Event::FocusGained => (),
                    Event::FocusLost => (),
                    Event::Resize(_, _) => (),

                    Event::Key(key) => match key.code {
                        KeyCode::Backspace => {
                            if state.input_byte_pos > 0 {
                                let mut i = 1;
                                while !state.input_text.is_char_boundary(state.input_byte_pos - i) {
                                    i += 1;
                                }
                                state.input_byte_pos -= i;
                                state.input_char_pos -= 1;
                                let pos = state.input_byte_pos;
                                state.input_text.remove(pos);
                            }
                        }

                        KeyCode::Enter => {
                            if state.input_text == "/quit" {
                                RUNNING.store(false, Ordering::Release);
                                break;
                            }
                            if !state.input_text.is_empty() {
                                room.send(
                                    RoomMessageEventContent::text_plain(state.input_text.clone()),
                                    None,
                                )
                                .await
                                .unwrap();
                                state.input_text.clear();
                                state.input_char_pos = 0;
                                state.input_byte_pos = 0;
                            }
                        }

                        KeyCode::Up => (),
                        KeyCode::Down => (),
                        KeyCode::Home => (),
                        KeyCode::End => (),
                        KeyCode::PageUp => (),
                        KeyCode::PageDown => (),
                        KeyCode::Tab => (),
                        KeyCode::BackTab => (),
                        KeyCode::Delete => (),
                        KeyCode::Insert => (),
                        KeyCode::F(_) => (),


                        KeyCode::Left => {
                            if state.input_byte_pos > 0 {
                                let mut i = 1;
                                while !state.input_text.is_char_boundary(state.input_byte_pos - i) {
                                    i += 1;
                                }
                                state.input_byte_pos -= i;
                                state.input_char_pos -= 1;
                            }
                        }

                        KeyCode::Right => {
                            if state.input_byte_pos < state.input_text.bytes().len() {
                                let mut i = 1;
                                while !state.input_text.is_char_boundary(state.input_byte_pos + i) {
                                    i += 1;
                                }
                                state.input_byte_pos += i;
                                state.input_char_pos += 1;
                            }
                        }


                        KeyCode::Char(c) => {
                            let pos = state.input_byte_pos;
                            state.input_text.insert(pos, c);
                            state.input_byte_pos += c.len_utf8();
                            state.input_char_pos += 1;
                        }

                        KeyCode::Null => (),

                        KeyCode::Esc => {
                            state.mode = Mode::Normal;
                        }

                        KeyCode::CapsLock => (),
                        KeyCode::ScrollLock => (),
                        KeyCode::NumLock => (),
                        KeyCode::PrintScreen => (),
                        KeyCode::Pause => (),
                        KeyCode::Menu => (),
                        KeyCode::KeypadBegin => (),
                        KeyCode::Media(_) => (),
                        KeyCode::Modifier(_) => (),
                    }

                    Event::Mouse(_) => (),
                    Event::Paste(_) => (),
                }               
            }

            Mode::Normal => {
                match event {
                    Event::FocusGained => (),
                    Event::FocusLost => (),

                    Event::Key(key) => {
                        match key.code {
                            KeyCode::Backspace => (),
                            KeyCode::Enter => {
                                if state.input_text == "/quit" {
                                    RUNNING.store(false, Ordering::Release);
                                    break;
                                }
                                if !state.input_text.is_empty() {
                                    if state.input_text == "/quit" {
                                        RUNNING.store(false, Ordering::Release);
                                        break;
                                    }
                                    room.send(
                                        RoomMessageEventContent::text_plain(state.input_text.clone()),
                                        None,
                                    )
                                    .await
                                    .unwrap();
                                    state.input_text.clear();
                                    state.input_char_pos = 0;
                                    state.input_byte_pos = 0;
                                }
                            }
                            KeyCode::Up => (),
                            KeyCode::Down => (),
                            KeyCode::Home => (),
                            KeyCode::End => (),
                            KeyCode::PageUp => (),
                            KeyCode::PageDown => (),
                            KeyCode::Tab => (),
                            KeyCode::BackTab => (),
                            KeyCode::Delete => (),
                            KeyCode::Insert => (),
                            KeyCode::F(_) => (),

                            KeyCode::Char('i') => {
                                state.mode = Mode::Insert;
                            }

                            KeyCode::Char('h') | KeyCode::Left => {
                                if state.input_byte_pos > 0 {
                                    let mut i = 1;
                                    while !state.input_text.is_char_boundary(state.input_byte_pos - i) {
                                        i += 1;
                                    }
                                    state.input_byte_pos -= i;
                                    state.input_char_pos -= 1;
                                }
                            }

                            KeyCode::Char('l') | KeyCode::Right => {
                                if state.input_byte_pos < state.input_text.bytes().len() {
                                    let mut i = 1;
                                    while !state.input_text.is_char_boundary(state.input_byte_pos + i) {
                                        i += 1;
                                    }
                                    state.input_byte_pos += i;
                                    state.input_char_pos += 1;
                                }
                            }

                            KeyCode::Char(_) => (),

                            KeyCode::Null => (),
                            KeyCode::Esc => (),
                            KeyCode::CapsLock => (),
                            KeyCode::ScrollLock => (),
                            KeyCode::NumLock => (),
                            KeyCode::PrintScreen => (),
                            KeyCode::Pause => (),
                            KeyCode::Menu => (),
                            KeyCode::KeypadBegin => (),
                            KeyCode::Media(_) => (),
                            KeyCode::Modifier(_) => (),
                        }
                    }

                    Event::Mouse(_) => (),
                    Event::Paste(_) => (),
                    Event::Resize(_, _) => (),
                }
            }
        }

    }
}
