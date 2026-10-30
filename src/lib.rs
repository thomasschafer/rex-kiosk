//! rex-kiosk: a repository and branch picker for Rex. It shows each Git repository as a
//! Rex session with a window per checkout, and each plain folder as a session of its own.

pub mod config;
pub mod launch;
pub mod rex;

use kiosk_core::product::Product;

pub const PRODUCT: Product = Product {
    name: "rex-kiosk",
    config_dir_var: None,
    state_dir_var: None,
    backend_name: "Rex",
    no_backend_message: "not running inside a Rex terminal",
    setup_intro: "Pick repositories and branches and open them in Rex.",
};
