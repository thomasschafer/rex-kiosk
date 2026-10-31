use std::{
    fs,
    sync::{atomic::AtomicBool, mpsc},
    time::Duration,
};

use kiosk_core::{
    backend::KnownCheckout,
    config::on_open::{OnOpenPaneConfig, OnOpenPaneDirection},
    event::{AppEvent, WorktreeRemovalOutcome},
    git::{
        CliGitProvider,
        test_fixtures::{repo_fixture, run_test_git},
    },
    spawn::{EventSender, spawn_create_new_branch, spawn_open_branch, spawn_worktree_removal},
};
use tempfile::TempDir;

use super::*;
use crate::rex::SHELL_FLAVOR;

const LOCAL_HOST: &str = "host-local";
const APP: &str = "client:app";

#[derive(Debug, Clone)]
struct FakeBlock {
    id: String,
    /// The foreground process's folder.
    folder: Option<PathBuf>,
    /// What the shell last reported with OSC 7.
    pwd: Option<String>,
    host: String,
    /// What was typed into the terminal as it started.
    input: Option<String>,
}

#[derive(Debug, Clone)]
struct FakeWindow {
    id: String,
    label: String,
    tiled: Vec<FakeBlock>,
    floating: Vec<FakeBlock>,
}

#[derive(Debug, Clone)]
struct FakeSession {
    id: String,
    label: String,
    windows: Vec<FakeWindow>,
    active: Option<String>,
}

#[derive(Default)]
struct FakeState {
    sessions: Vec<FakeSession>,
    next_id: usize,
    calls: Vec<(String, Value)>,
    apps: Vec<(String, String)>,
    perform_error: Option<RexError>,
    /// Blocks that close between a view listing them and a call on them.
    vanishing: Vec<String>,
    /// Sessions that close between being listed and being viewed.
    ghosts: Vec<String>,
    /// Why forgetting a saved removal fails, if it does.
    forget_error: Option<String>,
}

/// The parts of the Rex server kiosk uses, over in-memory sessions.
#[derive(Default)]
struct FakeRex {
    state: Mutex<FakeState>,
}

const READS: [&str; 6] = [
    "server.status",
    "session.list",
    "session.view",
    "client.list",
    "com.superlogical.terminal.process",
    "com.superlogical.terminal.pwd",
];

impl FakeRex {
    fn with_app() -> Self {
        let rex = Self::default();
        rex.state
            .lock()
            .unwrap()
            .apps
            .push((APP.into(), "Mac".into()));
        rex
    }

    fn id(state: &mut FakeState, kind: &str) -> String {
        state.next_id += 1;
        format!("{kind}:{}", state.next_id)
    }

    fn block(state: &mut FakeState, folder: Option<&Path>) -> FakeBlock {
        FakeBlock {
            id: Self::id(state, "block"),
            folder: folder.map(Path::to_path_buf),
            pwd: None,
            host: LOCAL_HOST.into(),
            input: None,
        }
    }

    /// Adds a session with a window per folder, each with one terminal there.
    fn add_session(&self, label: &str, folders: &[&Path]) -> FakeSession {
        let mut state = self.state.lock().unwrap();
        let id = Self::id(&mut state, "session");
        let windows: Vec<FakeWindow> = folders
            .iter()
            .map(|folder| {
                let tiled = vec![Self::block(&mut state, Some(folder))];
                FakeWindow {
                    id: Self::id(&mut state, "window"),
                    label: "tab".into(),
                    tiled,
                    floating: Vec::new(),
                }
            })
            .collect();
        let session = FakeSession {
            id,
            label: label.into(),
            active: windows.last().map(|window| window.id.clone()),
            windows,
        };
        state.sessions.push(session.clone());
        session
    }

    fn sessions(&self) -> Vec<FakeSession> {
        self.state.lock().unwrap().sessions.clone()
    }

    fn edit(&self, edit: impl FnOnce(&mut FakeState)) {
        edit(&mut self.state.lock().unwrap());
    }

    /// The calls that change something, in order.
    fn changes(&self) -> Vec<(String, Value)> {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(method, _)| !READS.contains(&method.as_str()))
            .cloned()
            .collect()
    }

    fn change_names(&self) -> Vec<String> {
        self.changes()
            .into_iter()
            .map(|(method, _)| method)
            .collect()
    }
}

fn not_found(what: &str) -> RexError {
    RexError::Api {
        code: "not_found".into(),
        message: format!("{what} not found"),
    }
}

