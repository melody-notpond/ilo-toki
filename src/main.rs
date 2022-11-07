use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crossterm::{
    event::{Event, KeyCode},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{backend::CrosstermBackend, layout, widgets, Terminal};

struct AppState {}

static RUNNING: AtomicBool = AtomicBool::new(true);

#[tokio::main]
async fn main() -> Result<(), io::Error> {
    tokio::task::spawn(ui_events());
    main_ui().await
}

async fn main_ui() -> Result<(), io::Error> {
    let stdout = io::stdout();
    crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    crossterm::terminal::enable_raw_mode()?;
    terminal.clear()?;

    while RUNNING.load(Ordering::Acquire) {
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
            f.render_widget(messages, vertical[0]);

            let input = widgets::Block::default().borders(widgets::Borders::ALL);
            f.render_widget(input, vertical[1]);

            let status = widgets::Paragraph::new("uwu");
            f.render_widget(status, vertical[2]);
        })?;

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    terminal.clear()?;
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    terminal.set_cursor(0, 0)?;

    Ok(())
}

async fn ui_events() {
    while let Ok(Ok(event)) = tokio::task::spawn_blocking(crossterm::event::read).await {
        match event {
            Event::FocusGained => (),
            Event::FocusLost => (),
            Event::Resize(_, _) => (),

            Event::Key(key) => match key.code {
                KeyCode::Backspace => (),
                KeyCode::Enter => (),
                KeyCode::Left => (),
                KeyCode::Right => (),
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
                KeyCode::Char(_) => (),
                KeyCode::Null => (),

                KeyCode::Esc => {
                    RUNNING.store(false, Ordering::Release);
                    break;
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
            },

            Event::Mouse(_) => (),
            Event::Paste(_) => (),
        }
    }
}
