use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
#[cfg(test)]
use std::time::Duration;

use crossterm::event::KeyCode;
use noesora_engine::{
    embedding, SearchError, SearchFilters, SearchMode, SearchResponse, SessionSummary, Store,
    StoreStatus, TranscriptPage,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::theme::{sanitize_terminal_text, ACCENT, ACCENT_SOFT, INK, LINE, MUTED, RAISED, SILVER};

const SESSION_PAGE_SIZE: usize = 50;
const SESSION_FETCH_LIMIT: usize = SESSION_PAGE_SIZE + 1;
const TRANSCRIPT_PAGE_SIZE: usize = 20;
#[cfg(test)]
const SEARCH_LIMIT: usize = 10;
const NARROW_WIDTH: u16 = 92;
const TRANSCRIPT_CHECKPOINT_INTERVAL: usize = 16_384;
const TRANSCRIPT_HISTORY_ROWS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Sessions,
    Search,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    List,
    Detail,
    Input,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    None,
    Quit,
    SearchStarted,
    QueryChanged,
    RunSearch,
    BackToSessions,
    OpenSession,
    MoveSelection(isize),
    SelectMessage(isize),
    PageSessions(isize),
    PageTranscript(isize),
    ScrollDetail(isize),
    ToggleTool,
    ToggleSearchMode,
    Refresh,
}

#[derive(Debug)]
pub enum MemoryAction {
    None,
    Close,
}

struct Navigation {
    screen: Screen,
    focus: Focus,
    query: String,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            screen: Screen::Sessions,
            focus: Focus::List,
            query: String::new(),
        }
    }
}

impl Navigation {
    fn handle_key(&mut self, code: KeyCode) -> Action {
        if self.screen == Screen::Search && self.focus == Focus::Input {
            return match code {
                KeyCode::Char(ch) if !ch.is_control() => {
                    self.query.push(ch);
                    Action::QueryChanged
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    Action::QueryChanged
                }
                KeyCode::Enter => {
                    self.focus = Focus::List;
                    Action::RunSearch
                }
                KeyCode::Esc => {
                    self.screen = Screen::Sessions;
                    self.focus = Focus::List;
                    Action::BackToSessions
                }
                KeyCode::Tab => {
                    self.focus = Focus::List;
                    Action::None
                }
                _ => Action::None,
            };
        }

        match code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char('/') => {
                self.screen = Screen::Search;
                self.focus = Focus::Input;
                self.query.clear();
                Action::SearchStarted
            }
            KeyCode::Esc if self.screen == Screen::Search => {
                self.screen = Screen::Sessions;
                self.focus = Focus::List;
                Action::BackToSessions
            }
            KeyCode::Esc if self.focus == Focus::Detail => {
                self.focus = Focus::List;
                Action::None
            }
            KeyCode::Esc => Action::Quit,
            KeyCode::Tab => {
                self.focus = match (self.screen, self.focus) {
                    (Screen::Sessions, Focus::List) => Focus::Detail,
                    (Screen::Sessions, _) => Focus::List,
                    (Screen::Search, Focus::Input) => Focus::List,
                    (Screen::Search, Focus::List) => Focus::Detail,
                    (Screen::Search, Focus::Detail) => Focus::Input,
                };
                Action::None
            }
            KeyCode::Enter => match (self.screen, self.focus) {
                (Screen::Sessions, Focus::List) => {
                    self.focus = Focus::Detail;
                    Action::OpenSession
                }
                (Screen::Sessions, Focus::Detail) => Action::PageTranscript(1),
                (Screen::Search, Focus::List) => {
                    self.focus = Focus::Detail;
                    Action::None
                }
                _ => Action::None,
            },
            KeyCode::Up => match (self.screen, self.focus) {
                (Screen::Sessions, Focus::List) | (Screen::Search, Focus::List) => {
                    Action::MoveSelection(-1)
                }
                (_, Focus::Detail) => Action::ScrollDetail(-1),
                _ => Action::None,
            },
            KeyCode::Down => match (self.screen, self.focus) {
                (Screen::Sessions, Focus::List) | (Screen::Search, Focus::List) => {
                    Action::MoveSelection(1)
                }
                (_, Focus::Detail) => Action::ScrollDetail(1),
                _ => Action::None,
            },
            KeyCode::Left if self.screen == Screen::Sessions && self.focus == Focus::Detail => {
                Action::SelectMessage(-1)
            }
            KeyCode::Right if self.screen == Screen::Sessions && self.focus == Focus::Detail => {
                Action::SelectMessage(1)
            }
            KeyCode::PageUp => match (self.screen, self.focus) {
                (Screen::Sessions, Focus::List) => Action::PageSessions(-1),
                (Screen::Sessions, Focus::Detail) => Action::PageTranscript(-1),
                (Screen::Search, Focus::Detail) => Action::ScrollDetail(-10),
                (Screen::Search, Focus::List) => Action::MoveSelection(-10),
                _ => Action::None,
            },
            KeyCode::PageDown => match (self.screen, self.focus) {
                (Screen::Sessions, Focus::List) => Action::PageSessions(1),
                (Screen::Sessions, Focus::Detail) => Action::PageTranscript(1),
                (Screen::Search, Focus::Detail) => Action::ScrollDetail(10),
                (Screen::Search, Focus::List) => Action::MoveSelection(10),
                _ => Action::None,
            },
            KeyCode::Char('x')
                if self.screen == Screen::Sessions && self.focus == Focus::Detail =>
            {
                Action::ToggleTool
            }
            KeyCode::Char('m') if self.screen == Screen::Search => Action::ToggleSearchMode,
            KeyCode::Char('r') if self.screen == Screen::Sessions && self.focus == Focus::List => {
                Action::Refresh
            }
            _ => Action::None,
        }
    }
}

#[derive(Clone, Default)]
struct Generation(Arc<AtomicU64>);

impl Generation {
    fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    fn current(&self, token: u64) -> bool {
        self.0.load(Ordering::Acquire) == token
    }
}

#[derive(Clone, Copy)]
enum Pending {
    None,
    Startup,
    Sessions,
    Transcript,
    Search,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StartupState {
    #[default]
    Needed,
    Loading,
    Settled,
}

impl StartupState {
    fn begin(&mut self) {
        *self = Self::Loading;
    }

    fn cancel(&mut self) {
        if *self == Self::Loading {
            *self = Self::Needed;
        }
    }

    fn settle(&mut self) {
        if *self == Self::Loading {
            *self = Self::Settled;
        }
    }

    fn needs_request(self) -> bool {
        self == Self::Needed
    }
}

#[derive(Clone)]
enum Readiness {
    NotLoaded,
    Loading,
    Ready {
        chunks: usize,
        embedded: usize,
    },
    IndexIncomplete {
        chunks: usize,
        embedded: usize,
        pending: usize,
    },
    ModelUnavailable {
        pending: usize,
    },
    Error(String),
}
enum SearchFailure {
    Engine(SearchError),
    WorkerStart(String),
}

enum WorkerPayload {
    Startup {
        sessions: Result<Vec<SessionSummary>, String>,
        status: Result<StoreStatus, String>,
        model: noesora_engine::ModelStatus,
    },
    Sessions {
        offset: usize,
        sessions: Result<Vec<SessionSummary>, String>,
    },
    Transcript {
        id: String,
        offset: usize,
        result: Result<TranscriptPage, String>,
    },
    Search {
        query: String,
        result: Result<SearchResponse, SearchFailure>,
    },
}

struct WorkerMessage {
    generation: u64,
    payload: WorkerPayload,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct LineOffset {
    span: usize,
    byte: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptSection {
    Header,
    CollapsedTool,
    EmptyBody,
    Body { line_start: usize, line_end: usize },
}

// Ratatui 0.30's pending-line/word/whitespace state, stored as source offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WrapCursor {
    input: LineOffset,
    row_start: LineOffset,
    line_width: u16,
    pending_line_end: LineOffset,
    pending_line: bool,
    word_width: u16,
    word_start: LineOffset,
    word_end: LineOffset,
    word: bool,
    whitespace_width: u16,
    whitespace_start: LineOffset,
    whitespace_end: LineOffset,
    whitespace: bool,
    non_whitespace_previous: bool,
    emitted_row: bool,
}

impl WrapCursor {
    fn new(start: LineOffset) -> Self {
        Self {
            input: start,
            row_start: start,
            line_width: 0,
            pending_line_end: start,
            pending_line: false,
            word_width: 0,
            word_start: start,
            word_end: start,
            word: false,
            whitespace_width: 0,
            whitespace_start: start,
            whitespace_end: start,
            whitespace: false,
            non_whitespace_previous: false,
            emitted_row: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TranscriptPosition {
    message: usize,
    section: TranscriptSection,
    wrap: WrapCursor,
}

#[derive(Clone, Copy)]
struct TranscriptCheckpoint {
    row: usize,
    position: Option<TranscriptPosition>,
}

// Stores only the active row, recent navigation history, and sparse jump points.
#[derive(Default)]
struct TranscriptViewport {
    width: Option<u16>,
    row: usize,
    cursor: Option<TranscriptPosition>,
    checkpoints: Vec<TranscriptCheckpoint>,
    history: VecDeque<TranscriptCheckpoint>,
}

impl TranscriptViewport {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

struct WrappedRow {
    end: LineOffset,
    next: Option<WrapCursor>,
}

struct SourceGrapheme {
    start: LineOffset,
    end: LineOffset,
    width: u16,
    whitespace: bool,
    ignored: bool,
}

struct TranscriptDisplayLine<'a> {
    line: Line<'a>,
    next: Option<TranscriptPosition>,
}

struct TranscriptLineSource<'a> {
    style: Style,
    spans: [Option<Span<'a>>; 5],
    len: usize,
}

impl<'a> TranscriptLineSource<'a> {
    fn new(style: Style) -> Self {
        Self {
            style,
            spans: std::array::from_fn(|_| None),
            len: 0,
        }
    }

    fn push(&mut self, span: Span<'a>) {
        debug_assert!(self.len < self.spans.len());
        self.spans[self.len] = Some(span);
        self.len += 1;
    }

    fn span(&self, index: usize) -> Option<&Span<'a>> {
        self.spans.get(index).and_then(Option::as_ref)
    }
}

pub struct App {
    store: Store,
    filters: SearchFilters,
    navigation: Navigation,
    search_mode: SearchMode,
    sessions: Vec<SessionSummary>,
    sessions_offset: usize,
    has_more_sessions: bool,
    selected_session: usize,
    select_after_page: usize,
    transcript: Option<TranscriptPage>,
    transcript_request: Option<String>,
    transcript_request_offset: Option<usize>,
    transcript_error: Option<String>,
    selected_message: usize,
    transcript_scroll: usize,
    expanded_tools: HashSet<String>,
    transcript_viewport: RefCell<TranscriptViewport>,
    search_results: Option<SearchResponse>,
    search_applied_query: Option<String>,
    search_error: Option<SearchFailure>,
    selected_hit: usize,
    evidence_scroll: usize,
    session_error: Option<String>,
    store_status: Option<StoreStatus>,
    readiness: Readiness,
    pending: Pending,
    startup: StartupState,
    generation: Generation,
    sender: mpsc::Sender<WorkerMessage>,
    receiver: mpsc::Receiver<WorkerMessage>,
}

impl App {
    pub fn new(store: Store, filters: SearchFilters) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut app = Self {
            store,
            filters,
            navigation: Navigation::default(),
            search_mode: SearchMode::Text,
            sessions: Vec::new(),
            sessions_offset: 0,
            has_more_sessions: false,
            selected_session: 0,
            select_after_page: 0,
            transcript: None,
            transcript_request: None,
            transcript_request_offset: None,
            transcript_error: None,
            selected_message: 0,
            transcript_scroll: 0,
            transcript_viewport: RefCell::default(),
            expanded_tools: HashSet::new(),
            search_results: None,
            search_applied_query: None,
            search_error: None,
            selected_hit: 0,
            evidence_scroll: 0,
            session_error: None,
            store_status: None,
            readiness: Readiness::Loading,
            pending: Pending::None,
            startup: StartupState::Needed,
            generation: Generation::default(),
            sender,
            receiver,
        };
        app.request_startup();
        app
    }
    pub fn tick(&mut self) {
        self.poll_workers();
    }

    fn request_startup(&mut self) {
        self.startup.begin();
        self.pending = Pending::Startup;
        self.session_error = None;
        self.readiness = Readiness::Loading;
        self.transcript_request = None;
        self.transcript_request_offset = None;
        self.launch(|store| {
            let sessions = store
                .sessions(None, 0, SESSION_FETCH_LIMIT)
                .map_err(|err| err.to_string());
            let status = store.status().map_err(|err| err.to_string());
            let model = embedding::model_status(&store.model_dir());
            WorkerPayload::Startup {
                sessions,
                status,
                model,
            }
        });
    }

    fn request_sessions(&mut self, offset: usize, selected: usize) {
        self.pending = Pending::Sessions;
        self.select_after_page = selected;
        self.session_error = None;
        self.transcript = None;
        self.transcript_request = None;
        self.transcript_request_offset = None;
        self.transcript_error = None;
        self.transcript_viewport.get_mut().reset();
        self.launch(move |store| WorkerPayload::Sessions {
            offset,
            sessions: store
                .sessions(None, offset, SESSION_FETCH_LIMIT)
                .map_err(|err| err.to_string()),
        });
    }

    fn request_transcript(&mut self, id: String, offset: usize) {
        self.pending = Pending::Transcript;
        self.transcript_error = None;
        self.transcript = None;
        self.transcript_scroll = 0;
        self.transcript_viewport.get_mut().reset();
        self.selected_message = 0;
        self.transcript_request = Some(id.clone());
        self.transcript_request_offset = Some(offset);
        self.launch(move |store| {
            let result = store
                .read_session(&id, offset, TRANSCRIPT_PAGE_SIZE)
                .map_err(|err| err.to_string());
            WorkerPayload::Transcript { id, offset, result }
        });
    }

    fn request_search(&mut self) {
        self.pending = Pending::Search;
        self.search_results = None;
        self.search_applied_query = None;
        self.search_error = None;
        self.selected_hit = 0;
        self.evidence_scroll = 0;
        let query = self.navigation.query.clone();
        let mode = self.search_mode;
        let filters = self.filters.clone();
        self.launch(move |store| {
            let result = store
                .search(&query, mode, &filters)
                .map_err(SearchFailure::Engine);
            WorkerPayload::Search { query, result }
        });
    }

    fn launch<F>(&mut self, work: F)
    where
        F: FnOnce(Store) -> WorkerPayload + Send + 'static,
    {
        if !matches!(self.pending, Pending::Startup) {
            self.cancel_startup();
        }
        let token = self.generation.next();
        let generation = self.generation.clone();
        let sender = self.sender.clone();
        let store = self.store.clone();
        let spawned = thread::Builder::new()
            .name("noesora-tui-worker".to_owned())
            .spawn(move || {
                if !generation.current(token) {
                    return;
                }
                let payload = work(store);
                if generation.current(token) {
                    let _ = sender.send(WorkerMessage {
                        generation: token,
                        payload,
                    });
                }
            });
        if let Err(err) = spawned {
            self.worker_start_failed(err);
        }
    }

    fn worker_start_failed(&mut self, err: std::io::Error) {
        let message = format!("Could not start background worker: {err}");
        if matches!(self.pending, Pending::Startup) {
            self.cancel_startup();
        }
        match self.pending {
            Pending::Startup | Pending::Sessions | Pending::None => {
                self.session_error = Some(message);
            }
            Pending::Transcript => {
                self.transcript_request = None;
                self.transcript_request_offset = None;
                self.transcript_error = Some(message);
            }
            Pending::Search => {
                self.search_applied_query = None;
                self.search_error = Some(SearchFailure::WorkerStart(message));
            }
        }
        self.pending = Pending::None;
    }

    fn poll_workers(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            if !self.generation.current(message.generation) {
                continue;
            }
            self.pending = Pending::None;
            match message.payload {
                WorkerPayload::Startup {
                    sessions,
                    status,
                    model,
                } => {
                    self.startup.settle();
                    self.apply_readiness(status, model);
                    self.apply_sessions(sessions, 0, 0);
                }
                WorkerPayload::Sessions { offset, sessions } => {
                    self.apply_sessions(sessions, offset, self.select_after_page);
                }
                WorkerPayload::Transcript { id, offset, result } => {
                    if self.transcript_request.as_deref() != Some(id.as_str())
                        || self.transcript_request_offset != Some(offset)
                        || self
                            .sessions
                            .get(self.selected_session)
                            .is_none_or(|session| session.id != id)
                    {
                        self.transcript_request = None;
                        self.transcript_request_offset = None;
                        continue;
                    }
                    self.transcript_request = None;
                    self.transcript_request_offset = None;
                    self.transcript_viewport.get_mut().reset();
                    match result {
                        Ok(page) if page.session.id == id && page.offset == offset => {
                            self.transcript_error = None;
                            self.selected_message = 0;
                            self.transcript = Some(page);
                        }
                        Ok(_) => {
                            self.transcript = None;
                            self.transcript_error = Some(
                                "Transcript identity or offset did not match request".to_owned(),
                            );
                        }
                        Err(err) => {
                            self.transcript = None;
                            self.transcript_error = Some(err);
                        }
                    }
                }
                WorkerPayload::Search {
                    query,
                    result: Ok(response),
                } => {
                    self.search_error = None;
                    self.selected_hit = 0;
                    self.evidence_scroll = 0;
                    self.search_applied_query = Some(query);
                    self.search_results = Some(response);
                }
                WorkerPayload::Search {
                    result: Err(err), ..
                } => {
                    self.search_results = None;
                    self.search_applied_query = None;
                    self.search_error = Some(err);
                }
            }
        }
    }