/// The blocks a layout makes, in layout order.
fn layout_blocks(state: &mut FakeState, layout: &Value) -> Vec<FakeBlock> {
    if let Some(split) = layout.get("split") {
        let mut blocks = layout_blocks(state, &split["before"]);
        blocks.extend(layout_blocks(state, &split["after"]));
        return blocks;
    }
    let block = &layout["block"];
    assert_eq!(block["flavor"], SHELL_FLAVOR);
    let options = &block["options"];
    let mut made = FakeRex::block(state, options["cwd"].as_str().map(Path::new));
    made.input = options["initial_input"].as_str().map(str::to_owned);
    vec![made]
}

fn session_index(state: &FakeState, args: &Value) -> Result<usize, RexError> {
    let id = args["session_id"].as_str().unwrap();
    state
        .sessions
        .iter()
        .position(|session| session.id == id)
        .ok_or_else(|| RexError::Api {
            code: "session_not_found".into(),
            message: format!("session {id} not found"),
        })
}

fn view_block(block: &FakeBlock) -> Value {
    json!({
        "block_id": block.id,
        "creator_name": "com.superlogical.terminal",
        "flavor": SHELL_FLAVOR,
        "host_id": block.host,
    })
}

fn view(session: &FakeSession) -> Value {
    let windows: Vec<Value> = session
        .windows
        .iter()
        .map(|window| {
            let mut layers = vec![json!({
                "layer_id": "layer:tiled", "kind": "tiled", "bounds": [0, 0, 1, 1],
                "blocks": window.tiled.iter().map(view_block).collect::<Vec<_>>(),
            })];
            if !window.floating.is_empty() {
                layers.push(json!({
                    "layer_id": "layer:float", "kind": "floating",
                    "bounds": [0.1, 0.1, 0.8, 0.8],
                    "blocks": window.floating.iter().map(view_block).collect::<Vec<_>>(),
                }));
            }
            json!({ "window_id": window.id, "label": window.label, "layers": layers })
        })
        .collect();
    let mut view = json!({ "revision": 1, "session_id": session.id, "label": session.label });
    if !windows.is_empty() {
        view["windows"] = windows.into();
    }
    view
}

/// A terminal block's `process` or `pwd`.
fn terminal(state: &FakeState, method: &str, args: &Value) -> Result<Value, RexError> {
    assert_eq!(args["args"], json!({}));
    let id = args["block_id"].as_str().unwrap();
    if state.vanishing.iter().any(|gone| gone == id) {
        return Err(not_found("block"));
    }
    let session = &state.sessions[session_index(state, args)?];
    let block = session
        .windows
        .iter()
        .flat_map(|window| window.tiled.iter().chain(&window.floating))
        .find(|block| block.id == id)
        .ok_or_else(|| not_found("block"))?;
    Ok(if method.ends_with("process") {
        let mut foreground = json!({ "pid": 2, "user_id": 501, "name": "zsh" });
        if let Some(folder) = &block.folder {
            foreground["cwd"] = folder.to_str().unwrap().into();
        }
        json!({ "last_exit": null, "foreground": foreground })
    } else {
        block
            .pwd
            .as_ref()
            .map_or_else(|| json!({}), |pwd| json!({ "pwd": pwd }))
    })
}

/// The window a `session.create` or `session.new_window` request asks for.
fn made_window(state: &mut FakeState, request: &Value) -> FakeWindow {
    let tiled = layout_blocks(state, &request["layout"]);
    FakeWindow {
        id: FakeRex::id(state, "window"),
        label: request["window_label"].as_str().unwrap().into(),
        tiled,
        floating: Vec::new(),
    }
}

fn window_created(window: &FakeWindow) -> Value {
    json!({
        "window_id": window.id,
        "block_ids": window.tiled.iter().map(|block| &block.id).collect::<Vec<_>>(),
        "revision": 1,
    })
}

fn clients(state: &FakeState) -> Value {
    let apps = state.apps.iter().map(|(id, label)| {
        json!({
            "client_id": id,
            "info": { "kind": "app", "platform": "macos", "label": label },
        })
    });
    let cli = json!({ "client_id": "client:cli", "info": { "kind": "cli" } });
    json!({ "clients": apps.chain([cli]).collect::<Vec<_>>() })
}

