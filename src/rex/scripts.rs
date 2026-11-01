//! The Lua scripts kiosk runs with `rex do`. Each does in one connection what would
//! otherwise take a `rex` process per request, and each starts with the helpers in
//! `common.lua`.

/// A Lua script for `rex do`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Script {
    /// What the script is called in errors.
    pub name: &'static str,
    pub source: &'static str,
}

macro_rules! script {
    ($name:literal) => {
        Script {
            name: $name,
            source: concat!(
                include_str!("scripts/common.lua"),
                include_str!(concat!("scripts/", $name, ".lua"))
            ),
        }
    };
}

/// Every session's windows, their blocks, and the folders of their terminals.
pub(crate) const SNAPSHOT: Script = script!("snapshot");
/// What one terminal knows of its folder.
pub(crate) const FOLDER: Script = script!("folder");
/// Makes a window if asked to, and shows it in the app.
pub(crate) const OPEN: Script = script!("open");
