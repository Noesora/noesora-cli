use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use noesora_engine::index::{self, SearchHit, SearchResult};
use noesora_engine::record;
use noesora_engine::vault;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Browse,
    Search,
}

struct App {
    root: PathBuf,
    files: Vec<PathBuf>,
    selected_file: usize,
    preview: String,
    mode: Mode,
    query: String,
    results: Option<SearchResult>,
    selected_hit: usize,
}

impl App {
    fn new(root: PathBuf) -> io::Result<Self> {
        let files = markdown_files(&record::records_dir(&root))?;
        let preview = files
            .first()
            .map(fs::read_to_string)
            .transpose()?
            .unwrap_or_else(|| "No Markdown records in this vault.".to_owned());
        Ok(Self {
            root,
            files,
            selected_file: 0,
            preview,
            mode: Mode::Browse,
            query: String::new(),
            results: None,
            selected_hit: 0,
        })
    }

    fn select_file(&mut self, offset: isize) -> io::Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        self.selected_file =
            (self.selected_file as isize + offset).rem_euclid(self.files.len() as isize) as usize;
        self.preview = fs::read_to_string(&self.files[self.selected_file])?;
        Ok(())
    }

    fn search(&mut self) -> Result<(), index::IndexError> {
        self.results = Some(index::search(&self.root, &self.query)?);
        self.selected_hit = 0;
        Ok(())
    }

    fn move_hit(&mut self, offset: isize) {
        let Some(SearchResult::Hits(hits)) = &self.results else {
            return;
        };
        if !hits.is_empty() {
            self.selected_hit =
                (self.selected_hit as isize + offset).rem_euclid(hits.len() as isize) as usize;
        }
    }
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let cwd = std::env::current_dir()?;
    let root = vault::find_vault(&cwd).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no vault found; run `noesora init`",
        )
    })?;
    vault::read_vault(&root)?;
    let mut app = App::new(root)?;
    let mut terminal = ratatui::try_init()?;
    let _restore = RestoreTerminal;
    run_app(&mut terminal, &mut app)?;
    Ok(())
}

struct RestoreTerminal;

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = ratatui::try_restore();
    }
}