impl RexApi for FakeRex {
    fn call(&self, method: &str, args: &Value) -> Result<Value, RexError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push((method.into(), args.clone()));
        match method {
            "server.status" => Ok(json!({ "host_id": LOCAL_HOST, "pid": 1 })),
            "session.list" => Ok(json!({ "sessions": state.sessions.iter().map(|session| {
                json!({ "session_id": session.id, "label": session.label })
            }).chain(state.ghosts.iter().map(|id| json!({ "session_id": id, "label": "ghost" })))
                .collect::<Vec<_>>() })),
            "session.view" => Ok(view(&state.sessions[session_index(&state, args)?])),
            "com.superlogical.terminal.process" | "com.superlogical.terminal.pwd" => {
                terminal(&state, method, args)
            }
            "session.create" => {
                let id = Self::id(&mut state, "session");
                let windows: Vec<FakeWindow> = args["initial_windows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|window| made_window(&mut state, window))
                    .collect();
                let output = json!({
                    "session_id": id,
                    "initial_windows": windows.iter().map(window_created).collect::<Vec<_>>(),
                });
                state.sessions.push(FakeSession {
                    id,
                    label: args["label"].as_str().unwrap().into(),
                    active: windows.last().map(|window| window.id.clone()),
                    windows,
                });
                Ok(output)
            }
            "session.new_window" => {
                let index = session_index(&state, args)?;
                let window = made_window(&mut state, args);
                let output = window_created(&window);
                let session = &mut state.sessions[index];
                session.active = Some(window.id.clone());
                session.windows.push(window);
                Ok(output)
            }
            "session.focus_window" => {
                let index = session_index(&state, args)?;
                let session = &mut state.sessions[index];
                let window = args["window_id"].as_str().unwrap();
                if !session
                    .windows
                    .iter()
                    .any(|candidate| candidate.id == window)
                {
                    return Err(not_found("window"));
                }
                session.active = Some(window.into());
                Ok(json!({ "revision": 3 }))
            }
            "session.close_window" => {
                let index = session_index(&state, args)?;
                let window = args["window_id"].as_str().unwrap();
                state.sessions[index]
                    .windows
                    .retain(|candidate| candidate.id != window);
                Ok(json!({ "revision": 4 }))
            }
            "session.destroy" => {
                let index = session_index(&state, args)?;
                state.sessions.remove(index);
                Ok(json!({}))
            }
            "block.close" => {
                let index = session_index(&state, args)?;
                let id = args["block_id"].as_str().unwrap();
                let session = &mut state.sessions[index];
                for window in &mut session.windows {
                    window.tiled.retain(|block| block.id != id);
                    window.floating.retain(|block| block.id != id);
                }
                // Rex closes a window once its last block closes.
                session
                    .windows
                    .retain(|window| !window.tiled.is_empty() || !window.floating.is_empty());
                Ok(json!({ "block_id": id, "revision": 5 }))
            }
            "client.list" => Ok(clients(&state)),
            other => panic!("unexpected call {other} {args}"),
        }
    }

    fn perform(&self, client: &ClientId, action: &str, args: &Value) -> Result<(), RexError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push((
            format!("do {action}"),
            json!({ "client": client.as_str(), "args": args }),
        ));
        assert_eq!(action, "session.select");
        if let Some(error) = state.perform_error.clone() {
            return Err(error);
        }
        if !state.apps.iter().any(|(id, _)| id == client.as_str()) {
            return Err(RexError::Invocation(format!(
                "client target {client:?} not found"
            )));
        }
        Ok(())
    }
}

struct Fixture {
    temp: TempDir,
    repo: PathBuf,
    rex: Arc<FakeRex>,
    backend: Arc<Rex>,
    notices: Arc<Mutex<Vec<u8>>>,
}

/// Shares written bytes with the test, and logs what each flush sends among the Rex
/// calls as a `notice`, so tests can see when it reached the terminal.
struct SharedWriter {
    written: Arc<Mutex<Vec<u8>>>,
    unflushed: Vec<u8>,
    rex: Arc<FakeRex>,
}

impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.unflushed.extend_from_slice(bytes);
        self.written.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let flushed = String::from_utf8(std::mem::take(&mut self.unflushed)).unwrap();
        self.rex
            .state
            .lock()
            .unwrap()
            .calls
            .push(("notice".into(), flushed.into()));
        Ok(())
    }
}

impl Fixture {
    fn new() -> Self {
        Self::with(
            OnOpenConfig::default(),
            Launch::default(),
            FakeRex::with_app(),
        )
    }

