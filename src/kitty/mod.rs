//! Talking to kitty: its remote control protocol, and finding its sockets.

pub mod discover;
pub mod rc;

pub use rc::{Addr, Client};
