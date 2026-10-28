use std::error::Error;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use noesora_engine::{SearchFilters, Store};
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::memory_ui::{self, App, MemoryAction};

pub fn run(store: Store, filters: SearchFilters) -> Result<(), Box<dyn Error + Send + Sync>> {
    let _restore = RestoreTerminal;
    let mut terminal = ratatui::try_init()?;
    let mut ui = App::new(store, filters);
    run_app(&mut terminal, &mut ui)
}

struct RestoreTerminal;

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = ratatui::try_restore();
    }
}

fn run_app(
    terminal: &mut ratatui::DefaultTerminal,
    ui: &mut App,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    loop {
        ui.tick();
        terminal.draw(|frame| draw(frame, frame.area(), ui))?;
        if event::poll(Duration::from_millis(60))? {
            if let Event::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    break;
                }
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                    && matches!(ui.handle_key(key.code), MemoryAction::Close)
                {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame, area: Rect, ui: &App) {
    memory_ui::draw(frame, area, ui);
}