    fn with(on_open: OnOpenConfig, launch: Launch, rex: FakeRex) -> Self {
        let temp = TempDir::new().unwrap();
        let repo = canonical(&repo_fixture(temp.path(), "repo"));
        let rex = Arc::new(rex);
        let notices = Arc::new(Mutex::new(Vec::new()));
        let backend = Arc::new(Rex::new(
            Arc::clone(&rex) as Arc<dyn RexApi>,
            Arc::new(CliGitProvider),
            WorktreePlacement::new(temp.path().join("worktrees")).unwrap(),
            on_open,
            launch,
            Box::new(SharedWriter {
                written: Arc::clone(&notices),
                unflushed: Vec::new(),
                rex: Arc::clone(&rex),
            }),
            forget_logged_in(Arc::clone(&rex)),
        ));
        Self {
            temp,
            repo,
            rex,
            backend,
            notices,
        }
    }

    fn linked(&self, branch: &str) -> PathBuf {
        let path = self.temp.path().join(format!("repo-{branch}"));
        run_test_git(
            &self.repo,
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
        );
        canonical(&path)
    }

    fn folder(&self, name: &str) -> PathBuf {
        let path = self.temp.path().join(name);
        fs::create_dir_all(&path).unwrap();
        canonical(&path)
    }

    fn checkout_window(&self, path: &Path) -> Option<WindowRef> {
        self.backend
            .checkouts(&self.repo)
            .unwrap()
            .into_iter()
            .find(|state| canonical(&state.path) == canonical(path))
            .unwrap_or_else(|| panic!("{} is not a checkout", path.display()))
            .session
    }