    fn apply_readiness(
        &mut self,
        status: Result<StoreStatus, String>,
        model: noesora_engine::ModelStatus,
    ) {
        match status {
            Ok(status) => {
                self.readiness = if !model.installed {
                    Readiness::ModelUnavailable {
                        pending: status.pending_embeddings,
                    }
                } else if status.pending_embeddings > 0 {
                    Readiness::IndexIncomplete {
                        chunks: status.chunks,
                        embedded: status.embedded_chunks,
                        pending: status.pending_embeddings,
                    }
                } else {
                    Readiness::Ready {
                        chunks: status.chunks,
                        embedded: status.embedded_chunks,
                    }
                };
                self.store_status = Some(status);
            }
            Err(err) => {
                self.readiness = Readiness::Error(err);
                self.store_status = None;
            }
        }
    }

    fn apply_sessions(
        &mut self,
        sessions: Result<Vec<SessionSummary>, String>,
        offset: usize,
        selected: usize,
    ) {
        match sessions {
            Ok(mut sessions) => {
                self.sessions_offset = offset;
                self.has_more_sessions = sessions.len() > SESSION_PAGE_SIZE;
                sessions.truncate(SESSION_PAGE_SIZE);
                self.sessions = sessions;
                self.selected_session = selected.min(self.sessions.len().saturating_sub(1));
                self.session_error = None;
                if self.transcript.as_ref().is_some_and(|page| {
                    self.sessions
                        .get(self.selected_session)
                        .is_none_or(|session| session.id != page.session.id)
                }) {
                    self.transcript = None;
                    self.transcript_viewport.get_mut().reset();
                    self.transcript_error = None;
                }
            }
            Err(err) => {
                self.sessions.clear();
                self.has_more_sessions = false;
                self.selected_session = 0;
                self.transcript_viewport.get_mut().reset();
                self.session_error = Some(err);
            }
        }
    }

    pub fn handle_key(&mut self, code: KeyCode) -> MemoryAction {
        let action = self.navigation.handle_key(code);
        match action {
            Action::None => {}
            Action::Quit => {
                self.invalidate();
                return MemoryAction::Close;
            }
            Action::SearchStarted => {
                self.invalidate();
                self.search_results = None;
                self.search_applied_query = None;
                self.search_error = None;
                self.evidence_scroll = 0;
            }
            Action::QueryChanged => {
                self.invalidate();
                self.search_results = None;
                self.search_applied_query = None;
                self.search_error = None;
            }
            Action::RunSearch => self.request_search(),
            Action::BackToSessions => {
                self.invalidate();
                self.search_results = None;
                self.search_applied_query = None;
                self.search_error = None;
                self.pending = Pending::None;
                if self.startup.needs_request() {
                    self.request_startup();
                }
            }
            Action::OpenSession => {
                if let Some(session) = self.sessions.get(self.selected_session) {
                    let id = session.id.clone();
                    self.expanded_tools.clear();
                    self.request_transcript(id, 0);
                }
            }
            Action::MoveSelection(delta) => self.move_selection(delta),
            Action::SelectMessage(delta) => self.select_message(delta),
            Action::PageSessions(direction) => self.page_sessions(direction),
            Action::PageTranscript(direction) => self.page_transcript(direction),
            Action::ScrollDetail(delta) => self.scroll_detail(delta),
            Action::ToggleTool => self.toggle_tool(),
            Action::ToggleSearchMode => {
                self.search_mode = match self.search_mode {
                    SearchMode::Text => SearchMode::Semantic,
                    SearchMode::Semantic => SearchMode::Text,
                };
                if self.navigation.query.trim().is_empty() {
                    self.invalidate();
                    self.search_results = None;
                    self.search_applied_query = None;
                    self.search_error = None;
                    self.navigation.focus = Focus::Input;
                } else {
                    self.request_search();
                }
            }
            Action::Refresh => self.request_startup(),
        }
        MemoryAction::None
    }

    fn cancel_startup(&mut self) {
        if self.startup == StartupState::Loading {
            self.startup.cancel();
            self.readiness = Readiness::NotLoaded;
        }
    }

    fn invalidate(&mut self) {
        self.cancel_startup();
        self.generation.next();
        self.pending = Pending::None;
        self.transcript_request = None;
        self.transcript_request_offset = None;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.navigation.screen == Screen::Sessions {
            if matches!(self.pending, Pending::Sessions) || self.sessions.is_empty() {
                return;
            }
            let next = self.selected_session as isize + delta;
            if next < 0 && self.sessions_offset > 0 {
                self.request_sessions(
                    self.sessions_offset.saturating_sub(SESSION_PAGE_SIZE),
                    SESSION_PAGE_SIZE - 1,
                );
            } else if next >= self.sessions.len() as isize && self.has_more_sessions {
                self.request_sessions(self.sessions_offset + SESSION_PAGE_SIZE, 0);
            } else {
                let previous = self.selected_session;
                self.selected_session = next.clamp(0, self.sessions.len() as isize - 1) as usize;
                if previous != self.selected_session {
                    let selected_id = self
                        .sessions
                        .get(self.selected_session)
                        .map(|session| session.id.as_str());
                    let transcript_id = self.transcript_request.as_deref().or_else(|| {
                        self.transcript
                            .as_ref()
                            .map(|page| page.session.id.as_str())
                    });
                    if transcript_id.is_some_and(|id| Some(id) != selected_id) {
                        self.invalidate();
                        self.transcript = None;
                        self.transcript_viewport.get_mut().reset();
                    }
                    self.transcript_error = None;
                }
            }
        } else if let Some(response) = &self.search_results {
            if !response.hits.is_empty() {
                self.selected_hit = (self.selected_hit as isize + delta)
                    .clamp(0, response.hits.len() as isize - 1)
                    as usize;
                self.evidence_scroll = 0;
            }
        }
    }

    fn page_sessions(&mut self, direction: isize) {
        if matches!(self.pending, Pending::Sessions) {
            return;
        }
        if direction > 0 {
            if self.has_more_sessions {
                self.request_sessions(self.sessions_offset + SESSION_PAGE_SIZE, 0);
            }
        } else if self.sessions_offset > 0 {
            self.request_sessions(
                self.sessions_offset.saturating_sub(SESSION_PAGE_SIZE),
                SESSION_PAGE_SIZE - 1,
            );
        }
    }

    fn page_transcript(&mut self, direction: isize) {
        let Some(page) = &self.transcript else { return };
        let Some(offset) = transcript_page_offset(page, direction) else {
            return;
        };
        let id = page.session.id.clone();
        self.request_transcript(id, offset);
    }

