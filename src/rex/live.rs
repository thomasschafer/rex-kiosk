//! What Rex is showing right now: every session's windows, and the folder each of their
//! terminals is in. Sessions and windows have only user-editable labels, so kiosk tells
//! what a window shows by where its terminals are.

use std::{fmt, marker::PhantomData, path::PathBuf};

use kiosk_core::backend::identity::LiveFolder;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::json;

use super::{BlockId, RexApi, RexError, SessionId, WindowId, WindowRef, decode, scripts};

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
struct Snapshot {
    #[serde(deserialize_with = "lua_list")]
    sessions: Vec<SnapshotSession>,
}

#[derive(Deserialize)]
struct SnapshotSession {
    session_id: SessionId,
    label: String,
    #[serde(deserialize_with = "lua_list")]
    windows: Vec<SnapshotWindow>,
}

#[derive(Deserialize)]
struct SnapshotWindow {
    window_id: WindowId,
    #[serde(deserialize_with = "lua_list")]
    blocks: Vec<BlockId>,
    #[serde(deserialize_with = "lua_list")]
    terminals: Vec<TerminalFolder>,
}

/// What a terminal knows of its folder: its foreground process's, or else the one its
/// shell last reported.
#[derive(Deserialize)]
struct TerminalFolder {
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    pwd: Option<String>,
}

impl TerminalFolder {
    fn folder(self) -> Option<PathBuf> {
        self.cwd
            .or_else(|| self.pwd.as_deref().and_then(reported_path))
    }
}

/// Reads every session, its windows, and where their terminals are, all in one script.
pub(crate) fn read(api: &dyn RexApi) -> Result<Live, RexError> {
    let snapshot: Snapshot = decode(
        scripts::SNAPSHOT.name,
        api.script(&scripts::SNAPSHOT, &json!({}))?,
    )?;
    Ok(Live {
        sessions: snapshot
            .sessions
            .into_iter()
            .map(|session| LiveSession {
                id: session.session_id,
                label: session.label,
                windows: session
                    .windows
                    .into_iter()
                    .map(|window| LiveWindow {
                        id: window.window_id,
                        folders: window
                            .terminals
                            .into_iter()
                            .map(TerminalFolder::folder)
                            .collect(),
                        blocks: window.blocks,
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// Where a terminal is: its foreground process's folder, or else the folder its shell
/// last reported. `None` if neither is known or the block has closed.
pub(crate) fn block_folder(
    api: &dyn RexApi,
    session: &SessionId,
    block: &BlockId,
) -> Result<Option<PathBuf>, RexError> {
    let folder: TerminalFolder = decode(
        scripts::FOLDER.name,
        api.script(
            &scripts::FOLDER,
            &json!({ "session_id": session, "block_id": block }),
        )?,
    )?;
    Ok(folder.folder())
}

/// A list a script returned. Lua cannot tell an empty list from an empty table, which
/// Rex turns into an empty object, so that is read as an empty list too.
fn lua_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct ListVisitor<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for ListVisitor<T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a list, or an empty object for an empty list")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(item) = seq.next_element()? {
                items.push(item);
            }
            Ok(items)
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
            match map.next_key::<String>()? {
                None => Ok(Vec::new()),
                Some(key) => Err(de::Error::custom(format!(
                    "expected a list, found an object with the key {key:?}"
                ))),
            }
        }
    }

    deserializer.deserialize_any(ListVisitor(PhantomData))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_lists_may_be_empty_objects_but_not_other_objects() {
        #[derive(Debug, Deserialize)]
        struct Holder {
            #[serde(deserialize_with = "lua_list")]
            items: Vec<u8>,
        }
        let read = |value: serde_json::Value| serde_json::from_value::<Holder>(value);

        assert_eq!(read(json!({ "items": [1, 2] })).unwrap().items, [1, 2]);
        assert_eq!(read(json!({ "items": {} })).unwrap().items, [0u8; 0]);
        let error = read(json!({ "items": { "a": 1 } })).unwrap_err();
        assert!(error.to_string().contains("found an object"), "{error}");
        assert!(read(json!({ "items": "x" })).is_err());
    }

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