    fn run(&self, start: impl FnOnce(&Arc<dyn GitProvider>, &Arc<Rex>, &EventSender)) -> AppEvent {
        let git: Arc<dyn GitProvider> = Arc::new(CliGitProvider);
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        start(&git, &self.backend, &sender);
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn remove(&self, branch: &str, checkout: &Path, force: bool) -> WorktreeRemovalOutcome {
        let event = self.run(|git, backend, sender| {
            spawn_worktree_removal(
                git,
                backend,
                sender,
                crate::PRODUCT.backend_name,
                self.repo.clone(),
                branch.into(),
                checkout.into(),
                force,
            );
        });
        let AppEvent::WorktreeRemovalFinished { outcome, .. } = event else {
            panic!("expected a removal, got {event:?}");
        };
        outcome
    }
}

/// Logs each forgotten removal among the Rex calls as a `forget`, or fails as the fake
/// says to.
fn forget_logged_in(rex: Arc<FakeRex>) -> ForgetRemoval {
    Box::new(move |checkout| {
        let mut state = rex.state.lock().unwrap();
        if let Some(error) = state.forget_error.clone() {
            return Err(error);
        }
        state
            .calls
            .push(("forget".into(), checkout.to_str().unwrap().into()));
        Ok(())
    })
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

fn default_branch(repo: &Path) -> String {
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn show_calls(session: &str, window: &str) -> Vec<(String, Value)> {
    let ids = json!({ "session_id": session, "window_id": window });
    vec![
        ("session.focus_window".into(), ids.clone()),
        (
            "do session.select".into(),
            json!({ "client": APP, "args": ids }),
        ),
    ]
}

fn editor_on_open() -> OnOpenConfig {
    OnOpenConfig {
        panes: vec![OnOpenPaneConfig {
            command: "hx".into(),
            direction: OnOpenPaneDirection::Right,
            ratio: Some(0.3),
        }],
    }
}

#[test]
fn opening_a_closed_repo_makes_a_session_for_it_with_a_window_for_the_branch() {
    let fixture = Fixture::new();

    let opened = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(opened, Opened::default());
    let sessions = fixture.rex.sessions();
    let [session] = sessions.as_slice() else {
        panic!("expected one session, got {sessions:?}");
    };
    assert_eq!(session.label, "repo");
    let [window] = session.windows.as_slice() else {
        panic!("expected one window, got {:?}", session.windows);
    };
    assert_eq!(window.label, default_branch(&fixture.repo));
    assert_eq!(
        window.tiled[0].folder.as_deref(),
        Some(fixture.repo.as_path())
    );
    let changes = fixture.rex.changes();
    assert_eq!(changes[0].0, "session.create");
    assert_eq!(changes[1..], show_calls(&session.id, &window.id));
}

#[test]
fn opening_a_checkout_already_shown_selects_its_window_without_on_open_panes() {
    let fixture = Fixture::with(editor_on_open(), Launch::default(), FakeRex::with_app());
    let feature = fixture.linked("feature");
    let src = fixture.repo.join("src");
    fs::create_dir(&src).unwrap();
    fixture
        .rex
        .add_session("elsewhere", &[&fixture.folder("other")]);
    let session = fixture.rex.add_session("mine", &[&feature, &src]);

    fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(
        fixture.rex.changes(),
        show_calls(&session.id, &session.windows[1].id)
    );
}

#[test]
fn another_checkout_of_a_repo_opens_as_a_window_in_its_session() {
    let fixture = Fixture::with(editor_on_open(), Launch::default(), FakeRex::with_app());
    let feature = fixture.linked("feature");
    let session = fixture
        .rex
        .add_session("renamed by the user", &[&fixture.repo]);

    fixture
        .backend
        .open_checkout(&fixture.repo, &feature)
        .unwrap();

    let sessions = fixture.rex.sessions();
    assert_eq!(sessions.len(), 1);
    let window = &sessions[0].windows[1];
    assert_eq!(window.label, "feature");
    let started: Vec<_> = window
        .tiled
        .iter()
        .map(|block| (block.folder.clone().unwrap(), block.input.clone()))
        .collect();
    assert_eq!(
        started,
        [
            (feature.clone(), None),
            (feature.clone(), Some("hx\n".into()))
        ]
    );
    let changes = fixture.rex.changes();
    assert_eq!(changes[0].0, "session.new_window");
    assert_eq!(changes[0].1["session_id"], session.id.as_str());
    assert_eq!(changes[0].1["layout"]["split"]["ratio"], 0.7);
    assert_eq!(changes[1..], show_calls(&session.id, &window.id));
    assert_eq!(
        fixture.checkout_window(&feature),
        Some(WindowRef {
            session: SessionId::try_from(session.id).unwrap(),
            window: WindowId::try_from(window.id.clone()).unwrap(),
        })
    );
}

#[test]
fn a_session_belongs_to_the_repo_of_its_first_window_only() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    let other = fixture.folder("notes");
    fixture.rex.add_session("misc", &[&other, &fixture.repo]);

    fixture
        .backend
        .open_checkout(&fixture.repo, &feature)
        .unwrap();

    let sessions = fixture.rex.sessions();
    assert_eq!(sessions.len(), 2, "a new session, not a window in misc");
    assert_eq!(sessions[1].label, "repo");
}

#[test]
fn several_sessions_for_a_repo_are_told_apart_by_label_or_refused() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    fixture.rex.add_session("repo-old", &[&fixture.repo]);
    let labelled = fixture.rex.add_session("repo", &[&fixture.repo]);

    fixture
        .backend
        .open_checkout(&fixture.repo, &feature)
        .unwrap();
    assert_eq!(fixture.rex.change_names()[0], "session.new_window");
    assert_eq!(
        fixture.rex.changes()[0].1["session_id"],
        labelled.id.as_str()
    );

    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    fixture.rex.add_session("one", &[&fixture.repo]);
    fixture.rex.add_session("two", &[&fixture.repo]);

    let error = fixture
        .backend
        .open_checkout(&fixture.repo, &feature)
        .unwrap_err();

    assert_eq!(
        error,
        RexError::AmbiguousSession {
            repo: "repo".into(),
            labels: vec!["one".into(), "two".into()],
        }
    );
    assert!(
        error
            .to_string()
            .contains("label exactly one of them \"repo\"")
    );
    assert_eq!(fixture.rex.changes(), []);
}

#[test]
fn a_session_with_no_placeable_terminal_falls_back_to_its_label() {
    let fixture = Fixture::new();
    let session = fixture.rex.add_session("repo", &[&fixture.repo]);
    fixture
        .rex
        .edit(|state| state.sessions[0].windows[0].tiled[0].folder = None);

    fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(fixture.rex.changes()[0].0, "session.new_window");
    assert_eq!(
        fixture.rex.changes()[0].1["session_id"],
        session.id.as_str()
    );
}

#[test]
fn a_reported_directory_places_a_terminal_whose_process_cannot_be_read() {
    let fixture = Fixture::new();
    let session = fixture.rex.add_session("x", &[Path::new("/nowhere")]);
    let url = format!("file://mac.local{}", fixture.repo.display()).replace(' ', "%20");
    fixture.rex.edit(|state| {
        let block = &mut state.sessions[0].windows[0].tiled[0];
        block.folder = None;
        block.pwd = Some(url);
    });

    fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(
        fixture.rex.changes(),
        show_calls(&session.id, &session.windows[0].id)
    );
}

#[test]
fn floating_layers_and_other_machines_do_not_place_a_window() {
    let fixture = Fixture::new();
    let session = fixture.rex.add_session("x", &[&fixture.folder("notes")]);
    fixture.rex.edit(|state| {
        let mut floating = FakeRex::block(state, Some(&fixture.repo));
        floating.id = "block:floating".into();
        let mut remote = FakeRex::block(state, Some(&fixture.repo));
        remote.host = "host-remote".into();
        let window = &mut state.sessions[0].windows[0];
        window.floating.push(floating);
        window.tiled.insert(0, remote);
    });

    assert_eq!(fixture.checkout_window(&fixture.repo), None);
    let folders = fixture.backend.open_folders().unwrap();
    assert_eq!(folders, [fixture.folder("notes")]);
    let looked_up: Vec<Value> = fixture
        .rex
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(method, _)| method == "com.superlogical.terminal.process")
        .map(|(_, args)| args["block_id"].clone())
        .collect();
    assert_eq!(
        looked_up,
        [
            session.windows[0].tiled[0].id.as_str(),
            session.windows[0].tiled[0].id.as_str()
        ]
    );
}