fn run_app(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<(), Box<dyn Error>> {
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match (app.mode, key.code) {
                (_, KeyCode::Char('q')) => break,
                (Mode::Search, KeyCode::Esc) => app.mode = Mode::Browse,
                (Mode::Browse, KeyCode::Esc) => break,
                (Mode::Browse, KeyCode::Char('/')) => {
                    app.mode = Mode::Search;
                    app.query.clear();
                    app.results = None;
                }
                (Mode::Search, KeyCode::Enter) => app.search()?,
                (Mode::Search, KeyCode::Backspace) => {
                    app.query.pop();
                    app.results = None;
                }
                (Mode::Search, KeyCode::Char(ch)) => {
                    if !ch.is_control() {
                        app.query.push(ch);
                        app.results = None;
                    }
                }
                (Mode::Browse, KeyCode::Up) => app.select_file(-1)?,
                (Mode::Browse, KeyCode::Down) => app.select_file(1)?,
                (Mode::Search, KeyCode::Up) => app.move_hit(-1),
                (Mode::Search, KeyCode::Down) => app.move_hit(1),
                _ => {}
            }
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let root = clean(&app.root.display().to_string());
    let heading = if app.mode == Mode::Search {
        format!("Vault: {root}\nSearch: {}", clean(&app.query))
    } else {
        format!("Vault: {root}")
    };
    frame.render_widget(Paragraph::new(heading), chunks[0]);
    let footer = if app.mode == Mode::Search {
        "Enter search · ↑/↓ results · Esc back · q quit"
    } else {
        "↑/↓ browse · / search · q quit"
    };
    frame.render_widget(Paragraph::new(footer), chunks[2]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(chunks[1]);
    match app.mode {
        Mode::Browse => draw_files(frame, app, body[0], body[1]),
        Mode::Search => draw_search(frame, app, body[0], body[1]),
    }
}

fn draw_files(
    frame: &mut Frame,
    app: &App,
    left: ratatui::layout::Rect,
    right: ratatui::layout::Rect,
) {
    let records = record::records_dir(&app.root);
    let items = app
        .files
        .iter()
        .map(|path| {
            let relative = path.strip_prefix(&records).unwrap_or(path);
            ListItem::new(clean(&relative.display().to_string()))
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .title("Markdown files")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select((!app.files.is_empty()).then_some(app.selected_file));
    frame.render_stateful_widget(list, left, &mut state);

    let title = app.files.get(app.selected_file).map_or_else(
        || "Preview".to_owned(),
        |path| {
            clean(
                &path
                    .strip_prefix(&records)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            )
        },
    );
    frame.render_widget(
        Paragraph::new(clean(&app.preview))
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        right,
    );
}

fn draw_search(
    frame: &mut Frame,
    app: &App,
    left: ratatui::layout::Rect,
    right: ratatui::layout::Rect,
) {
    let hits = match &app.results {
        Some(SearchResult::Hits(hits)) => hits.as_slice(),
        _ => &[],
    };
    let items = match &app.results {
        Some(SearchResult::Refused) => vec![ListItem::new("No evidence found.")],
        Some(SearchResult::Hits(hits)) if hits.is_empty() => {
            vec![ListItem::new("No evidence found.")]
        }
        Some(SearchResult::Hits(hits)) => hits
            .iter()
            .map(|hit| ListItem::new(format!("{} [{}]", clean(&hit.title), clean(&hit.kind))))
            .collect(),
        None => vec![ListItem::new("Type a query and press Enter.")],
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title("Cited search results")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select((!hits.is_empty()).then_some(app.selected_hit));
    frame.render_stateful_widget(list, left, &mut state);

    let detail = if let Some(hit) = hits.get(app.selected_hit) {
        render_hit(hit)
    } else {
        match &app.results {
            Some(SearchResult::Refused) => "Refused: no evidence found.".to_owned(),
            Some(SearchResult::Hits(_)) => "No evidence found.".to_owned(),
            None => "Search uses local indexed evidence.".to_owned(),
        }
    };
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().title("Evidence").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        right,
    );
}

fn render_hit(hit: &SearchHit) -> String {
    format!(
        "{}\nID: {}\nType: {}\nPath: {}\nSpan: {}\nHash: {}\n\n{}",
        clean(&hit.title),
        clean(&hit.id),
        clean(&hit.kind),
        clean(&hit.path),
        clean(&hit.span),
        clean(&hit.hash),
        clean(&hit.text)
    )
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' | '\t' => ch,
            ch if ch.is_control() => '�',
            ch => ch,
        })
        .collect()
}

fn markdown_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_markdown_files(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_markdown_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_markdown_files(&path, files)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use noesora_engine::record::RecordStatus;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "noesora-tui-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create isolated fixture");
        path
    }

    #[test]
    fn browser_lists_nested_markdown_in_stable_order() {
        let dir = temp_dir();
        fs::create_dir(dir.join("nested")).unwrap();
        fs::write(dir.join("z.md"), "last").unwrap();
        fs::write(dir.join("a.md"), "first").unwrap();
        fs::write(dir.join("nested/b.md"), "nested").unwrap();
        fs::write(dir.join("ignored.txt"), "not markdown").unwrap();

        let files = markdown_files(&dir).unwrap();
        let relative = files
            .iter()
            .map(|path| {
                path.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(relative, ["a.md", "nested/b.md", "z.md"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn search_shows_engine_citations_without_changing_canonical_records() {
        let dir = temp_dir();
        let root = vault::init_vault(&dir).unwrap();
        let written = record::write_note(
            &root,
            "Workspace credits",
            "Keep credits on the workspace.",
            RecordStatus::Open,
            &[],
        )
        .unwrap();
        let before = fs::read(&written.path).unwrap();
        let mut app = App::new(root.clone()).unwrap();
        app.query = "workspace credits".to_owned();
        app.search().unwrap();

        let Some(SearchResult::Hits(hits)) = app.results else {
            panic!("expected cited search hit");
        };
        let hit = &hits[0];
        assert_eq!(hit.title, "Workspace credits");
        assert_eq!(
            hit.path,
            format!(
                "records/{}",
                written.path.file_name().unwrap().to_string_lossy()
            )
        );
        assert!(!hit.span.is_empty());
        assert!(!hit.hash.is_empty());
        let source = String::from_utf8(before.clone()).unwrap();
        let (start, end) = hit
            .span
            .strip_prefix('B')
            .unwrap()
            .split_once("-B")
            .unwrap();
        let start = start.parse::<usize>().unwrap();
        let end = end.parse::<usize>().unwrap();
        assert_eq!(source.get(start..end), Some(hit.text.as_str()));
        let detail = render_hit(hit);
        assert!(detail.contains(&format!("Path: {}", hit.path)));
        assert!(detail.contains(&format!("Span: {}", hit.span)));
        assert!(detail.contains(&format!("Hash: {}", hit.hash)));
        assert_eq!(fs::read(&written.path).unwrap(), before);
        assert!(!root.join(".noesora/candidates").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
