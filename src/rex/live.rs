//! What Rex is showing right now: every session's windows, and the folder each of their
//! terminals is in. Sessions and windows have only user-editable labels, so kiosk tells
//! what a window shows by where its terminals are.

use std::path::PathBuf;

use kiosk_core::backend::identity::LiveFolder;
use serde::Deserialize;
use serde_json::json;

use super::{BlockId, RexApi, RexError, SessionId, WindowId, WindowRef, decode};

const TERMINAL_CREATOR: &str = "com.superlogical.terminal";
const TILED_LAYER: &str = "tiled";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Live {
    pub sessions: Vec<LiveSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveSession {
    pub id: SessionId,
    pub label: String,
    pub windows: Vec<LiveWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveWindow {
    pub id: WindowId,
    /// The folder of each terminal in the window's layout on this machine, in layout
    /// order, where Rex can tell. Floating layers are left out: they come and go over the
    /// window, as the picker itself does.
    pub folders: Vec<Option<PathBuf>>,
    /// Every block in the window, floating ones included.
    pub blocks: Vec<BlockId>,
}

impl LiveWindow {
    /// Where the window is: the folder of its first terminal with a known one.
    pub fn place(&self) -> Option<PathBuf> {
        self.folders
            .iter()
            .flatten()
            .next()
            .map(|folder| kiosk_core::path::canonical_or_original(folder))
    }
}

impl LiveSession {
    pub fn window_ref(&self, window: &LiveWindow) -> WindowRef {
        WindowRef {
            session: self.id.clone(),
            window: window.id.clone(),
        }
    }
}

impl Live {
    pub fn windows(&self) -> impl Iterator<Item = (&LiveSession, &LiveWindow)> {
        self.sessions
            .iter()
            .flat_map(|session| session.windows.iter().map(move |window| (session, window)))
    }

    /// Each window with a known place, and that place.
    pub fn window_places(&self) -> Vec<LiveFolder<WindowRef>> {
        self.windows()
            .filter_map(|(session, window)| {
                Some(LiveFolder {
                    session: session.window_ref(window),
                    folder: window.place()?,
                })
            })
            .collect()
    }

    /// Every terminal's known folder, with the window it is in.
    pub fn terminal_folders(&self) -> Vec<LiveFolder<WindowRef>> {
        self.windows()
            .flat_map(|(session, window)| {
                window.folders.iter().flatten().map(|folder| LiveFolder {
                    session: session.window_ref(window),
                    folder: folder.clone(),
                })
            })
            .collect()
    }

    pub fn session(&self, id: &SessionId) -> Option<&LiveSession> {
        self.sessions.iter().find(|session| &session.id == id)
    }
}

#[derive(Deserialize)]
struct SessionList {
    sessions: Vec<SessionEntry>,
}

#[derive(Deserialize)]
struct SessionEntry {
    session_id: SessionId,
}

#[derive(Deserialize)]
struct Status {
    host_id: String,
}

#[derive(Deserialize)]
struct View {
    label: String,
    #[serde(default)]
    windows: Option<Vec<ViewWindow>>,
}

#[derive(Deserialize)]
struct ViewWindow {
    window_id: WindowId,
    #[serde(default)]
    layers: Option<Vec<ViewLayer>>,
}

#[derive(Deserialize)]
struct ViewLayer {
    kind: String,
    #[serde(default)]
    blocks: Option<Vec<ViewBlock>>,
}

#[derive(Deserialize)]
struct ViewBlock {
    block_id: BlockId,
    #[serde(default)]
    creator_name: Option<String>,
    #[serde(default)]
    host_id: Option<String>,
}

#[derive(Deserialize)]
struct Process {
    #[serde(default)]
    foreground: Option<ProcessInfo>,
}

#[derive(Deserialize)]
struct ProcessInfo {
    #[serde(default)]
    cwd: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Pwd {
    #[serde(default)]
    pwd: Option<String>,
}

/// A window's blocks from a view, before their folders are looked up.
struct ViewedWindow {
    id: WindowId,
    terminals: Vec<BlockId>,
    blocks: Vec<BlockId>,
}

struct ViewedSession {
    id: SessionId,
    label: String,
    windows: Vec<ViewedWindow>,
}

/// The server's host ID, which blocks on this machine share.
pub(crate) fn host_id(api: &dyn RexApi) -> Result<String, RexError> {
    let status: Status = decode("server.status", api.call("server.status", &json!({}))?)?;
    Ok(status.host_id)
}

/// Reads every session, then every terminal's folder, each step all at once: one after
/// another, the calls add up to most of the time an open takes.
pub(crate) fn read(api: &dyn RexApi, host_id: &str) -> Result<Live, RexError> {
    let list: SessionList = decode("session.list", api.call("session.list", &json!({}))?)?;
    let viewed: Vec<ViewedSession> = in_parallel(&list.sessions, |entry| {
        view(api, &entry.session_id, host_id)
    })?
    .into_iter()
    .flatten()
    .collect();

    let terminals: Vec<(&SessionId, &BlockId)> = viewed
        .iter()
        .flat_map(|session| {
            session
                .windows
                .iter()
                .flat_map(|window| window.terminals.iter().map(|block| (&session.id, block)))
        })
        .collect();
    let mut folders = in_parallel(&terminals, |(session, block)| {
        block_folder(api, session, block)
    })?
    .into_iter();

    Ok(Live {
        sessions: viewed
            .into_iter()
            .map(|session| LiveSession {
                id: session.id,
                label: session.label,
                windows: session
                    .windows
                    .into_iter()
                    .map(|window| LiveWindow {
                        id: window.id,
                        folders: window
                            .terminals
                            .iter()
                            .map(|_| folders.next().expect("a folder for every terminal"))
                            .collect(),
                        blocks: window.blocks,
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// A session's windows, or `None` if it closed since it was listed.
fn view(
    api: &dyn RexApi,
    session: &SessionId,
    host_id: &str,
) -> Result<Option<ViewedSession>, RexError> {
    let view: View = match api.call("session.view", &json!({ "session_id": session })) {
        Ok(view) => decode("session.view", view)?,
        Err(error) if error.code() == Some("session_not_found") => return Ok(None),
        Err(error) => return Err(error),
    };
    let windows = view
        .windows
        .unwrap_or_default()
        .into_iter()
        .map(|window| {
            let layers = window.layers.unwrap_or_default();
            let terminals = layers
                .iter()
                .filter(|layer| layer.kind == TILED_LAYER)
                .flat_map(|layer| layer.blocks.iter().flatten())
                .filter(|block| {
                    block.creator_name.as_deref() == Some(TERMINAL_CREATOR)
                        && block.host_id.as_deref() == Some(host_id)
                })
                .map(|block| block.block_id.clone())
                .collect();
            let blocks = layers
                .iter()
                .flat_map(|layer| layer.blocks.iter().flatten())
                .map(|block| block.block_id.clone())
                .collect();
            ViewedWindow {
                id: window.window_id,
                terminals,
                blocks,
            }
        })
        .collect();
    Ok(Some(ViewedSession {
        id: session.clone(),
        label: view.label,
        windows,
    }))
}

/// Where a terminal is: its foreground process's folder, or else the folder its shell
/// last reported. `None` if neither is known or the block has closed.
pub(crate) fn block_folder(
    api: &dyn RexApi,
    session: &SessionId,
    block: &BlockId,
) -> Result<Option<PathBuf>, RexError> {
    let args = json!({ "session_id": session, "block_id": block, "args": {} });
    let process: Process = match api.call("com.superlogical.terminal.process", &args) {
        Ok(process) => decode("com.superlogical.terminal.process", process)?,
        Err(error) if error.code() == Some("not_found") => return Ok(None),
        Err(error) => return Err(error),
    };
    if let Some(cwd) = process.foreground.and_then(|foreground| foreground.cwd) {
        return Ok(Some(cwd));
    }
    let pwd: Pwd = match api.call("com.superlogical.terminal.pwd", &args) {
        Ok(pwd) => decode("com.superlogical.terminal.pwd", pwd)?,
        Err(error) if error.code() == Some("not_found") => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(pwd.pwd.as_deref().and_then(reported_path))
}

/// The path in a reported working directory: a `file://` URL from OSC 7, or a plain path.
fn reported_path(pwd: &str) -> Option<PathBuf> {
    let Some(rest) = pwd.strip_prefix("file://") else {
        return Some(PathBuf::from(pwd)).filter(|path| path.is_absolute());
    };
    // The host comes before the path; this terminal runs on the server's machine.
    let path = &rest[rest.find('/')?..];
    percent_decode(path).map(PathBuf::from)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// Runs `work` on every item at once and collects the results in order.
fn in_parallel<T: Sync, R: Send>(
    items: &[T],
    work: impl Fn(&T) -> Result<R, RexError> + Sync,
) -> Result<Vec<R>, RexError> {
    std::thread::scope(|scope| {
        let work = &work;
        let running: Vec<_> = items
            .iter()
            .map(|item| scope.spawn(move || work(item)))
            .collect();
        running
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_directories_can_be_file_urls_or_paths() {
        assert_eq!(
            reported_path("file://mac.local/Users/me/My%20Repo"),
            Some(PathBuf::from("/Users/me/My Repo"))
        );
        assert_eq!(
            reported_path("file:///tmp/x"),
            Some(PathBuf::from("/tmp/x"))
        );
        assert_eq!(reported_path("/srv/app"), Some(PathBuf::from("/srv/app")));
        for unusable in [
            "relative/dir",
            "file://host",
            "file:///bad%zz",
            "file:///%ff",
        ] {
            assert_eq!(reported_path(unusable), None, "{unusable}");
        }
    }
}