#[test]
fn sessions_and_blocks_that_close_mid_listing_are_skipped() {
    let fixture = Fixture::new();
    let plain = fixture.folder("plain");
    let session = fixture.rex.add_session("kept", &[&fixture.repo, &plain]);
    fixture.rex.edit(|state| {
        state.vanishing.push(session.windows[0].tiled[0].id.clone());
        state.ghosts.push("session:ghost".into());
    });

    assert_eq!(fixture.backend.open_folders().unwrap(), [plain]);
}

#[test]
fn windows_report_their_repository_and_folders() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    let plain = fixture.folder("plain");
    fixture.rex.add_session("repo", &[&fixture.repo, &feature]);
    fixture.rex.add_session("plain", &[&plain]);

    let roots: Vec<_> = fixture
        .backend
        .open_repos()
        .unwrap()
        .iter()
        .map(|root| canonical(root))
        .collect();
    assert_eq!(roots, [fixture.repo.clone(), fixture.repo.clone()]);
    assert_eq!(
        fixture.backend.open_folders().unwrap(),
        [fixture.repo.clone(), feature, plain]
    );
}

#[test]
fn folders_open_as_a_session_with_one_window_and_are_selected_once_open() {
    let fixture = Fixture::with(editor_on_open(), Launch::default(), FakeRex::with_app());
    let plain = fixture.folder("plain");

    fixture.backend.open_folder(&plain).unwrap();
    fixture.backend.open_folder(&plain).unwrap();

    let sessions = fixture.rex.sessions();
    let [session] = sessions.as_slice() else {
        panic!("expected one session, got {sessions:?}");
    };
    assert_eq!(session.label, "plain");
    assert_eq!(session.windows.len(), 1);
    assert_eq!(
        session.windows[0].tiled.len(),
        1,
        "folders get no on_open panes"
    );
    let mut expected = vec![fixture.rex.changes()[0].clone()];
    expected.extend(show_calls(&session.id, &session.windows[0].id));
    expected.extend(show_calls(&session.id, &session.windows[0].id));
    assert_eq!(fixture.rex.changes(), expected);
}

#[test]
fn opening_a_branch_without_a_checkout_makes_one_in_the_worktree_directory() {
    let fixture = Fixture::new();
    run_test_git(&fixture.repo, &["branch", "existing"]);

    let event = fixture.run(|git, backend, sender| {
        spawn_open_branch(
            git,
            backend,
            sender,
            fixture.repo.clone(),
            "existing".into(),
            KnownCheckout::Missing,
        );
    });

    assert!(
        matches!(event, AppEvent::RepoOpened { warning: None }),
        "{event:?}"
    );
    let checkout = fixture.temp.path().join("worktrees/repo/existing");
    assert!(checkout.join("README.md").exists());
    assert!(fixture.checkout_window(&checkout).is_some());
}

