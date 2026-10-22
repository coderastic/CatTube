//! CatTube's logic, independent of the window: YouTube's TV interface, the
//! screens' state, the player (mpv) and the on-screen keyboard. Everything
//! here is unit-tested without a display or a network.

pub mod account;
pub mod community;
pub mod env;
pub mod error;
pub mod input;
pub mod lounge;
pub mod mpv;
pub mod net;
pub mod osk;
pub mod player;
pub mod process;
pub mod scan;
pub mod subs;
pub mod tube;
pub mod util;
pub mod watched;
pub mod youtube;

pub use error::{Error, Result};
