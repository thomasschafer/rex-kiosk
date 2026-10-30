//! Rex as a kiosk backend. Rex has no worktrees of its own, so kiosk does the Git work,
//! and shows each repository as one Rex session with a window (tab) per checkout. A plain
//! folder gets a session with one window.
//!
//! Sessions and windows carry only labels, which the user can change, so kiosk goes by
//! where terminals are. A window shows a checkout when its first terminal's folder is
//! inside it. A session belongs to the repository its first such window is in; only a
//! session with no terminal kiosk can place falls back to its label.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use kiosk_core::{
    backend::{
        Backend, CheckoutState, ManagedWorktrees, Opened, WorktreePlacement, Worktrees,
        identity::{session_in_folder, session_showing_checkout},
    },
    git::{GitProvider, Worktree},
    path::{canonical_or_original, equivalent},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    BlockId, ClientId, RexApi, RexError, SessionId, WindowId, WindowRef, decode,
    layout::Layout,
    live::{self, Live, LiveSession, LiveWindow},
    path_arg,
};
use crate::config::OnOpenConfig;

/// The longest label Rex accepts for a session or window, in characters.
const MAX_LABEL_CHARS: usize = 128;
const APP_CLIENT: &str = "app";

/// How a picker started in Rex was launched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    /// The Rex app to show what kiosk opens in. Without one, kiosk uses the only app
    /// connected to the server.
    pub client: Option<ClientId>,
    /// The block the picker runs in, so kiosk never closes it from under itself.
    pub picker_block: Option<BlockId>,
}

pub struct Rex {
    api: Arc<dyn RexApi>,
    git: Arc<dyn GitProvider>,
    placement: WorktreePlacement,
    on_open: OnOpenConfig,
    launch: Launch,
    /// Where notifications are written, as terminal escape sequences: the picker's own
    /// terminal, which passes them to the Rex app.
    notices: Mutex<Box<dyn Write + Send>>,
    host_id: OnceLock<String>,
}

#[derive(Deserialize)]
struct Created {
    session_id: SessionId,
    initial_windows: Vec<CreatedWindow>,
}

#[derive(Deserialize)]
struct CreatedWindow {
    window_id: WindowId,
}

#[derive(Deserialize)]
struct NewWindow {
    window_id: WindowId,
}

#[derive(Deserialize)]
struct ClientList {
    clients: Vec<ClientEntry>,
}

#[derive(Deserialize)]
struct ClientEntry {
    client_id: ClientId,
    #[serde(default)]
    info: Option<ClientInfo>,
}

#[derive(Deserialize)]
struct ClientInfo {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
struct SessionWindows {
    #[serde(default)]
    windows: Option<Vec<Value>>,
}

impl Rex {
    pub fn new(
        api: Arc<dyn RexApi>,
        git: Arc<dyn GitProvider>,
        placement: WorktreePlacement,
        on_open: OnOpenConfig,
        launch: Launch,
        notices: Box<dyn Write + Send>,
    ) -> Self {
        Self {
            api,
            git,
            placement,
            on_open,
            launch,
            notices: Mutex::new(notices),
            host_id: OnceLock::new(),
        }
    }

    fn call(&self, method: &str, args: &Value) -> Result<Value, RexError> {
        self.api.call(method, args)
    }

    fn live(&self) -> Result<Live, RexError> {
        let host_id = if let Some(host_id) = self.host_id.get() {
            host_id
        } else {
            let host_id = live::host_id(self.api.as_ref())?;
            self.host_id.get_or_init(|| host_id)
        };
        live::read(self.api.as_ref(), host_id)
    }

    /// The folder `block` of `session` is in, for preselecting where the user was.
    pub fn block_folder(
        &self,
        session: &SessionId,
        block: &BlockId,
    ) -> Result<Option<PathBuf>, RexError> {
        live::block_folder(self.api.as_ref(), session, block)
    }

    fn worktrees_of(&self, repo: &Path) -> Result<Vec<Worktree>, RexError> {
        path_arg("repository", repo)?;
        self.git
            .list_worktrees(repo)
            .map(|listed| listed.items)
            .map_err(|error| git_error(&error))
    }

    /// Makes `target` the active window of its session and shows it in the Rex app.
    fn show(&self, target: &WindowRef) -> Result<(), RexError> {
        let ids = json!({ "session_id": target.session, "window_id": target.window });
        self.call("session.focus_window", &ids)?;
        match self.api.perform(&self.client()?, "session.select", &ids) {
            Err(error) if error.to_string().contains("Remote Control") => {
                Err(RexError::RemoteControlOff)
            }
            performed => performed,
        }
    }