    fn scroll_detail(&mut self, delta: isize) {
        if self.navigation.screen == Screen::Sessions {
            let amount = delta.unsigned_abs();
            self.transcript_scroll = if delta < 0 {
                self.transcript_scroll.saturating_sub(amount)
            } else {
                self.transcript_scroll.saturating_add(amount)
            };
            let width = self.transcript_viewport.borrow().width;
            if let (Some(page), Some(width)) = (self.transcript.as_ref(), width) {
                self.transcript_viewport.borrow_mut().position_at(
                    page,
                    &self.expanded_tools,
                    self.selected_message,
                    width,
                    self.transcript_scroll,
                );
            }
        } else {
            let amount = delta.unsigned_abs();
            self.evidence_scroll = if delta < 0 {
                self.evidence_scroll.saturating_sub(amount)
            } else {
                self.evidence_scroll.saturating_add(amount)
            };
        }
    }

    fn select_message(&mut self, delta: isize) {
        let Some(page) = &self.transcript else { return };
        if page.messages.is_empty() {
            return;
        }
        let last = page.messages.len() as isize - 1;
        self.selected_message = (self.selected_message as isize + delta).clamp(0, last) as usize;
    }

    fn toggle_tool(&mut self) {
        let Some(message) = self
            .transcript
            .as_ref()
            .and_then(|page| page.messages.get(self.selected_message))
        else {
            return;
        };
        if !is_tool_message(message) {
            return;
        }
        if !self.expanded_tools.insert(message.id.clone()) {
            self.expanded_tools.remove(&message.id);
        }
        self.transcript_viewport.get_mut().reset();
    }
}
fn transcript_page_offset(page: &TranscriptPage, direction: isize) -> Option<usize> {
    if direction > 0 {
        let next = page.offset.saturating_add(page.messages.len());
        (next < page.total).then_some(next)
    } else if page.offset > 0 {
        Some(page.offset.saturating_sub(TRANSCRIPT_PAGE_SIZE))
    } else {
        None
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.generation.next();
    }
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let header_height = if app.navigation.screen == Screen::Search {
        4
    } else {
        3
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
    draw_header(frame, app, chunks[0]);
    draw_footer(frame, app, chunks[2]);

    if area.width < NARROW_WIDTH {
        draw_narrow(frame, app, chunks[1]);
    } else {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(chunks[1]);
        match app.navigation.screen {
            Screen::Sessions => {
                draw_session_list(frame, app, panes[0]);
                draw_transcript(frame, app, panes[1]);
            }
            Screen::Search => {
                draw_search_list(frame, app, panes[0]);
                draw_search_evidence(frame, app, panes[1]);
            }
        }
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let root = app
        .store_status
        .as_ref()
        .map(|status| status.root.display().to_string())
        .unwrap_or_else(|| app.store.root().display().to_string());
    let sessions = app
        .store_status
        .as_ref()
        .map(|status| status.sessions.to_string())
        .unwrap_or_else(|| "…".to_owned());
    let metadata = format!("All projects · {sessions} sessions · {root}");
    let readiness = readiness_text(&app.readiness);
    let title_style = Style::default()
        .fg(INK)
        .bg(RAISED)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::styled(
            bounded_header_text("NOESORA  /  LOCAL SESSION WORKSPACE", area.width),
            title_style,
        ),
        Line::from(bounded_header_text(&metadata, area.width)),
        Line::from(bounded_header_text(&readiness, area.width)),
    ];
    let search_label;
    if app.navigation.screen == Screen::Search {
        let focus = if app.navigation.focus == Focus::Input {
            "●"
        } else {
            " "
        };
        search_label = format!("{focus} Search [{}]  ", mode_name(app.search_mode));
        let search_label = bounded_header_text(&search_label, area.width);
        let label_width = Span::raw(search_label.as_ref())
            .width()
            .min(usize::from(area.width)) as u16;
        let query_width = area.width.saturating_sub(label_width);
        let query = bounded_header_text(&app.navigation.query, query_width);
        lines.push(Line::from(vec![
            Span::styled(
                search_label,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(query),
        ]));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        area,
    );
}

fn bounded_header_text(text: &str, max_width: u16) -> Cow<'_, str> {
    let text = single_line_header_text(text);
    let span = Span::raw(text.as_ref());
    let max_width = usize::from(max_width);
    if span.width() <= max_width {
        return text;
    }

    let ellipsis = "…";
    let Some(prefix_width) = max_width.checked_sub(Span::raw(ellipsis).width()) else {
        return Cow::Owned(String::new());
    };
    let mut visible_width = 0usize;
    let mut visible_bytes = 0usize;
    for grapheme in span.styled_graphemes(Style::default()) {
        let width = Span::raw(grapheme.symbol).width();
        if visible_width.saturating_add(width) > prefix_width {
            break;
        }
        visible_width += width;
        visible_bytes += grapheme.symbol.len();
    }

    let mut bounded = String::with_capacity(visible_bytes + ellipsis.len());
    bounded.push_str(&text[..visible_bytes]);
    bounded.push_str(ellipsis);
    Cow::Owned(bounded)
}

fn single_line_header_text(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|ch| ch.is_control()) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|ch| match ch {
                '\n' | '\t' => ' ',
                ch if ch.is_control() => '�',
                ch => ch,
            })
            .collect(),
    )
}

fn readiness_text(readiness: &Readiness) -> String {
    match readiness {
        Readiness::Loading => "Store status: loading".to_owned(),
        Readiness::NotLoaded => "Store status not loaded".to_owned(),
        Readiness::Ready { chunks, embedded } => {
            format!("Ready · {embedded}/{chunks} text passages embedded")
        }
        Readiness::IndexIncomplete { chunks, embedded, pending } => format!(
            "Index incomplete · {embedded}/{chunks} passages embedded · {pending} pending"
        ),
        Readiness::ModelUnavailable { pending } => format!(
            "Model unavailable · {pending} passages pending · text search and browsing remain available"
        ),
        Readiness::Error(err) => format!("Store status error: {err}"),
    }
}

fn mode_name(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Text => "text",
        SearchMode::Semantic => "semantic",
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let focus = match (app.navigation.screen, app.navigation.focus) {
        (Screen::Sessions, Focus::List) => "focus: sessions",
        (Screen::Sessions, Focus::Detail) => "focus: transcript",
        (_, Focus::Input) => "focus: query",
        (Screen::Search, Focus::List) => "focus: results",
        (Screen::Search, Focus::Detail) => "focus: evidence",
    };
    let busy = match app.pending {
        Pending::None => "",
        Pending::Startup => " · loading archive",
        Pending::Sessions => " · loading sessions",
        Pending::Transcript => " · loading transcript",
        Pending::Search => " · searching locally",
    };
    let hint = match app.navigation.screen {
        Screen::Sessions if app.navigation.focus == Focus::Detail => {
            "Tab list/transcript · ↑/↓ scroll · ←/→ message · x expand tool · PgUp/PgDn transcript page · Esc back · q quit"
        }
        Screen::Sessions => "↑/↓ select · Enter open · Tab focus · / search · PgUp/PgDn sessions · r refresh · q quit",
        Screen::Search if app.navigation.focus == Focus::Input => {
            "Type query (q is text) · Enter search · Tab results (m mode) · Esc back"
        }
        Screen::Search if app.navigation.focus == Focus::List => {
            "↑/↓ select · Enter evidence · Tab focus · / edit query · m text/semantic · Esc back"
        }
        Screen::Search => "↑/↓ scroll evidence · Tab query/results/evidence · / edit query · Esc back",
    };
    let text = format!(
        "{focus}{busy}
{hint}"
    );
    frame.render_widget(
        Paragraph::new(clean(&text)).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_narrow(frame: &mut Frame, app: &App, area: Rect) {
    match (app.navigation.screen, app.navigation.focus) {
        (Screen::Sessions, Focus::List) => draw_session_list(frame, app, area),
        (Screen::Sessions, Focus::Detail) => draw_transcript(frame, app, area),
        (Screen::Search, Focus::List) => draw_search_list(frame, app, area),
        (Screen::Search, Focus::Detail) => draw_search_evidence(frame, app, area),
        (Screen::Search, Focus::Input) => {
            let block = focused_block("Query input", true);
            frame.render_widget(
                Paragraph::new(
                    "Type your query above and press Enter to search the global archive.",
                )
                .block(block)
                .wrap(Wrap { trim: false }),
                area,
            );
        }
        _ => {}
    }
}

fn focused_block(title: &str, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(LINE)
    };
    Block::default()
        .title(focused_title(title, focused))
        .borders(Borders::ALL)
        .border_style(style)
}

fn focused_title(title: &str, focused: bool) -> String {
    format!("{} {title}", if focused { "▶" } else { " " })
}

fn draw_session_list(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!(
        "Raw sessions · all projects · offset {}",
        app.sessions_offset
    );
    let block = focused_block(&title, app.navigation.focus == Focus::List);
    if let Some(err) = &app.session_error {
        frame.render_widget(
            Paragraph::new(clean(err))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.sessions.is_empty() && matches!(app.pending, Pending::Startup | Pending::Sessions) {
        frame.render_widget(Paragraph::new("Loading sessions…").block(block), area);
        return;
    }
    if app.sessions.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "No imported sessions. Import a JSONL session or add and sync a source root.",
            )
            .block(block)
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let items = app.sessions.iter().map(session_item).collect::<Vec<_>>();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(ACCENT_SOFT)
            .fg(INK)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(app.selected_session.min(app.sessions.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn session_item(session: &SessionSummary) -> ListItem<'static> {
    let title = clean(&session.title);
    let project = clean(&session.project);
    let source_namespace = clean(&session.source_namespace);
    let external_id = clean(&session.external_id);
    let client = native_client_label(session.client.as_ref());
    let created = clean(source_time(session.created_at.as_deref()));
    let updated = clean(source_time(session.updated_at.as_deref()));
    ListItem::new(Text::from(vec![
        Line::styled(
            title,
            Style::default()
                .fg(INK)
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(format!(
            "Project {project} · source namespace {source_namespace} · native client {client}"
        )),
        Line::from(format!(
            "External session ID {external_id} · {} messages · created {created} · updated {updated}",
            session.messages
        )),
    ]))
}

fn native_client_label(client: Option<&noesora_engine::Client>) -> &'static str {
    match client {
        Some(noesora_engine::Client::Omp) => "omp",
        Some(noesora_engine::Client::ClaudeCode) => "claude-code",
        Some(noesora_engine::Client::Codex) => "codex",
        Some(noesora_engine::Client::Grok) => "grok",
        Some(noesora_engine::Client::OpenCode) => "opencode",
        Some(noesora_engine::Client::Cursor) => "cursor-agent",
        None => "not applicable",
    }
}

fn source_time(timestamp: Option<&str>) -> &str {
    timestamp.unwrap_or("unknown")
}

fn transcript_page_range(page: &TranscriptPage) -> Option<(usize, usize)> {
    (!page.messages.is_empty()).then(|| {
        (
            page.offset.saturating_add(1),
            page.offset.saturating_add(page.messages.len()),
        )
    })
}

fn draw_transcript(frame: &mut Frame, app: &App, area: Rect) {
    let (title, identity, page) = if let Some(page) = &app.transcript {
        let range = transcript_page_range(page)
            .map(|(first, last)| format!("messages {first}–{last} of {}", page.total))
            .unwrap_or_else(|| format!("messages 0 of {}", page.total));
        let identity = format!(
            "{} · project {}\nSource namespace {} · external ID {} · native client {}\nSource created {} · updated {}\nSnapshot {}",
            clean(&page.session.id),
            clean(&page.session.project),
            clean(&page.session.source_namespace),
            clean(&page.session.external_id),
            native_client_label(page.session.client.as_ref()),
            clean(source_time(page.session.created_at.as_deref())),
            clean(source_time(page.session.updated_at.as_deref())),
            clean(&page.session.snapshot),
        );
        (format!("Raw transcript · {range}"), identity, Some(page))
    } else {
        (
            "Raw transcript".to_owned(),
            app.transcript_error
                .as_ref()
                .map(|err| clean(err))
                .unwrap_or_else(|| {
                    "Select a session and press Enter to open its raw transcript.".to_owned()
                }),
            None,
        )
    };
    let focused = app.navigation.focus == Focus::Detail;
    let block = focused_block(&title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(page) = page {
        let meta_height = inner.height.min(4);
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(meta_height), Constraint::Min(0)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(clean(&identity)).wrap(Wrap { trim: false }),
            split[0],
        );
        let mut viewport = app.transcript_viewport.borrow_mut();
        let position = viewport.position_at(
            page,
            &app.expanded_tools,
            app.selected_message,
            split[1].width,
            app.transcript_scroll,
        );
        let lines = transcript_window_lines(
            page,
            &app.expanded_tools,
            app.selected_message,
            position,
            split[1].width,
            split[1].height,
        );
        frame.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            split[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(clean(&identity)).wrap(Wrap { trim: false }),
            inner,
        );
    }
}

#[cfg(test)]
fn transcript_lines<'a>(
    page: &'a TranscriptPage,
    expanded_tools: &HashSet<String>,
    selected_message: usize,
) -> Vec<Line<'a>> {
    let mut output = Vec::with_capacity(page.messages.len().saturating_mul(2));
    for (index, message) in page.messages.iter().enumerate() {
        let selected = index == selected_message;
        let role_style = role_style(&message.role);
        let stamp = message.timestamp.as_deref().unwrap_or("-");
        let same_kind = message.kind.eq_ignore_ascii_case(&message.role);
        let mut header = vec![Span::styled(
            clean_view(&message.role),
            role_style.add_modifier(Modifier::BOLD),
        )];
        if !same_kind {
            header.push(Span::raw("  "));
            header.push(Span::styled(
                clean_view(&message.kind),
                Style::default().fg(MUTED),
            ));
        }
        header.push(Span::raw("  "));
        header.push(Span::raw(clean_view(stamp)));
        let mut header = Line::from(header);
        if selected {
            header = header.style(Style::default().bg(ACCENT_SOFT));
        }
        output.push(header);

        if is_tool_message(message) && !expanded_tools.contains(&message.id) {
            let mut collapsed = Line::styled(
                format!(
                    "Tool body collapsed · {} bytes · select message and press x to expand",
                    message.text.len()
                ),
                Style::default().fg(SILVER),
            );
            if selected {
                collapsed = collapsed.style(Style::default().bg(ACCENT_SOFT));
            }
            output.push(collapsed);
            continue;
        }

        if message.text.is_empty() {
            let mut empty = Line::from(Span::styled("(empty message)", Style::default().fg(MUTED)));
            if selected {
                empty = empty.style(Style::default().bg(ACCENT_SOFT));
            }
            output.push(empty);
            continue;
        }

        for text_line in message.text.lines() {
            let mut body = Line::styled(clean_view(text_line), role_style);
            if selected {
                body = body.style(Style::default().bg(ACCENT_SOFT));
            }
            output.push(body);
        }
    }
    output
}

fn transcript_window_lines<'a>(
    page: &'a TranscriptPage,
    expanded_tools: &HashSet<String>,
    selected_message: usize,
    mut position: Option<TranscriptPosition>,
    width: u16,
    height: u16,
) -> Vec<Line<'a>> {
    let mut output = Vec::with_capacity(height as usize);
    for _ in 0..height {
        let Some(current) = position else { break };
        let Some(display) =
            transcript_display_line(page, expanded_tools, selected_message, current, width)
        else {
            break;
        };
        output.push(display.line);
        position = display.next;
    }
    output
}