#[test]
fn creating_a_branch_opens_its_new_checkout() {
    let fixture = Fixture::new();
    fixture.rex.add_session("repo", &[&fixture.repo]);

    let event = fixture.run(|git, backend, sender| {
        spawn_create_new_branch(
            git,
            backend,
            sender,
            fixture.repo.clone(),
            "Feature/Login_Page".into(),
            "HEAD".into(),
        );
    });

    assert!(
        matches!(event, AppEvent::RepoOpened { warning: None }),
        "{event:?}"
    );
    let checkout = fixture
        .temp
        .path()
        .join("worktrees/repo/feature-login-page");
    let sessions = fixture.rex.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].windows[1].label, "Feature/Login_Page");
    assert!(fixture.checkout_window(&checkout).is_some());
}

#[test]
fn removing_a_worktree_closes_its_window_and_an_emptied_session() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    let other = fixture.linked("other");
    fixture.rex.add_session("repo", &[&fixture.repo, &feature]);
    fixture.rex.add_session("lonely", &[&other]);

    assert_eq!(
        fixture.remove("feature", &feature, false),
        WorktreeRemovalOutcome::Removed { warning: None }
    );
    assert_eq!(
        fixture.remove("other", &other, false),
        WorktreeRemovalOutcome::Removed { warning: None }
    );

    assert!(!feature.exists() && !other.exists());
    let sessions = fixture.rex.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].windows.len(), 1);
    assert_eq!(
        fixture.rex.change_names(),
        [
            "session.close_window",
            "session.close_window",
            "session.destroy"
        ]
    );
}

#[test]
fn removing_a_dirty_worktree_needs_force_and_keeps_the_window() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    fs::write(feature.join("scratch.txt"), "unsaved").unwrap();
    fixture.rex.add_session("repo", &[&fixture.repo, &feature]);

    assert_eq!(
        fixture.remove("feature", &feature, false),
        WorktreeRemovalOutcome::DirtyRequiresForce
    );
    assert!(feature.exists());
    assert_eq!(fixture.rex.change_names(), Vec::<String>::new());

    assert_eq!(
        fixture.remove("feature", &feature, true),
        WorktreeRemovalOutcome::Removed { warning: None }
    );
    assert_eq!(fixture.rex.sessions()[0].windows.len(), 1);
}

#[test]
fn removing_the_checkout_the_picker_floats_over_leaves_the_window_to_close_with_it() {
    let (fixture, feature) = picker_over_feature(|fixture| vec![fixture.repo.clone()]);

    assert_eq!(
        fixture.remove("feature", &feature, false),
        WorktreeRemovalOutcome::Removed { warning: None }
    );

    let window = &fixture.rex.sessions()[0].windows[1];
    assert!(window.tiled.is_empty());
    assert_eq!(window.floating[0].id, "block:picker");
    assert_eq!(fixture.rex.change_names(), ["block.close"]);
    assert!(fixture.notices.lock().unwrap().is_empty());
}

/// A fixture whose picker floats over the window showing a new `feature` checkout, in a
/// session with a window for each of `folders` before it.
fn picker_over_feature(folders: impl FnOnce(&Fixture) -> Vec<PathBuf>) -> (Fixture, PathBuf) {
    let fixture = Fixture::with(
        OnOpenConfig::default(),
        Launch {
            client: None,
            picker_block: Some(BlockId::try_from("block:picker".to_string()).unwrap()),
        },
        FakeRex::with_app(),
    );
    let feature = fixture.linked("feature");
    let mut folders = folders(&fixture);
    folders.push(feature.clone());
    let folders: Vec<&Path> = folders.iter().map(PathBuf::as_path).collect();
    fixture.rex.add_session("repo", &folders);
    fixture.rex.edit(|state| {
        let mut picker = FakeRex::block(state, None);
        picker.id = "block:picker".into();
        state.sessions[0]
            .windows
            .last_mut()
            .unwrap()
            .floating
            .push(picker);
    });
    (fixture, feature)
}

#[test]
fn removing_the_checkout_of_the_pickers_only_window_ends_its_session_with_a_notice() {
    let (fixture, feature) = picker_over_feature(|_| Vec::new());
    let picker_session = fixture.rex.sessions()[0].id.clone();
    let other = fixture.rex.add_session("other", &[&fixture.repo]);

    assert_eq!(
        fixture.remove("feature", &feature, false),
        WorktreeRemovalOutcome::Removed { warning: None }
    );

    assert!(!feature.exists());
    let sessions = fixture.rex.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, other.id);
    let notice = format!(
        "\x1b]777;notify;rex-kiosk;Removed {} and closed the repo session, which had \
         nothing else open\x07",
        feature.display()
    );
    assert_eq!(
        fixture.rex.changes(),
        [
            ("forget".into(), feature.to_str().unwrap().into()),
            ("notice".into(), notice.into()),
            (
                "session.destroy".into(),
                json!({ "session_id": picker_session })
            ),
        ]
    );
}