    fn client(&self) -> Result<ClientId, RexError> {
        if let Some(client) = &self.launch.client {
            return Ok(client.clone());
        }
        let list: ClientList = decode("client.list", self.call("client.list", &json!({}))?)?;
        let apps: Vec<ClientEntry> = list
            .clients
            .into_iter()
            .filter(|client| {
                client.info.as_ref().and_then(|info| info.kind.as_deref()) == Some(APP_CLIENT)
            })
            .collect();
        match <[ClientEntry; 1]>::try_from(apps) {
            Ok([app]) => Ok(app.client_id),
            Err(apps) if apps.is_empty() => Err(RexError::InvalidArgument(
                "no Rex app is connected to the server to show it in".into(),
            )),
            Err(apps) => Err(RexError::InvalidArgument(format!(
                "{} Rex apps are connected ({}); open kiosk from a Rex action, which says \
                 which app to show it in",
                apps.len(),
                apps.iter()
                    .map(|app| {
                        app.info
                            .as_ref()
                            .and_then(|info| info.label.clone())
                            .unwrap_or_else(|| app.client_id.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// The session new windows for `repo` go in, if it has one.
    fn repo_session<'a>(
        &self,
        live: &'a Live,
        repo: &Path,
        label: &str,
    ) -> Result<Option<&'a LiveSession>, RexError> {
        let repo = canonical_or_original(repo);
        let mut by_folder = Vec::new();
        let mut by_label = Vec::new();
        for session in &live.sessions {
            match session.windows.iter().find_map(LiveWindow::place) {
                Some(place) => {
                    let main = self
                        .git
                        .main_checkout(&place)
                        .map_err(|error| git_error(&error))?;
                    if main.is_some_and(|main| equivalent(&canonical_or_original(&main), &repo)) {
                        by_folder.push(session);
                    }
                }
                None if session.label == label => by_label.push(session),
                None => {}
            }
        }
        let candidates = if by_folder.is_empty() {
            by_label
        } else if by_folder.len() > 1 {
            let labelled: Vec<_> = by_folder
                .iter()
                .copied()
                .filter(|session| session.label == label)
                .collect();
            if labelled.len() == 1 {
                labelled
            } else {
                by_folder
            }
        } else {
            by_folder
        };
        match candidates.as_slice() {
            [] => Ok(None),
            [session] => Ok(Some(session)),
            several => Err(RexError::AmbiguousSession {
                repo: label.into(),
                labels: several
                    .iter()
                    .map(|session| session.label.clone())
                    .collect(),
            }),
        }
    }

    fn new_window(
        &self,
        session: &SessionId,
        label: &str,
        layout: &Layout,
    ) -> Result<WindowRef, RexError> {
        let created: NewWindow = decode(
            "session.new_window",
            self.call(
                "session.new_window",
                &json!({
                    "session_id": session,
                    "window_label": truncate_label(label),
                    "layout": layout,
                }),
            )?,
        )?;
        Ok(WindowRef {
            session: session.clone(),
            window: created.window_id,
        })
    }

    /// Creates a session with one window, all at once, so no app sees it half made.
    fn new_session(
        &self,
        label: &str,
        window_label: &str,
        layout: &Layout,
    ) -> Result<WindowRef, RexError> {
        let created: Created = decode(
            "session.create",
            self.call(
                "session.create",
                &json!({
                    "label": truncate_label(label),
                    "initial_windows": [{
                        "window_label": truncate_label(window_label),
                        "layout": layout,
                    }],
                }),
            )?,
        )?;
        let [window] =
            <[CreatedWindow; 1]>::try_from(created.initial_windows).map_err(|windows| {
                RexError::InvalidResponse(format!(
                    "session.create made {} windows, not the one asked for",
                    windows.len()
                ))
            })?;
        Ok(WindowRef {
            session: created.session_id,
            window: window.window_id,
        })
    }
}

impl Backend for Rex {
    type SessionId = WindowRef;
    type Error = RexError;

    fn open_repos(&self) -> Result<Vec<PathBuf>, RexError> {
        self.live()?
            .window_places()
            .into_iter()
            .filter_map(|place| {
                self.git
                    .main_checkout(&place.folder)
                    .map_err(|error| git_error(&error))
                    .transpose()
            })
            .collect()
    }

    fn open_folders(&self) -> Result<Vec<PathBuf>, RexError> {
        Ok(self
            .live()?
            .terminal_folders()
            .into_iter()
            .map(|terminal| terminal.folder)
            .collect())
    }

    fn checkouts(&self, repo: &Path) -> Result<Vec<CheckoutState<WindowRef>>, RexError> {
        let paths: Vec<PathBuf> = self
            .worktrees_of(repo)?
            .into_iter()
            .map(|worktree| worktree.path)
            .collect();
        let places = self.live()?.window_places();
        Ok(paths
            .iter()
            .map(|path| CheckoutState {
                path: path.clone(),
                session: session_showing_checkout(&places, path, &paths).cloned(),
            })
            .collect())
    }

    fn open_checkout(&self, repo: &Path, checkout: &Path) -> Result<Opened, RexError> {
        path_arg("checkout", checkout)?;
        let worktrees = self.worktrees_of(repo)?;
        let paths: Vec<PathBuf> = worktrees
            .iter()
            .map(|worktree| worktree.path.clone())
            .collect();
        let live = self.live()?;
        if let Some(window) = session_showing_checkout(&live.window_places(), checkout, &paths) {
            self.show(window)?;
            return Ok(Opened::default());
        }
        let canonical = canonical_or_original(checkout);
        let worktree = worktrees
            .iter()
            .find(|worktree| equivalent(&canonical_or_original(&worktree.path), &canonical))
            .ok_or_else(|| {
                RexError::Git(format!(
                    "{} is not a checkout of {}",
                    kiosk_core::path::display(checkout),
                    kiosk_core::path::display(repo)
                ))
            })?;
        let repo_label = folder_label(repo)?;
        let window_label = match &worktree.branch {
            Some(branch) => branch.clone(),
            None => folder_label(&worktree.path)?,
        };
        let layout = Layout::checkout(&path_arg("checkout", &worktree.path)?, &self.on_open);
        let window = match self.repo_session(&live, repo, &repo_label)? {
            Some(session) => self.new_window(&session.id, &window_label, &layout)?,
            None => self.new_session(&repo_label, &window_label, &layout)?,
        };
        self.show(&window)?;
        Ok(Opened::default())
    }

    fn open_folder(&self, folder: &Path) -> Result<Opened, RexError> {
        let cwd = path_arg("folder", folder)?;
        let terminals = self.live()?.terminal_folders();
        let window = if let Some(window) = session_in_folder(&terminals, folder) {
            window.clone()
        } else {
            let label = folder_label(folder)?;
            self.new_session(&label, &label, &Layout::folder(&cwd))?
        };
        self.show(&window)?;
        Ok(Opened::default())
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), RexError> {
        let title = notice_text(title).replace(';', ",");
        let body = notice_text(body);
        if title.is_empty() || body.is_empty() {
            return Err(RexError::InvalidArgument(
                "a notification needs a title and a body".into(),
            ));
        }
        let mut notices = self
            .notices
            .lock()
            .expect("the notice writer is not poisoned");
        write!(notices, "\x1b]777;notify;{title};{body}\x07")
            .and_then(|()| notices.flush())
            .map_err(|error| {
                RexError::Invocation(format!("could not write a notification: {error}"))
            })
    }

    fn worktrees(&self) -> Worktrees<'_, Self> {
        Worktrees::Managed(self)
    }
}

impl ManagedWorktrees<Self> for Rex {
    fn placement(&self) -> &WorktreePlacement {
        &self.placement
    }

    /// Closes the window, and its session if that leaves it empty. A window the picker
    /// is floating over keeps only the picker, and closes when the picker exits.
    fn close(&self, target: &WindowRef) -> Result<(), RexError> {
        let live = self.live()?;
        let Some(window) = live.session(&target.session).and_then(|session| {
            session
                .windows
                .iter()
                .find(|window| window.id == target.window)
        }) else {
            return Ok(());
        };
        if let Some(picker) = &self.launch.picker_block
            && window.blocks.contains(picker)
        {
            for block in window.blocks.iter().filter(|block| *block != picker) {
                self.call(
                    "block.close",
                    &json!({ "session_id": target.session, "block_id": block }),
                )?;
            }
            return Ok(());
        }
        self.call(
            "session.close_window",
            &json!({ "session_id": target.session, "window_id": target.window }),
        )?;
        let remaining: SessionWindows = decode(
            "session.view",
            self.call("session.view", &json!({ "session_id": target.session }))?,
        )?;
        if remaining.windows.unwrap_or_default().is_empty() {
            self.call("session.destroy", &json!({ "session_id": target.session }))?;
        }
        Ok(())
    }
}

/// A label naming `path` by its last component.
fn folder_label(path: &Path) -> Result<String, RexError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            RexError::InvalidArgument(format!(
                "{} has no folder name to label it with",
                path.display()
            ))
        })
}

fn truncate_label(label: &str) -> String {
    label.chars().take(MAX_LABEL_CHARS).collect()
}

/// `text` without control characters, which would end or corrupt the escape sequence.
fn notice_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn git_error(error: &anyhow::Error) -> RexError {
    RexError::Git(format!("{error:#}"))
}

#[cfg(test)]
mod tests;
