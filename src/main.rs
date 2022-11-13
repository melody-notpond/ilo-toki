use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration, collections::{HashMap, hash_map::Entry},
};

use crossterm::{
    event::{Event, KeyCode},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use matrix_sdk::{
    config::SyncSettings,
    reqwest::Url,
    ruma::{
        events::{room::message::{RoomMessageEventContent, SyncRoomMessageEvent}, SyncMessageLikeEvent, AnyTimelineEvent, AnyMessageLikeEvent, MessageLikeEvent},
        UserId, OwnedRoomId, UInt,
    },
    Client, Session, room::{Room, Joined, MessagesOptions},
};
use tokio::sync::Mutex;
use tui::{backend::CrosstermBackend, layout, widgets, Terminal, text::{Spans, Span, Text}, style::{Style, Color}};

struct Channel {
    name: String,
    room: Joined,
    messages: Vec<Message>,
    // last_update: Option<Instant>,
    at_top: bool,
    messages_prev_batch: Option<String>,
    last_updated_sync: Option<String>,
}

struct Message {
    user: String,
    content: String,
    timestamp: UInt,
}

enum Mode {
    Insert,
    Normal,
    SelectChannel,
    ScrollMessages,
}

struct AppState {
    channels: HashMap<OwnedRoomId, Channel>,
    channel_ids: Vec<OwnedRoomId>,
    current_channel: Option<OwnedRoomId>,
    channels_state: widgets::ListState,

    messages_state: widgets::ListState,

    input_text: String,
    input_char_pos: usize,
    input_byte_pos: usize,

    mode: Mode,
    client: Arc<Client>,
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
        channels: HashMap::new(),
        channel_ids: vec![],
        current_channel: None,
        channels_state: widgets::ListState::default(),
        messages_state: widgets::ListState::default(),
        input_text: String::new(),
        input_char_pos: 0,
        input_byte_pos: 0,
        mode: Mode::Normal,
        client: client.clone(),
    };
    let state = Arc::new(Mutex::new(state));

    {
        let lock = state.lock().await;

        let state2 = state.clone();
        lock.client
            .add_event_handler(move |event: SyncRoomMessageEvent, room: Room| {
                let state = state2.clone();
                async move {
                    let mut lock = state.lock().await;
                    let last_updated_sync = lock.client.sync_token().await;
                    match event {
                        SyncMessageLikeEvent::Original(message) => {
                            let id = room.room_id().to_owned();
                            if let Entry::Vacant(v) = lock.channels.entry(room.room_id().to_owned()) {
                                if let Room::Joined(room) = room {
                                    let channel = Channel {
                                        name: room.display_name().await.map(|v| v.to_string()).unwrap_or_else(|_| String::from("[unknown room]")),
                                        room,
                                        messages: vec![],
                                        at_top: false,
                                        messages_prev_batch: None,
                                        last_updated_sync: None,
                                    };
                                    v.insert(channel);
                                }
                            }

                            let message = Message {
                                user: message.sender.to_string(),
                                content: message.content.body().to_string(),
                                timestamp: message.origin_server_ts.as_secs(),
                            };
                            let channel = lock.channels.get_mut(&id).unwrap();
                            channel.last_updated_sync = last_updated_sync;

                            for i in (0..=channel.messages.len()).rev() {
                                if i == 0 {
                                    channel.messages.insert(0, message);
                                    break;
                                }

                                if channel.messages[i - 1].timestamp <= message.timestamp {
                                    channel.messages.insert(i, message);
                                    break;
                                }
                            }
                        }

                        SyncMessageLikeEvent::Redacted(_) => (),
                    }
                }
            });
    }

    client.sync_once(SyncSettings::default()).await.unwrap();

    {
        let mut lock = state.lock().await;
        for room in lock.client.joined_rooms() {
            let id = room.room_id().to_owned();
            if let Entry::Vacant(v) = lock.channels.entry(id.clone()) {
                v.insert(Channel {
                    name: room.display_name().await.map(|v| v.to_string()).unwrap_or_else(|_| String::from("[unknown room]")),
                    room,
                    messages: vec![],
                    at_top: false,
                    messages_prev_batch: None,
                    last_updated_sync: None,
                });
            }
            lock.channel_ids.push(id);
        }
    }

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
            let horizontal = layout::Layout::default()
                .direction(layout::Direction::Horizontal)
                .constraints([
                    layout::Constraint::Length(20),
                    layout::Constraint::Min(3),
                ])
                .split(f.size());
            let content = layout::Layout::default()
                .direction(layout::Direction::Vertical)
                .constraints([
                    layout::Constraint::Min(3),
                    layout::Constraint::Length(3),
                    layout::Constraint::Length(1),
                ])
                .split(horizontal[1]);

            let channels = widgets::Block::default().borders(widgets::Borders::ALL);
            let channels_list: Vec<_> = state.channel_ids.iter().filter_map(|id| {
                state.channels.get(id).map(|v| vec![Spans::from(vec![Span::raw(&v.name)])])
            })
            .map(|v| widgets::ListItem::new(Text::from(v))).collect();
            let channels = widgets::List::new(channels_list)
                .highlight_style(Style::default().bg(Color::Magenta))
                .block(channels);
            f.render_stateful_widget(channels, horizontal[0], &mut state.channels_state.clone());

            let messages = widgets::Block::default().borders(widgets::Borders::ALL);
            match state.current_channel.as_ref().and_then(|v| state.channels.get(v)).map(|v| &v.messages) {
                Some(current) => {
                    let messages_list: Vec<_> = current.iter().rev().map(|v| {
                        vec![Spans::from(vec![Span::raw(&v.user)]), Spans::from(vec![Span::raw(&v.content)])]
                    })
                    .map(|v| widgets::ListItem::new(Text::from(v))).collect();
                    let messages = widgets::List::new(messages_list)
                        .highlight_style(Style::default().bg(Color::Magenta))
                        .block(messages)
                        .start_corner(layout::Corner::BottomLeft);
                    f.render_stateful_widget(messages, content[0], &mut state.messages_state.clone());
                }

                None => {
                    f.render_widget(messages, content[0]);
                }
            }

            let input = widgets::Block::default().borders(widgets::Borders::ALL);
            let input = widgets::Paragraph::new(state.input_text.as_str()).block(input);
            f.render_widget(input, content[1]);

            let status = {
                match state.mode {
                    Mode::Insert => Span::raw("INSERT"),
                    Mode::Normal => Span::raw("NORMAL"),
                    Mode::SelectChannel => Span::raw("SELECT"),
                    Mode::ScrollMessages => Span::raw("SCROLL"),
                }
            };
            let status = widgets::Paragraph::new(status);
            f.render_widget(status, content[2]);

            match state.mode {
                Mode::Insert => {
                    use crossterm::cursor::{CursorShape, SetCursorShape};
                    crossterm::execute!(stdout, SetCursorShape(CursorShape::Line)).unwrap();
                    let m = state.input_char_pos as u16 % (content[1].width - 2);
                    if m == 0 && state.input_char_pos != 0 {
                        f.set_cursor(
                            content[1].x + content[1].width - 1,
                            content[1].y
                                + (state.input_char_pos as u16 - 1) / (content[1].width - 2)
                                + 1,
                        );
                    } else {
                        f.set_cursor(
                            content[1].x + m + 1,
                            content[1].y + state.input_char_pos as u16 / (content[1].width - 2) + 1,
                        );
                    }
                }

                Mode::Normal => {
                    use crossterm::cursor::{CursorShape, SetCursorShape};
                    crossterm::execute!(stdout, SetCursorShape(CursorShape::Block)).unwrap();
                    let m = state.input_char_pos as u16 % (content[1].width - 2);
                    if m == 0 && state.input_char_pos != 0 {
                        f.set_cursor(
                            content[1].x + content[1].width - 1,
                            content[1].y
                                + (state.input_char_pos as u16 - 1) / (content[1].width - 2)
                                + 1,
                        );
                    } else {
                        f.set_cursor(
                            content[1].x + m + 1,
                            content[1].y + state.input_char_pos as u16 / (content[1].width - 2) + 1,
                        );
                    }
                }

                _ => (),
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
                                if let Some(room) = state.current_channel.as_ref().and_then(|v| state.channels.get(v)).map(|v| &v.room) {
                                    room.send(
                                        RoomMessageEventContent::text_plain(state.input_text.clone()),
                                        None,
                                    )
                                    .await
                                    .unwrap();
                                }
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
                                    if let Some(room) = state.current_channel.as_ref().and_then(|v| state.channels.get(v)).map(|v| &v.room) {
                                        room.send(
                                            RoomMessageEventContent::text_plain(state.input_text.clone()),
                                            None,
                                        )
                                        .await
                                        .unwrap();
                                    }
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

                            KeyCode::Char('C') => {
                                state.mode = Mode::SelectChannel;
                            }

                            KeyCode::Char('S') => {
                                if state.current_channel.clone().and_then(|v| state.channels.get_mut(&v)).is_some() {
                                    state.messages_state.select(Some(0));
                                    state.mode = Mode::ScrollMessages;
                                }
                            }

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

            Mode::SelectChannel => {
                match event {
                    Event::FocusGained => (),
                    Event::FocusLost => (),

                    Event::Key(key) => {
                        match key.code {
                            KeyCode::Backspace => (),

                            KeyCode::Enter => {
                                state.current_channel = state.channels_state.selected().and_then(|v| state.channel_ids.get(v)).cloned();
                                state.mode = Mode::Normal;
                            }

                            KeyCode::Left => (),
                            KeyCode::Right => (),

                            KeyCode::Up | KeyCode::Char('k') => {
                                match state.channels_state.selected() {
                                    Some(current) => {
                                        if current > 0 {
                                            state.channels_state.select(Some(current - 1));
                                        } else {
                                            let select = state.channel_ids.len() - 1;
                                            state.channels_state.select(Some(select));
                                        }
                                    }

                                    None => {
                                        let select = state.channel_ids.len() - 1;
                                        state.channels_state.select(Some(select));
                                    }
                                }
                            }

                            KeyCode::Down | KeyCode::Char('j') => {
                                match state.channels_state.selected() {
                                    Some(current) => {
                                        if current < state.channel_ids.len() - 1 {
                                            state.channels_state.select(Some(current + 1));
                                        } else {
                                            state.channels_state.select(Some(0));
                                        }
                                    }

                                    None => {
                                        let select = state.channel_ids.len() - 1;
                                        state.channels_state.select(Some(select));
                                    }
                                }
                            }

                            KeyCode::Esc => {
                                state.channels_state.select(None);
                                state.current_channel = None;
                                state.mode = Mode::Normal;
                            }

                            KeyCode::Home => (),
                            KeyCode::End => (),
                            KeyCode::PageUp => (),
                            KeyCode::PageDown => (),
                            KeyCode::Tab => (),
                            KeyCode::BackTab => (),
                            KeyCode::Delete => (),
                            KeyCode::Insert => (),
                            KeyCode::F(_) => (),
                            KeyCode::Char(_) => (),
                            KeyCode::Null => (),
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

            Mode::ScrollMessages => {
                match event {
                    Event::FocusGained => (),
                    Event::FocusLost => (),

                    Event::Key(key) => {
                        match key.code {
                            KeyCode::Backspace => (),
                            KeyCode::Enter => (),
                            KeyCode::Left => (),
                            KeyCode::Right => (),

                            KeyCode::Up | KeyCode::Char('k') => {
                                if let Some(channel) = state.current_channel.as_ref().and_then(|v| state.channels.get(v)) {
                                    match state.messages_state.selected() {
                                        Some(current) => {
                                            if current < channel.messages.len() - 1 {
                                                state.messages_state.select(Some(current + 1));
                                            } else if let Some(current) = state.current_channel.clone().and_then(|v| state.channels.get_mut(&v)) {
                                                if !current.at_top {
                                                    let mut options = MessagesOptions::backward();
                                                    options.limit = UInt::from(50u32);
                                                    options.from = current.messages_prev_batch.as_ref().or_else(|| current.last_updated_sync.as_ref()).map(|v| v.as_str());
                                                    match current.room.messages(options).await {
                                                        Ok(v) => {
                                                            current.at_top = v.end.is_none();
                                                            current.messages_prev_batch = v.end;
                                                            for event in v.chunk.into_iter() {
                                                                if let Ok(v) = event.event.deserialize() {
                                                                    if let AnyTimelineEvent::MessageLike(AnyMessageLikeEvent::RoomMessage(MessageLikeEvent::Original(v))) = v {
                                                                        let message = Message {
                                                                            user: v.sender.to_string(),
                                                                            content: v.content.body().to_string(),
                                                                            timestamp: v.origin_server_ts.as_secs(),
                                                                        };
                                                                        current.messages.insert(0, message);
                                                                    }
                                                                }
                                                            }
                                                        }

                                                        Err(_) => (),
                                                    }
                                                }
                                            }
                                        }

                                        None => {
                                            if !channel.messages.is_empty() {
                                                state.channels_state.select(Some(0));
                                            }
                                        }
                                    }
                                }
                            }

                            KeyCode::Down | KeyCode::Char('j') => {
                                match state.messages_state.selected() {
                                    Some(current) => {
                                        if current > 0 {
                                            state.messages_state.select(Some(current - 1));
                                        }
                                    }

                                    None => {
                                        if let Some(channel) = state.current_channel.as_ref().and_then(|v| state.channels.get(v)) {
                                            if !channel.messages.is_empty() {
                                                state.channels_state.select(Some(0));
                                            }
                                        }
                                    }
                                }
                            }

                            KeyCode::Home => (),
                            KeyCode::End => (),
                            KeyCode::PageUp => (),
                            KeyCode::PageDown => (),
                            KeyCode::Tab => (),
                            KeyCode::BackTab => (),
                            KeyCode::Delete => (),
                            KeyCode::Insert => (),
                            KeyCode::F(_) => (),

                            KeyCode::Char(_) => (),

                            KeyCode::Null => (),

                            KeyCode::Esc => {
                                state.messages_state.select(None);
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
                    }

                    Event::Mouse(_) => (),
                    Event::Paste(_) => (),
                    Event::Resize(_, _) => (),
                }
            }
        }
    }
}