fn transcript_display_line<'a>(
    page: &'a TranscriptPage,
    expanded_tools: &HashSet<String>,
    selected_message: usize,
    current: TranscriptPosition,
    width: u16,
) -> Option<TranscriptDisplayLine<'a>> {
    let line = transcript_logical_line(page, expanded_tools, selected_message, current)?;
    let row = wrapped_row(&line, current.wrap, width)?;
    let next = row
        .next
        .map(|wrap| TranscriptPosition { wrap, ..current })
        .or_else(|| next_logical_position(page, expanded_tools, current));
    Some(TranscriptDisplayLine {
        line: transcript_line_slice(&line, current.wrap.row_start, row.end),
        next,
    })
}

impl TranscriptViewport {
    fn position_at(
        &mut self,
        page: &TranscriptPage,
        expanded_tools: &HashSet<String>,
        selected_message: usize,
        width: u16,
        target: usize,
    ) -> Option<TranscriptPosition> {
        if self.width != Some(width) {
            self.reset();
            self.width = Some(width);
            self.cursor = transcript_first_position(page);
            let start = TranscriptCheckpoint {
                row: 0,
                position: self.cursor,
            };
            self.checkpoints.push(start);
            self.history.push_back(start);
        }

        if target < self.row {
            if let Some(index) = self
                .history
                .iter()
                .position(|checkpoint| checkpoint.row == target)
            {
                self.row = self.history[index].row;
                self.cursor = self.history[index].position;
                self.history.truncate(index + 1);
                return self.cursor;
            }
            let checkpoint = self
                .checkpoints
                .iter()
                .rposition(|checkpoint| checkpoint.row <= target)
                .unwrap_or(0);
            let start = self.checkpoints[checkpoint];
            self.row = start.row;
            self.cursor = start.position;
            self.history.clear();
            self.history.push_back(start);
        } else if let Some(checkpoint) = self
            .checkpoints
            .iter()
            .rposition(|checkpoint| checkpoint.row <= target && checkpoint.row > self.row)
        {
            let start = self.checkpoints[checkpoint];
            self.row = start.row;
            self.cursor = start.position;
            self.history.clear();
            self.history.push_back(start);
        }

        while self.row < target {
            let Some(current) = self.cursor else { break };
            self.cursor =
                next_transcript_position(page, expanded_tools, selected_message, current, width);
            self.row += 1;
            self.history.push_back(TranscriptCheckpoint {
                row: self.row,
                position: self.cursor,
            });
            if self.history.len() > TRANSCRIPT_HISTORY_ROWS {
                self.history.pop_front();
            }
            if self.row % TRANSCRIPT_CHECKPOINT_INTERVAL == 0 {
                self.remember_checkpoint(TranscriptCheckpoint {
                    row: self.row,
                    position: self.cursor,
                });
            }
        }
        self.cursor
    }

    fn remember_checkpoint(&mut self, checkpoint: TranscriptCheckpoint) {
        match self
            .checkpoints
            .binary_search_by_key(&checkpoint.row, |entry| entry.row)
        {
            Ok(index) => self.checkpoints[index] = checkpoint,
            Err(index) => self.checkpoints.insert(index, checkpoint),
        }
    }
}

fn transcript_first_position(page: &TranscriptPage) -> Option<TranscriptPosition> {
    (!page.messages.is_empty()).then(|| transcript_position(0, TranscriptSection::Header))
}

fn transcript_position(message: usize, section: TranscriptSection) -> TranscriptPosition {
    TranscriptPosition {
        message,
        section,
        wrap: WrapCursor::new(LineOffset::default()),
    }
}

fn next_transcript_position(
    page: &TranscriptPage,
    expanded_tools: &HashSet<String>,
    selected_message: usize,
    current: TranscriptPosition,
    width: u16,
) -> Option<TranscriptPosition> {
    let line = transcript_logical_line(page, expanded_tools, selected_message, current)?;
    let row = wrapped_row(&line, current.wrap, width)?;
    row.next
        .map(|wrap| TranscriptPosition { wrap, ..current })
        .or_else(|| next_logical_position(page, expanded_tools, current))
}

fn next_logical_position(
    page: &TranscriptPage,
    expanded_tools: &HashSet<String>,
    current: TranscriptPosition,
) -> Option<TranscriptPosition> {
    let message = page.messages.get(current.message)?;
    match current.section {
        TranscriptSection::Header => {
            if is_tool_message(message) && !expanded_tools.contains(&message.id) {
                Some(transcript_position(
                    current.message,
                    TranscriptSection::CollapsedTool,
                ))
            } else if message.text.is_empty() {
                Some(transcript_position(
                    current.message,
                    TranscriptSection::EmptyBody,
                ))
            } else {
                let line_end = transcript_body_end(&message.text, 0);
                Some(transcript_position(
                    current.message,
                    TranscriptSection::Body {
                        line_start: 0,
                        line_end,
                    },
                ))
            }
        }
        TranscriptSection::CollapsedTool | TranscriptSection::EmptyBody => (current.message + 1
            < page.messages.len())
        .then(|| transcript_position(current.message + 1, TranscriptSection::Header)),
        TranscriptSection::Body {
            line_start: _,
            line_end,
        } => {
            if line_end < message.text.len() {
                let line_start = line_end + 1;
                if line_start < message.text.len() {
                    let line_end = transcript_body_end(&message.text, line_start);
                    Some(transcript_position(
                        current.message,
                        TranscriptSection::Body {
                            line_start,
                            line_end,
                        },
                    ))
                } else {
                    (current.message + 1 < page.messages.len()).then(|| {
                        transcript_position(current.message + 1, TranscriptSection::Header)
                    })
                }
            } else {
                (current.message + 1 < page.messages.len())
                    .then(|| transcript_position(current.message + 1, TranscriptSection::Header))
            }
        }
    }
}

fn transcript_body_end(text: &str, line_start: usize) -> usize {
    text[line_start..]
        .find('\n')
        .map(|newline| line_start + newline)
        .unwrap_or(text.len())
}

fn transcript_body_text(text: &str, line_start: usize, line_end: usize) -> &str {
    let mut end = line_end;
    if line_end < text.len() && end > line_start && text.as_bytes()[end - 1] == b'\r' {
        end -= 1;
    }
    &text[line_start..end]
}

fn transcript_logical_line<'a>(
    page: &'a TranscriptPage,
    expanded_tools: &HashSet<String>,
    selected_message: usize,
    position: TranscriptPosition,
) -> Option<TranscriptLineSource<'a>> {
    let message = page.messages.get(position.message)?;
    let selected = position.message == selected_message;
    let mut line = match position.section {
        TranscriptSection::Header => {
            let role_style = role_style(&message.role);
            let stamp = message.timestamp.as_deref().unwrap_or("-");
            let same_kind = message.kind.eq_ignore_ascii_case(&message.role);
            let mut line = TranscriptLineSource::new(Style::default());
            line.push(Span::styled(
                message.role.as_str(),
                role_style.add_modifier(Modifier::BOLD),
            ));
            if !same_kind {
                line.push(Span::raw("  "));
                line.push(Span::styled(
                    message.kind.as_str(),
                    Style::default().fg(MUTED),
                ));
            }
            line.push(Span::raw("  "));
            line.push(Span::raw(stamp));
            line
        }
        TranscriptSection::CollapsedTool => {
            if !is_tool_message(message) || expanded_tools.contains(&message.id) {
                return None;
            }
            let mut line = TranscriptLineSource::new(Style::default().fg(SILVER));
            line.push(Span::raw(format!(
                "Tool body collapsed · {} bytes · select message and press x to expand",
                message.text.len()
            )));
            line
        }
        TranscriptSection::EmptyBody => {
            if !message.text.is_empty() {
                return None;
            }
            let mut line = TranscriptLineSource::new(Style::default());
            line.push(Span::styled("(empty message)", Style::default().fg(MUTED)));
            line
        }
        TranscriptSection::Body {
            line_start,
            line_end,
        } => {
            if message.text.is_empty()
                || (is_tool_message(message) && !expanded_tools.contains(&message.id))
            {
                return None;
            }
            let text = transcript_body_text(&message.text, line_start, line_end);
            let mut line = TranscriptLineSource::new(role_style(&message.role));
            line.push(Span::raw(text));
            line
        }
    };
    if selected {
        line.style = Style::default().bg(ACCENT_SOFT);
    }
    Some(line)
}