#[test]
fn a_session_whose_removal_record_cannot_be_cleared_is_kept() {
    let (fixture, feature) = picker_over_feature(|_| Vec::new());
    fixture
        .rex
        .edit(|state| state.forget_error = Some("state file is malformed".into()));

    let WorktreeRemovalOutcome::Removed {
        warning: Some(warning),
    } = fixture.remove("feature", &feature, false)
    else {
        panic!("expected a removal with a warning");
    };

    assert!(!feature.exists());
    assert!(
        warning.contains("could not be closed in Rex")
            && warning.contains("could not clear the record of removing")
            && warning.contains("state file is malformed"),
        "{warning}"
    );
    assert_eq!(fixture.rex.sessions().len(), 1);
    assert_eq!(fixture.rex.change_names(), Vec::<String>::new());
    assert!(fixture.notices.lock().unwrap().is_empty());
}

#[test]
fn remote_control_being_off_says_how_to_turn_it_on() {
    let rex = FakeRex::with_app();
    rex.edit(|state| {
        state.perform_error = Some(RexError::Invocation(
            "rex do session.select exited with exit status: 1: Error: Rex.app is not \
             accepting actions. Turn on Remote Control in its Settings under Rex Server."
                .into(),
        ));
    });
    let fixture = Fixture::with(OnOpenConfig::default(), Launch::default(), rex);

    let error = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap_err();

    assert_eq!(error, RexError::RemoteControlOff);
    let message = error.to_string();
    assert!(
        message.contains("turn on Remote Control in Rex's Settings"),
        "{message}"
    );
    assert_eq!(fixture.rex.sessions().len(), 1, "the session is still made");
}

#[test]
fn the_launching_app_is_used_or_else_the_only_one_connected() {
    let rex = FakeRex::with_app();
    rex.edit(|state| state.apps.push(("client:second".into(), "iPad".into())));
    let fixture = Fixture::with(
        OnOpenConfig::default(),
        Launch {
            client: Some(ClientId::try_from("client:second".to_string()).unwrap()),
            picker_block: None,
        },
        rex,
    );
    fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();
    assert_eq!(fixture.rex.changes()[2].1["client"], "client:second");

    let rex = FakeRex::with_app();
    rex.edit(|state| state.apps.push(("client:second".into(), "iPad".into())));
    let fixture = Fixture::with(OnOpenConfig::default(), Launch::default(), rex);
    let error = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("2 Rex apps are connected (Mac, iPad)"),
        "{error}"
    );

    let fixture = Fixture::with(
        OnOpenConfig::default(),
        Launch::default(),
        FakeRex::default(),
    );
    let error = fixture
        .backend
        .open_folder(&fixture.folder("x"))
        .unwrap_err();
    assert!(
        error.to_string().contains("no Rex app is connected"),
        "{error}"
    );
}

#[test]
fn notifications_are_terminal_escapes_without_control_characters() {
    let fixture = Fixture::new();

    fixture
        .backend
        .notify("rex-kiosk", "on_open failed;\x1b]0;evil\x07 retry")
        .unwrap();

    assert_eq!(
        String::from_utf8(fixture.notices.lock().unwrap().clone()).unwrap(),
        "\x1b]777;notify;rex-kiosk;on_open failed; ]0;evil  retry\x07"
    );
    assert!(fixture.backend.notify(" ", "body").is_err());
}

#[test]
fn the_folder_a_block_is_in_comes_from_its_process() {
    let fixture = Fixture::new();
    let session = fixture.rex.add_session("x", &[&fixture.repo]);
    let block = BlockId::try_from(session.windows[0].tiled[0].id.clone()).unwrap();
    let session = SessionId::try_from(session.id).unwrap();

    assert_eq!(
        fixture.backend.block_folder(&session, &block).unwrap(),
        Some(fixture.repo.clone())
    );
}

#[test]
fn long_labels_are_cut_to_what_rex_accepts() {
    let long = "b".repeat(200);
    assert_eq!(truncate_label(&long).chars().count(), MAX_LABEL_CHARS);
    assert_eq!(truncate_label("main"), "main");
}