fn transcript_line_slice<'a>(
    line: &TranscriptLineSource<'a>,
    start: LineOffset,
    end: LineOffset,
) -> Line<'a> {
    let mut spans = Vec::with_capacity(line.len);
    for index in start.span..line.len {
        if index > end.span {
            break;
        }
        let Some(span) = line.span(index) else { break };
        let from = if index == start.span { start.byte } else { 0 };
        let to = if index == end.span {
            end.byte
        } else {
            span.content.len()
        };
        if from >= to {
            continue;
        }
        let content = match &span.content {
            Cow::Borrowed(content) => clean_view(&content[from..to]),
            Cow::Owned(content) => Cow::Owned(clean_view(&content[from..to]).into_owned()),
        };
        spans.push(Span {
            content,
            style: span.style,
        });
    }
    Line {
        style: line.style,
        alignment: None,
        spans,
    }
}

fn line_end_offset(line: &TranscriptLineSource<'_>) -> LineOffset {
    LineOffset {
        span: line.len,
        byte: 0,
    }
}

fn source_grapheme_at(
    line: &TranscriptLineSource<'_>,
    mut offset: LineOffset,
) -> Option<SourceGrapheme> {
    while offset.span < line.len {
        let span = line.span(offset.span)?;
        let content = span.content.as_ref();
        if offset.byte >= content.len() {
            offset = LineOffset {
                span: offset.span + 1,
                byte: 0,
            };
            continue;
        }

        let suffix = &content[offset.byte..];
        let first = suffix.chars().next()?;
        let start = offset;
        if first.is_control() {
            let end = LineOffset {
                span: offset.span,
                byte: offset.byte + first.len_utf8(),
            };
            if matches!(first, '\n' | '\t') {
                return Some(SourceGrapheme {
                    start,
                    end,
                    width: 0,
                    whitespace: false,
                    ignored: true,
                });
            }
            return Some(SourceGrapheme {
                start,
                end,
                width: wrapped_symbol("�").0,
                whitespace: false,
                ignored: false,
            });
        }

        let source_span = Span::styled(suffix, span.style);
        let grapheme = source_span.styled_graphemes(Style::default()).next()?;
        let end = LineOffset {
            span: offset.span,
            byte: offset.byte + grapheme.symbol.len(),
        };
        let (width, whitespace) = wrapped_symbol(grapheme.symbol);
        return Some(SourceGrapheme {
            start,
            end,
            width,
            whitespace,
            ignored: false,
        });
    }
    None
}

fn wrapped_symbol(symbol: &str) -> (u16, bool) {
    let cleaned = clean_view(symbol);
    let span = Span::raw(cleaned.as_ref());
    let width = span.width().min(u16::MAX as usize) as u16;
    let mut graphemes = span.styled_graphemes(Style::default());
    let mut any = false;
    let mut whitespace = true;
    for grapheme in &mut graphemes {
        any = true;
        whitespace &= grapheme.is_whitespace();
    }
    (width, any && whitespace)
}

fn wrapped_width_sum(line: u16, whitespace: u16, word: u16) -> u16 {
    line.saturating_add(whitespace).saturating_add(word)
}

fn commit_wrap_buffers(cursor: &mut WrapCursor) {
    if cursor.whitespace {
        cursor.line_width = cursor.line_width.saturating_add(cursor.whitespace_width);
        cursor.pending_line_end = cursor.whitespace_end;
        cursor.pending_line = true;
    }
    if cursor.word {
        cursor.line_width = cursor.line_width.saturating_add(cursor.word_width);
        cursor.pending_line_end = cursor.word_end;
        cursor.pending_line = true;
    }
    cursor.whitespace_width = 0;
    cursor.whitespace = false;
    cursor.word_width = 0;
    cursor.word = false;
}

fn append_wrap_token(
    cursor: &mut WrapCursor,
    start: LineOffset,
    end: LineOffset,
    width: u16,
    whitespace: bool,
) {
    if whitespace {
        if !cursor.whitespace {
            cursor.whitespace_start = start;
        }
        cursor.whitespace_end = end;
        cursor.whitespace_width = cursor.whitespace_width.saturating_add(width);
        cursor.whitespace = true;
    } else {
        if !cursor.word {
            cursor.word_start = start;
        }
        cursor.word_end = end;
        cursor.word_width = cursor.word_width.saturating_add(width);
        cursor.word = true;
    }
    cursor.non_whitespace_previous = !whitespace;
    cursor.input = end;
}

fn trim_wrap_whitespace(
    line: &TranscriptLineSource<'_>,
    cursor: &mut WrapCursor,
    mut remaining: u16,
) {
    let mut consumed_width = 0u16;
    while cursor.whitespace {
        let Some(token) = source_grapheme_at(line, cursor.whitespace_start) else {
            break;
        };
        if token.ignored {
            cursor.whitespace_start = token.end;
            continue;
        }
        if token.end > cursor.whitespace_end || token.width > remaining {
            break;
        }
        remaining -= token.width;
        consumed_width = consumed_width.saturating_add(token.width);
        cursor.whitespace_start = token.end;
        if cursor.whitespace_start >= cursor.whitespace_end {
            cursor.whitespace = false;
            cursor.whitespace_width = 0;
            break;
        }
    }
    if cursor.whitespace {
        cursor.whitespace_width = cursor.whitespace_width.saturating_sub(consumed_width);
    }
}

fn next_emittable_position(
    line: &TranscriptLineSource<'_>,
    mut offset: LineOffset,
    max_width: u16,
) -> Option<LineOffset> {
    while let Some(token) = source_grapheme_at(line, offset) {
        if token.ignored || token.width > max_width {
            offset = token.end;
        } else {
            return Some(token.start);
        }
    }
    None
}

// Keep Ratatui's update order: decide overflow before queuing the current grapheme.
fn wrapped_row(
    line: &TranscriptLineSource<'_>,
    initial: WrapCursor,
    max_width: u16,
) -> Option<WrappedRow> {
    if max_width == 0 {
        return None;
    }
    let line_end = line_end_offset(line);
    let mut cursor = initial;
    while let Some(token) = source_grapheme_at(line, cursor.input) {
        let start = token.start;
        let end = token.end;
        let symbol_width = token.width;
        let is_whitespace = token.whitespace;
        if token.ignored || symbol_width > max_width {
            cursor.input = end;
            continue;
        }

        let word_found = cursor.non_whitespace_previous && is_whitespace;
        let untrimmed_overflow = !cursor.pending_line
            && wrapped_width_sum(cursor.word_width, cursor.whitespace_width, symbol_width)
                > max_width;
        if word_found || untrimmed_overflow {
            commit_wrap_buffers(&mut cursor);
        }

        let line_full = cursor.line_width >= max_width;
        let pending_word_overflow = symbol_width > 0
            && wrapped_width_sum(
                cursor.line_width,
                cursor.whitespace_width,
                cursor.word_width,
            ) >= max_width;
        if line_full || pending_word_overflow {
            let row_end = if cursor.pending_line {
                cursor.pending_line_end
            } else {
                cursor.row_start
            };
            let remaining = max_width.saturating_sub(cursor.line_width);
            trim_wrap_whitespace(line, &mut cursor, remaining);
            cursor.line_width = 0;
            cursor.pending_line = false;
            cursor.emitted_row = true;

            let skip_current = is_whitespace && !cursor.whitespace;
            if skip_current {
                cursor.input = end;
            } else {
                append_wrap_token(&mut cursor, start, end, symbol_width, is_whitespace);
            }
            let next_start = if cursor.whitespace {
                cursor.whitespace_start
            } else if cursor.word {
                cursor.word_start
            } else if skip_current {
                if let Some(next) = next_emittable_position(line, cursor.input, max_width) {
                    cursor.input = next;
                    next
                } else {
                    line_end
                }
            } else {
                cursor.input
            };
            cursor.row_start = next_start;
            let has_next = cursor.whitespace || cursor.word || cursor.input < line_end;
            return Some(WrappedRow {
                end: row_end,
                next: has_next.then_some(cursor),
            });
        }
        append_wrap_token(&mut cursor, start, end, symbol_width, is_whitespace);
    }

    commit_wrap_buffers(&mut cursor);
    if cursor.pending_line {
        Some(WrappedRow {
            end: cursor.pending_line_end,
            next: None,
        })
    } else if !cursor.emitted_row {
        Some(WrappedRow {
            end: line_end,
            next: None,
        })
    } else {
        None
    }
}

fn is_tool_message(message: &noesora_engine::SessionMessage) -> bool {
    let role = message.role.to_ascii_lowercase();
    let kind = message.kind.to_ascii_lowercase();
    role == "tool"
        || role == "toolresult"
        || role == "tool_result"
        || kind.contains("tool")
        || kind.contains("function")
}

fn role_style(role: &str) -> Style {
    if role.eq_ignore_ascii_case("user") {
        Style::default().fg(ACCENT)
    } else if role.eq_ignore_ascii_case("assistant") {
        Style::default().fg(INK)
    } else if role.eq_ignore_ascii_case("tool")
        || role.eq_ignore_ascii_case("toolresult")
        || role.eq_ignore_ascii_case("tool_result")
    {
        Style::default().fg(SILVER)
    } else {
        Style::default().fg(MUTED)
    }
}

fn search_error_text(error: &SearchFailure) -> String {
    let message = match error {
        SearchFailure::Engine(error) => format!("Search error: {error}"),
        SearchFailure::WorkerStart(message) => format!("Search error: {message}"),
    };
    clean(&message)
}

fn draw_search_list(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!("Cited results · {} search", mode_name(app.search_mode));
    let block = focused_block(&title, app.navigation.focus == Focus::List);
    if let Some(err) = &app.search_error {
        frame.render_widget(
            Paragraph::new(search_error_text(err))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if matches!(app.pending, Pending::Search) {
        frame.render_widget(
            Paragraph::new("Searching local evidence…").block(block),
            area,
        );
        return;
    }
    let Some(response) = &app.search_results else {
        frame.render_widget(
            Paragraph::new(
                "Type a query above and press Enter. Search uses launch project, client, and source namespace filters.",
            )
            .block(block)
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    };
    if response.refused || response.hits.is_empty() {
        let message = if response.refused {
            format!(
                "No evidence · refused ({})",
                clean(response.reason.as_deref().unwrap_or("no_evidence"))
            )
        } else {
            "No evidence matches this query.".to_owned()
        };
        frame.render_widget(
            Paragraph::new(message)
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let items = response
        .hits
        .iter()
        .map(|hit| {
            let source_namespace = hit
                .source_namespace
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "not applicable".to_owned());
            let external_id = hit
                .external_id
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "not applicable".to_owned());
            ListItem::new(Text::from(vec![
                Line::styled(
                    clean(&hit.title),
                    Style::default().fg(INK).add_modifier(Modifier::BOLD),
                ),
                Line::from(format!(
                    "{} · source namespace {} · external ID {} · native client {} · {}",
                    clean(&hit.project),
                    source_namespace,
                    external_id,
                    native_client_label(hit.client.as_ref()),
                    clean(&hit.kind)
                )),
            ]))
        })
        .collect::<Vec<_>>();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(ACCENT_SOFT)
            .fg(INK)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(app.selected_hit.min(response.hits.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_search_evidence(frame: &mut Frame, app: &App, area: Rect) {
    let title = "Source evidence".to_owned();
    let block = focused_block(&title, app.navigation.focus == Focus::Detail);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(err) = &app.search_error {
        frame.render_widget(
            Paragraph::new(search_error_text(err)).wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    let Some(response) = &app.search_results else {
        let message = match app.pending {
            Pending::Search => "Waiting for the local search worker…".to_owned(),
            _ => "Select a cited result to inspect its verified source evidence.".to_owned(),
        };
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: false }), inner);
        return;
    };
    if response.refused || response.hits.is_empty() {
        let reason = clean(response.reason.as_deref().unwrap_or("no_evidence"));
        frame.render_widget(
            Paragraph::new(format!(
                "No evidence found. Search refused: {reason}.\nIndex coverage: {}/{} passages · {}.",
                response.embedded_chunks,
                response.total_chunks,
                if response.index_complete {
                    "complete"
                } else {
                    "incomplete"
                },
            ))
            .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    let Some(hit) = response.hits.get(app.selected_hit) else {
        frame.render_widget(
            Paragraph::new("No evidence selected.").wrap(Wrap { trim: false }),
            inner,
        );
        return;
    };
    let origin = if hit.session_id.is_some() {
        format!(
            "Source namespace: {}\nNative client: {}\nExternal session ID: {}\nSession ID: {}\nMessage ID: {}",
            clean(hit.source_namespace.as_deref().unwrap_or("not applicable")),
            native_client_label(hit.client.as_ref()),
            clean(hit.external_id.as_deref().unwrap_or("not applicable")),
            clean(hit.session_id.as_deref().unwrap_or("not applicable")),
            clean(hit.message_id.as_deref().unwrap_or("not applicable")),
        )
    } else {
        "Source: approved record".to_owned()
    };
    let detail = format!(
        "{}\nSearch hit ID: {}\nType: {} · role {}\nProject: {}\n{}\nPath: {}\nSource bytes: {}..{}\nSHA-256: {}\nJSON pointer: {}\nDecoded-text span: {}\n\n{}\n\nIndex coverage: {}/{} passages · {}{}",
        clean(&hit.title),
        clean(&hit.id),
        clean(&hit.kind),
        clean(&hit.role),
        clean(&hit.project),
        origin,
        clean(&hit.path),
        hit.span[0],
        hit.span[1],
        clean(&hit.hash),
        clean(hit.field.as_deref().unwrap_or("-")),
        hit.text_span
            .map(|span| format!("{}..{}", span[0], span[1]))
            .unwrap_or_else(|| "-".to_owned()),
        clean(&hit.text),
        response.embedded_chunks,
        response.total_chunks,
        if response.index_complete {
            "complete"
        } else {
            "incomplete"
        },
        if response.truncated {
            " · more ranked hits exist"
        } else {
            ""
        },
    );
    frame.render_widget(
        Paragraph::new(clean(&detail))
            .scroll((app.evidence_scroll.min(u16::MAX as usize) as u16, 0))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn clean_view(text: &str) -> Cow<'_, str> {
    if !text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
    {
        return Cow::Borrowed(text);
    }
    Cow::Owned(sanitize_terminal_text(text))
}

fn clean(text: &str) -> String {
    clean_view(text).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;
    static NEXT_TEST_HOME: AtomicU64 = AtomicU64::new(0);

    struct TestHome(std::path::PathBuf);

    impl TestHome {
        fn new() -> Self {
            let id = NEXT_TEST_HOME.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!("noesora-tui-test-{}-{id}", std::process::id())))
        }

        fn store(&self) -> Store {
            Store::open(&self.0).expect("open isolated test store")
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn test_session() -> SessionSummary {
        SessionSummary {
            id: "session-1".to_owned(),
            client: Some(noesora_engine::Client::Codex),
            source_namespace: "legacy:codex".to_owned(),
            external_id: "external-session-1".to_owned(),
            project: "project".to_owned(),
            title: "Transcript fixture".to_owned(),
            created_at: Some("2026-10-21T00:00:00Z".to_owned()),
            updated_at: Some("2026-10-21T00:00:00Z".to_owned()),
            messages: 0,
            snapshot: "snapshot-1".to_owned(),
        }
    }

    fn test_message(
        id: &str,
        role: &str,
        kind: &str,
        text: &str,
    ) -> noesora_engine::SessionMessage {
        noesora_engine::SessionMessage {
            id: id.to_owned(),
            role: role.to_owned(),
            kind: kind.to_owned(),
            text: text.to_owned(),
            timestamp: None,
        }
    }

    fn test_page(
        offset: usize,
        total: usize,
        messages: Vec<noesora_engine::SessionMessage>,
    ) -> TranscriptPage {
        TranscriptPage {
            session: test_session(),
            messages,
            offset,
            total,
        }
    }

    fn test_app(store: Store, page: TranscriptPage) -> App {
        let (sender, receiver) = mpsc::channel();
        App {
            store,
            filters: SearchFilters {
                project: None,
                client: None,
                source_namespace: None,
                scope: noesora_engine::ContentScope::Conversation,
                limit: SEARCH_LIMIT,
            },
            navigation: Navigation::default(),
            search_mode: SearchMode::Text,
            sessions: Vec::new(),
            sessions_offset: 0,
            has_more_sessions: false,
            selected_session: 0,
            select_after_page: 0,
            transcript: Some(page),
            transcript_request: None,
            transcript_request_offset: None,
            transcript_error: None,
            selected_message: 0,
            transcript_scroll: 0,
            transcript_viewport: RefCell::default(),
            expanded_tools: HashSet::new(),
            search_results: None,
            search_applied_query: None,
            search_error: None,
            selected_hit: 0,
            evidence_scroll: 0,
            session_error: None,
            store_status: None,
            readiness: Readiness::Loading,
            pending: Pending::None,
            startup: StartupState::Settled,
            generation: Generation::default(),
            sender,
            receiver,
        }
    }

    fn rendered_lines(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn finish_search_worker(app: &mut App) {
        let result = app
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("wait for search worker");
        app.sender.send(result).expect("queue search worker result");
        app.poll_workers();
    }

    // SPEC V32: range remains visible even when source identity wraps beyond metadata rows.
    #[test]
    fn v32_page_range_is_visible_with_long_identity_at_narrow_and_wide_widths() {
        let home = TestHome::new();
        let mut page = test_page(
            20,
            26,
            (0..6)
                .map(|i| test_message(&format!("m{i}"), "user", "message", "page body"))
                .collect(),
        );
        page.session.id = "identity".repeat(40);
        page.session.project = "project".repeat(40);
        page.session.snapshot = "snapshot".repeat(40);
        let app = test_app(home.store(), page);
        for width in [72, 120] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 26)).unwrap();
            terminal
                .draw(|frame| draw_transcript(frame, &app, frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let visible = (0..26)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                visible.contains("messages 21–26 of 26"),
                "page range hidden at {width} columns: {visible}"
            );
            assert!(
                visible.contains("page body"),
                "transcript body hidden at {width} columns"
            );
        }
    }

    #[test]
    fn session_pages_use_a_lookahead_and_do_not_skip_while_loading() {
        let home = TestHome::new();
        let mut app = test_app(home.store(), test_page(0, 0, Vec::new()));

        app.apply_sessions(Ok(vec![test_session(); SESSION_PAGE_SIZE]), 0, 0);
        assert_eq!(app.sessions.len(), SESSION_PAGE_SIZE);
        assert!(!app.has_more_sessions);
        app.page_sessions(1);
        assert_eq!(app.sessions_offset, 0);
        assert!(matches!(app.pending, Pending::None));

        app.apply_sessions(Ok(vec![test_session(); SESSION_PAGE_SIZE + 1]), 0, 0);
        assert_eq!(app.sessions.len(), SESSION_PAGE_SIZE);
        assert!(app.has_more_sessions);

        app.pending = Pending::Sessions;
        app.sessions_offset = SESSION_PAGE_SIZE;
        app.page_sessions(1);
        app.move_selection(1);
        assert_eq!(app.sessions_offset, SESSION_PAGE_SIZE);
    }

    #[test]
    fn canceled_session_page_keeps_rows_and_offset_until_current_result() {
        let home = TestHome::new();
        let mut app = test_app(home.store(), test_page(0, 0, Vec::new()));
        let mut old = test_session();
        old.id = "old-page".to_owned();
        app.sessions = vec![old];
        app.sessions_offset = SESSION_PAGE_SIZE;
        app.pending = Pending::Sessions;

        let stale_generation = app.generation.next();
        app.invalidate();
        let mut stale = test_session();
        stale.id = "stale-page".to_owned();
        app.sender
            .send(WorkerMessage {
                generation: stale_generation,
                payload: WorkerPayload::Sessions {
                    offset: SESSION_PAGE_SIZE * 2,
                    sessions: Ok(vec![stale]),
                },
            })
            .expect("queue canceled page result");
        app.poll_workers();

        assert_eq!(app.sessions_offset, SESSION_PAGE_SIZE);
        assert_eq!(app.sessions[0].id, "old-page");

        app.pending = Pending::Sessions;
        let current_generation = app.generation.next();
        let mut current = test_session();
        current.id = "current-page".to_owned();
        app.sender
            .send(WorkerMessage {
                generation: current_generation,
                payload: WorkerPayload::Sessions {
                    offset: SESSION_PAGE_SIZE * 2,
                    sessions: Ok(vec![current]),
                },
            })
            .expect("queue current page result");
        app.poll_workers();

        assert_eq!(app.sessions_offset, SESSION_PAGE_SIZE * 2);
        assert_eq!(app.sessions[0].id, "current-page");
    }

    #[test]
    fn selecting_another_session_rejects_pending_transcripts_with_wrong_id_or_offset() {
        let home = TestHome::new();
        let mut app = test_app(home.store(), test_page(0, 0, Vec::new()));
        let mut session_a = test_session();
        session_a.id = "session-a".to_owned();
        let mut session_b = test_session();
        session_b.id = "session-b".to_owned();
        app.sessions = vec![session_a, session_b];
        app.pending = Pending::Transcript;
        app.transcript_request = Some("session-a".to_owned());
        app.transcript_request_offset = Some(0);
        let pending_generation = app.generation.next();
        let mut stale_page = test_page(
            0,
            1,
            vec![test_message("message-a", "user", "message", "A transcript")],
        );
        stale_page.session.id = "session-a".to_owned();
        app.sender
            .send(WorkerMessage {
                generation: pending_generation,
                payload: WorkerPayload::Transcript {
                    id: "session-a".to_owned(),
                    offset: 0,
                    result: Ok(stale_page),
                },
            })
            .expect("queue pending transcript result");

        app.move_selection(1);
        app.poll_workers();

        assert_eq!(app.selected_session, 1);
        assert!(app.transcript.is_none());
        assert!(app.transcript_request.is_none());
        assert!(app.transcript_request_offset.is_none());
        assert!(matches!(app.pending, Pending::None));
        let wrong_session_generation = app.generation.next();
        let mut wrong_session_page = test_page(
            0,
            1,
            vec![test_message("message-a", "user", "message", "A transcript")],
        );
        wrong_session_page.session.id = "session-a".to_owned();
        app.sender
            .send(WorkerMessage {
                generation: wrong_session_generation,
                payload: WorkerPayload::Transcript {
                    id: "session-a".to_owned(),
                    offset: 0,
                    result: Ok(wrong_session_page),
                },
            })
            .expect("queue current-generation wrong-session result");
        app.poll_workers();
        assert!(app.transcript.is_none());

        app.pending = Pending::Transcript;
        app.transcript_request = Some("session-b".to_owned());
        app.transcript_request_offset = Some(1);
        let wrong_offset_generation = app.generation.next();
        let mut wrong_offset_page = test_page(
            0,
            1,
            vec![test_message("message-b", "user", "message", "B transcript")],
        );
        wrong_offset_page.session.id = "session-b".to_owned();
        app.sender
            .send(WorkerMessage {
                generation: wrong_offset_generation,
                payload: WorkerPayload::Transcript {
                    id: "session-b".to_owned(),
                    offset: 0,
                    result: Ok(wrong_offset_page),
                },
            })
            .expect("queue current-generation wrong-offset result");
        app.poll_workers();
        assert!(app.transcript.is_none());
        assert!(app.transcript_request_offset.is_none());

        app.pending = Pending::Transcript;
        app.transcript_request = Some("session-b".to_owned());
        app.transcript_request_offset = Some(0);
        let current_generation = app.generation.next();
        let mut current_page = test_page(
            0,
            1,
            vec![test_message("message-b", "user", "message", "B transcript")],
        );
        current_page.session.id = "session-b".to_owned();
        app.sender
            .send(WorkerMessage {
                generation: current_generation,
                payload: WorkerPayload::Transcript {
                    id: "session-b".to_owned(),
                    offset: 0,
                    result: Ok(current_page),
                },
            })
            .expect("queue selected transcript result");
        app.poll_workers();
        assert_eq!(
            app.transcript.as_ref().map(|page| page.session.id.as_str()),
            Some("session-b")
        );
    }

    #[test]
    fn canceled_startup_shows_not_loaded_and_restarts_when_sessions_return() {
        let home = TestHome::new();
        let mut app = test_app(home.store(), test_page(0, 0, Vec::new()));
        app.sessions_offset = SESSION_PAGE_SIZE * 2;
        app.startup.begin();
        app.pending = Pending::Startup;
        app.readiness = Readiness::Loading;

        assert!(matches!(
            app.handle_key(KeyCode::Char('/')),
            MemoryAction::None
        ));
        assert_eq!(app.startup, StartupState::Needed);
        assert!(matches!(&app.readiness, Readiness::NotLoaded));
        assert!(matches!(app.pending, Pending::None));

        assert!(matches!(app.handle_key(KeyCode::Esc), MemoryAction::None));
        assert_eq!(app.sessions_offset, SESSION_PAGE_SIZE * 2);
        assert_eq!(app.startup, StartupState::Loading);
        assert!(matches!(&app.readiness, Readiness::Loading));
        assert!(matches!(app.pending, Pending::Startup));
        // Result application is a generation transition, not a five-second disk-speed contract.
        let generation = app.generation.next();
        let status = app.store.status().map_err(|error| error.to_string());
        let model = embedding::model_status(&app.store.model_dir());
        app.sender
            .send(WorkerMessage {
                generation,
                payload: WorkerPayload::Startup {
                    sessions: Ok(Vec::new()),
                    status,
                    model,
                },
            })
            .expect("queue the current startup result");
        app.poll_workers();
        assert_eq!(app.sessions_offset, 0);
        assert_eq!(app.startup, StartupState::Settled);
        assert!(!matches!(
            &app.readiness,
            Readiness::NotLoaded | Readiness::Loading
        ));
    }

    #[test]
    fn stale_session_page_from_before_search_does_not_replace_visible_sessions() {
        let home = TestHome::new();
        let mut app = test_app(home.store(), test_page(0, 0, Vec::new()));
        let mut current = test_session();
        current.id = "current".to_owned();
        app.sessions = vec![current];
        let stale_generation = app.generation.next();

        assert!(matches!(
            app.handle_key(KeyCode::Char('/')),
            MemoryAction::None
        ));
        let mut stale = test_session();
        stale.id = "stale".to_owned();
        app.sender
            .send(WorkerMessage {
                generation: stale_generation,
                payload: WorkerPayload::Sessions {
                    offset: 0,
                    sessions: Ok(vec![stale]),
                },
            })
            .expect("queue stale session result");
        app.poll_workers();

        assert_eq!(app.sessions[0].id, "current");
        assert!(app.search_results.is_none());
    }

    #[test]
    fn worker_start_failures_are_reported_in_the_requested_view() {
        let home = TestHome::new();
        let mut app = test_app(home.store(), test_page(0, 0, Vec::new()));

        app.startup.begin();
        app.pending = Pending::Startup;
        app.readiness = Readiness::Loading;
        app.worker_start_failed(std::io::Error::other("simulated"));
        assert_eq!(app.startup, StartupState::Needed);
        assert!(matches!(&app.readiness, Readiness::NotLoaded));
        assert!(app.session_error.is_some());
        assert!(matches!(app.pending, Pending::None));

        app.pending = Pending::Transcript;
        app.transcript = None;
        app.navigation.focus = Focus::Detail;
        app.worker_start_failed(std::io::Error::other("simulated"));
        assert!(app.transcript_error.is_some());
        assert!(app.session_error.is_some());

        app.pending = Pending::Search;
        app.navigation.screen = Screen::Search;
        app.navigation.focus = Focus::List;
        app.worker_start_failed(std::io::Error::other("simulated"));
        assert!(app.search_error.is_some());
        assert!(matches!(app.pending, Pending::None));
    }

    #[test]
    fn engine_search_errors_remain_typed_and_text_search_recovers_verified_citations() {
        let home = TestHome::new();
        let store = home.store();
        let record = store
            .write_record(&noesora_engine::RecordInput {
                title: "Text recovery source".to_owned(),
                body: "This note contains evidence recovery marker in canonical source text."
                    .to_owned(),
                kind: "note".to_owned(),
                status: "done".to_owned(),
                project: "workspace".to_owned(),
                files: Vec::new(),
            })
            .expect("write recovery source");
        let query = "evidence recovery marker";
        let mut app = test_app(store.clone(), test_page(0, 0, Vec::new()));
        app.navigation.screen = Screen::Search;
        app.navigation.focus = Focus::List;
        app.navigation.query = query.to_owned();
        app.search_mode = SearchMode::Semantic;

        app.request_search();
        finish_search_worker(&mut app);
        let failure = match app.search_error.as_ref().expect("semantic Engine error") {
            SearchFailure::Engine(error) => error,
            SearchFailure::WorkerStart(_) => panic!("Store failure must remain an Engine error"),
        };
        assert_eq!(failure.code(), "MODEL_UNAVAILABLE");
        assert!(rendered_tui(&app, 72, 26).contains("MODEL_UNAVAILABLE"));

        app.search_mode = SearchMode::Text;
        app.request_search();
        finish_search_worker(&mut app);
        assert!(app.search_error.is_none());
        assert_eq!(app.search_applied_query.as_deref(), Some(query));
        let response = app.search_results.as_ref().expect("recovered text results");
        assert_eq!(response.hits.len(), 1);
        let hit = &response.hits[0];
        assert_eq!(hit.id, record.id);
        assert!(hit.client.is_none());
        assert!(hit.source_namespace.is_none());
        assert!(hit.external_id.is_none());
        store
            .verify_evidence(hit)
            .expect("recovered citation verifies against canonical source");
    }

    #[test]
    fn transcript_ranges_are_one_based_and_empty_pages_have_no_range() {
        let empty = test_page(0, 0, Vec::new());
        assert_eq!(transcript_page_range(&empty), None);

        let populated = test_page(
            20,
            26,
            (0..6)
                .map(|index| test_message(&format!("message-{index}"), "user", "message", "x"))
                .collect(),
        );
        assert_eq!(transcript_page_range(&populated), Some((21, 26)));
    }

    #[test]
    fn transcript_paging_stops_at_both_boundaries_and_uses_page_edges() {
        let messages = || {
            (0..TRANSCRIPT_PAGE_SIZE)
                .map(|index| test_message(&format!("message-{index}"), "user", "message", "x"))
                .collect::<Vec<_>>()
        };
        let first = test_page(0, 42, messages());
        let middle = test_page(20, 42, messages());
        let last = test_page(
            40,
            42,
            vec![
                test_message("message-40", "user", "message", "x"),
                test_message("message-41", "user", "message", "x"),
            ],
        );

        assert_eq!(transcript_page_offset(&first, -1), None);
        assert_eq!(transcript_page_offset(&first, 1), Some(20));
        assert_eq!(transcript_page_offset(&middle, -1), Some(0));
        assert_eq!(transcript_page_offset(&middle, 1), Some(40));
        assert_eq!(transcript_page_offset(&last, -1), Some(20));
        assert_eq!(transcript_page_offset(&last, 1), None);

        let home = TestHome::new();
        let mut app = test_app(home.store(), first);
        app.page_transcript(-1);
        assert_eq!(app.transcript.as_ref().map(|page| page.offset), Some(0));
        assert!(matches!(app.pending, Pending::None));
        app.transcript = Some(last);
        app.page_transcript(1);
        assert_eq!(app.transcript.as_ref().map(|page| page.offset), Some(40));
        assert!(matches!(app.pending, Pending::None));
    }

    #[test]
    fn selected_tool_message_toggles_collapse_and_expand_in_rendered_transcript() {
        let page = test_page(
            0,
            3,
            vec![
                test_message("user-1", "user", "message", "before"),
                test_message("tool-1", "tool", "tool_result", "private tool output"),
                test_message("assistant-1", "assistant", "assistant", "after"),
            ],
        );
        let home = TestHome::new();
        let mut app = test_app(home.store(), page);
        app.select_message(1);
        assert_eq!(app.selected_message, 1);

        let collapsed = transcript_lines(
            app.transcript.as_ref().unwrap(),
            &app.expanded_tools,
            app.selected_message,
        );
        let collapsed = rendered_lines(&collapsed);
        assert!(collapsed.contains("Tool body collapsed"));
        assert!(!collapsed.contains("private tool output"));

        app.toggle_tool();
        assert!(app.expanded_tools.contains("tool-1"));
        let expanded = transcript_lines(
            app.transcript.as_ref().unwrap(),
            &app.expanded_tools,
            app.selected_message,
        );
        let expanded = rendered_lines(&expanded);
        assert!(!expanded.contains("Tool body collapsed"));
        assert!(expanded.contains("private tool output"));

        app.toggle_tool();
        assert!(!app.expanded_tools.contains("tool-1"));
        let collapsed = transcript_lines(
            app.transcript.as_ref().unwrap(),
            &app.expanded_tools,
            app.selected_message,
        );
        let collapsed = rendered_lines(&collapsed);
        assert!(collapsed.contains("Tool body collapsed"));
        assert!(!collapsed.contains("private tool output"));
    }

    #[test]
    fn transcript_render_sanitizes_terminal_controls_without_changing_source_text() {
        let source = "雪\n\tkeep\u{001b}]52;c;clipboard\u{0007}\u{001b}[31mred\u{001b}[0m";
        let page = test_page(
            0,
            1,
            vec![test_message("user-1", "user", "message", source)],
        );
        let lines = transcript_lines(&page, &HashSet::new(), 0);

        assert_eq!(
            rendered_lines(&lines),
            "user  message  -\n雪\n\tkeep�]52;c;clipboard��[31mred�[0m"
        );
        assert_eq!(page.messages[0].text, source);
    }

    fn rendered_tui(app: &App, width: u16, height: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, frame.area(), app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn selected_search_evidence_renders_verified_neutral_provenance_and_locators() {
        let home = TestHome::new();
        let store = home.store();
        let source = home.0.join("neutral-evidence.jsonl");
        let bytes = concat!(
            r#"{"type":"session","schema":"noesora.session/v1","source_namespace":"fictional.ide-alpha","external_id":"neutral-session","project":"workspace","title":"TUI evidence fixture","created_at":null,"updated_at":null}"#,
            "\n",
            r#"{"type":"message","external_id":"message-proof","role":"assistant","kind":"message","timestamp":null,"payload":{"content":{"text":"violet lantern citation"}},"parts":[{"field":"/content/text","scope":"conversation"}]}"#,
            "\n"
        );
        std::fs::write(&source, bytes).expect("write neutral search fixture");
        store
            .ingest_session(&source)
            .expect("ingest neutral TUI fixture");

        let beta_source = home.0.join("neutral-beta.jsonl");
        let beta_bytes = concat!(
            r#"{"type":"session","schema":"noesora.session/v1","source_namespace":"fictional.ide-beta","external_id":"beta-session","project":"workspace","title":"TUI beta fixture","created_at":null,"updated_at":null}"#,
            "\n",
            r#"{"type":"message","external_id":"beta-message","role":"assistant","kind":"message","timestamp":null,"payload":{"content":{"text":"violet lantern citation"}},"parts":[{"field":"/content/text","scope":"conversation"}]}"#,
            "\n"
        );
        std::fs::write(&beta_source, beta_bytes).expect("write beta namespace fixture");
        store
            .ingest_session(&beta_source)
            .expect("ingest beta namespace fixture");
        let query = "violet lantern citation";
        let filters = SearchFilters {
            project: Some("workspace".to_owned()),
            client: None,
            source_namespace: Some("fictional.ide-alpha".to_owned()),
            scope: noesora_engine::ContentScope::Conversation,
            limit: SEARCH_LIMIT,
        };
        let response = store
            .search(query, SearchMode::Text, &filters)
            .expect("search neutral TUI fixture");
        assert_eq!(response.hits.len(), 1);
        let unfiltered = SearchFilters {
            project: Some("workspace".to_owned()),
            client: None,
            source_namespace: None,
            scope: noesora_engine::ContentScope::Conversation,
            limit: SEARCH_LIMIT,
        };
        assert_eq!(
            store
                .search(query, SearchMode::Text, &unfiltered)
                .expect("search both neutral namespaces")
                .hits
                .len(),
            2
        );
        let hit = &response.hits[0];
        store
            .verify_evidence(hit)
            .expect("selected Engine evidence must verify against its source");
        assert!(hit.client.is_none());
        assert_eq!(hit.source_namespace.as_deref(), Some("fictional.ide-alpha"));
        assert_eq!(hit.external_id.as_deref(), Some("neutral-session"));
        let session_id = hit.session_id.as_ref().expect("session ID").clone();
        let message_id = hit.message_id.as_ref().expect("message ID").clone();
        let path = hit.path.clone();
        let hash = hit.hash.clone();
        let source_span = format!("{}..{}", hit.span[0], hit.span[1]);
        let pointer = hit.field.clone().expect("JSON pointer");
        let text_span = hit
            .text_span
            .map(|span| format!("{}..{}", span[0], span[1]))
            .expect("decoded text span");
        let quote = hit.text.clone();

        let mut app = test_app(store, test_page(0, 0, Vec::new()));
        app.navigation.screen = Screen::Search;
        app.navigation.focus = Focus::Detail;
        app.navigation.query = query.to_owned();
        app.search_applied_query = Some(query.to_owned());
        app.search_results = Some(response);

        let compact = |text: &str| {
            text.chars()
                .filter(|ch| !ch.is_whitespace() && *ch != '│')
                .collect::<String>()
        };
        for (width, height) in [(72, 26), (120, 38)] {
            app.evidence_scroll = 0;
            let top = rendered_tui(&app, width, height);
            let compact_top = compact(&top);
            for expected in [
                "fictional.ide-alpha",
                "neutral-session",
                "not applicable",
                &session_id,
                &message_id,
                &path,
            ] {
                assert!(
                    compact_top.contains(&compact(expected)),
                    "missing selected identity {expected:?} at {width} columns:\n{top}"
                );
            }

            app.evidence_scroll = 8;
            let detail = rendered_tui(&app, width, height);
            let compact_detail = compact(&detail);
            for expected in [&source_span, &hash, &pointer, &text_span, &quote] {
                assert!(
                    compact_detail.contains(&compact(expected)),
                    "missing selected citation {expected:?} at {width} columns:\n{detail}"
                );
            }
        }
    }

    #[test]
    fn long_header_metadata_keeps_readiness_and_query_visible() {
        let home = TestHome::new();
        let page = test_page(0, 0, Vec::new());
        let mut app = test_app(home.store(), page);
        let long_root = std::path::PathBuf::from(format!(
            "/canonical/archive/{}",
            "long-store-root-segment/".repeat(12)
        ));
        app.store_status = Some(StoreStatus {
            root: long_root.clone(),
            sessions: 7,
            messages: 11,
            records: 13,
            chunks: 17,
            embedded_chunks: 14,
            pending_embeddings: 3,
        });
        app.readiness = Readiness::Error("known readiness failure: model offline".to_owned());
        app.navigation.screen = Screen::Search;
        app.navigation.focus = Focus::Input;
        app.navigation.query = "status query".to_owned();

        for width in [72, 120] {
            let visible = rendered_tui(&app, width, 26);
            let header = visible.lines().take(4).collect::<Vec<_>>();
            assert_eq!(header.len(), 4);
            assert!(header[1].contains("All projects · 7 sessions · /canonical/archive/"));
            assert!(header[1].contains('…'));
            assert!(
                header[2].contains("Store status error: known readiness failure: model offline"),
                "readiness row hidden or clipped at {width} columns: {}",
                header[2]
            );
            assert!(
                header[3].contains("Search [text]") && header[3].contains("status query"),
                "search query hidden at {width} columns: {}",
                header[3]
            );
        }

        assert_eq!(app.store_status.as_ref().unwrap().root, long_root);
        assert!(matches!(
            &app.readiness,
            Readiness::Error(message) if message == "known readiness failure: model offline"
        ));
        assert_eq!(app.navigation.query, "status query");
    }

    #[test]
    fn v38_ascii_tool_tail_after_control_lines_is_reachable_by_incremental_keys() {
        let tool_text = format!(
            "untrusted fixture\nsource-osc52: \u{1b}]52;c;probe\u{7}\nsource-c1-osc52: \u{9d}52;c;probe\u{7}\ntool-wrap-start-marker {} tool-wrap-tail-marker\nvisible-tail-marker",
            "T".repeat(4096)
        );
        for (width, height) in [(72, 26), (120, 38)] {
            let page = test_page(
                20,
                26,
                vec![
                    test_message("before-tool", "user", "message", "later-page-marker"),
                    test_message("tool", "tool", "tool_result", &tool_text),
                    test_message("after-tool", "assistant", "message", "following message"),
                ],
            );
            let home = TestHome::new();
            let mut app = test_app(home.store(), page);
            app.navigation.focus = Focus::Detail;
            app.handle_key(KeyCode::Right);
            app.handle_key(KeyCode::Char('x'));
            let initial = rendered_tui(&app, width, height);
            assert!(initial.contains("tool-wrap-start-marker"));
            assert!(!initial.contains("tool-wrap-tail-marker"));
            assert!(initial.contains("source-c1-osc52: �52;c;probe�"));
            let mut visible = initial;
            let mut reached_tail = false;
            for _ in 0..128 {
                app.handle_key(KeyCode::Down);
                visible = rendered_tui(&app, width, height);
                if visible.contains("tool-wrap-tail-marker")
                    && visible.contains("visible-tail-marker")
                {
                    reached_tail = true;
                    break;
                }
            }
            assert!(
                reached_tail,
                "tool tail unreachable at {width}x{height}, scroll={}, viewport row={}: {visible}",
                app.transcript_scroll,
                app.transcript_viewport.borrow().row,
            );
        }
    }
    #[test]
    fn transcript_tails_past_u16_scroll_limit_at_narrow_and_wide_widths() {
        const ROWS: usize = 65_700;
        let mut text = "ordinary row\n".repeat(ROWS);
        text.push_str("TAIL_BEYOND_U16");

        for width in [72, 120] {
            for (tool, id, role, kind) in [
                (false, "message-1", "user", "message"),
                (true, "tool-1", "tool", "tool_result"),
            ] {
                let page = test_page(0, 1, vec![test_message(id, role, kind, &text)]);
                let home = TestHome::new();
                let mut app = test_app(home.store(), page);
                app.navigation.focus = Focus::Detail;
                if tool {
                    app.toggle_tool();
                }

                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 26)).unwrap();
                terminal
                    .draw(|frame| draw_transcript(frame, &app, frame.area()))
                    .unwrap();
                for _ in 0..ROWS {
                    assert!(matches!(app.handle_key(KeyCode::Down), MemoryAction::None));
                }
                assert!(
                    app.transcript_scroll > u16::MAX as usize + 26,
                    "test did not navigate beyond the scroll boundary at {width} columns"
                );
                terminal
                    .draw(|frame| draw_transcript(frame, &app, frame.area()))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let visible = (0..26)
                    .map(|y| {
                        (0..width)
                            .map(|x| buffer[(x, y)].symbol())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    visible.contains("TAIL_BEYOND_U16"),
                    "transcript tail not visible at {width} columns (tool={tool})"
                );
            }
        }
    }

    fn paragraph_cells(
        lines: Vec<Line<'_>>,
        width: u16,
        height: u16,
        scroll: u16,
    ) -> Vec<(String, Color, Color, Modifier)> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(Text::from(lines))
                        .scroll((scroll, 0))
                        .wrap(Wrap { trim: false }),
                    frame.area(),
                );
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| (cell.symbol().to_owned(), cell.fg, cell.bg, cell.modifier))
            .collect()
    }

    #[test]
    fn virtualized_rows_match_paragraph_wrapping_at_representable_offsets() {
        let text = format!(
            "alpha beta-gamma delta\n  leading\tspace\n{}\nwide e\u{301}雪\u{301}\n{}\u{001b}\u{301}tail \u{001b}]52;c;clipboard\u{0007}\n{}",
            "abcdef".repeat(16),
            "x".repeat(71),
            "ending-word".repeat(12),
        );
        for tool in [false, true] {
            let page = test_page(
                0,
                1,
                vec![test_message(
                    if tool { "tool-1" } else { "message-1" },
                    if tool { "tool" } else { "user" },
                    if tool { "tool_result" } else { "message" },
                    &text,
                )],
            );
            let home = TestHome::new();
            let mut app = test_app(home.store(), page);
            if tool {
                app.toggle_tool();
            }
            let page = app.transcript.as_ref().unwrap();
            let mut viewport = TranscriptViewport::default();
            let baseline = transcript_lines(page, &app.expanded_tools, 0);
            for width in [12, 72, 120] {
                for scroll in 0..32 {
                    let position = viewport.position_at(
                        page,
                        &app.expanded_tools,
                        app.selected_message,
                        width,
                        scroll,
                    );
                    let virtualized = transcript_window_lines(
                        page,
                        &app.expanded_tools,
                        app.selected_message,
                        position,
                        width,
                        8,
                    );
                    assert_eq!(
                        paragraph_cells(virtualized, width, 8, 0),
                        paragraph_cells(baseline.clone(), width, 8, scroll as u16),
                        "wrapped content differs at {width} columns, tool={tool}, row={scroll}"
                    );
                }
            }
        }
    }

    #[test]
    fn v22_q_is_text_only_while_input_has_focus() {
        let mut navigation = Navigation::default();
        assert_eq!(
            navigation.handle_key(KeyCode::Char('/')),
            Action::SearchStarted
        );
        assert_eq!(navigation.focus, Focus::Input);
        assert_eq!(
            navigation.handle_key(KeyCode::Char('q')),
            Action::QueryChanged
        );
        assert_eq!(navigation.query, "q");
        assert_eq!(navigation.handle_key(KeyCode::Enter), Action::RunSearch);
        assert_eq!(navigation.screen, Screen::Search);
        assert_eq!(navigation.handle_key(KeyCode::Char('q')), Action::Quit);
        navigation.focus = Focus::Detail;
        assert_eq!(navigation.handle_key(KeyCode::Char('q')), Action::Quit);

        navigation.screen = Screen::Sessions;
        navigation.focus = Focus::List;
        assert_eq!(navigation.handle_key(KeyCode::Char('q')), Action::Quit);
    }

    #[test]
    fn tab_cycles_visible_search_focus_and_escape_returns_to_sessions() {
        let mut navigation = Navigation::default();
        navigation.handle_key(KeyCode::Char('/'));
        assert_eq!(navigation.focus, Focus::Input);
        navigation.handle_key(KeyCode::Tab);
        assert_eq!(navigation.focus, Focus::List);
        navigation.handle_key(KeyCode::Tab);
        assert_eq!(navigation.focus, Focus::Detail);
        navigation.handle_key(KeyCode::Tab);
        assert_eq!(navigation.focus, Focus::Input);
        assert_eq!(navigation.handle_key(KeyCode::Esc), Action::BackToSessions);
        assert_eq!(navigation.screen, Screen::Sessions);
    }

    #[test]
    fn stale_worker_generations_are_rejected() {
        let generations = Generation::default();
        let stale = generations.next();
        let current = generations.next();
        assert!(!generations.current(stale));
        assert!(generations.current(current));
    }
    #[test]
    fn canceled_startup_restarts_on_return_until_completion() {
        let mut navigation = Navigation::default();
        let mut startup = StartupState::default();

        startup.begin();
        assert_eq!(
            navigation.handle_key(KeyCode::Char('/')),
            Action::SearchStarted
        );
        startup.cancel();
        assert_eq!(
            navigation.handle_key(KeyCode::Char('x')),
            Action::QueryChanged
        );
        startup.cancel();
        assert!(startup.needs_request());
        assert_eq!(navigation.handle_key(KeyCode::Esc), Action::BackToSessions);
        if startup.needs_request() {
            startup.begin();
        }
        assert_eq!(startup, StartupState::Loading);

        startup.settle();
        assert_eq!(startup, StartupState::Settled);
        assert!(!startup.needs_request());

        startup.begin();
        assert_eq!(
            navigation.handle_key(KeyCode::Char('/')),
            Action::SearchStarted
        );
        startup.cancel();
        assert_eq!(navigation.handle_key(KeyCode::Esc), Action::BackToSessions);
        assert!(startup.needs_request());
        startup.begin();
        startup.settle();
        assert!(!startup.needs_request());
    }
}
