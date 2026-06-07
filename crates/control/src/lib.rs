pub mod command;
pub mod protocol;
pub mod server;

pub use command::{CommandServer, RemoteCommand};
pub use protocol::{handle, Request, Response};
pub use server::ControlServer;

#[cfg(test)]
mod command_tests;

#[cfg(test)]
mod protocol_tests;

#[cfg(test)]
mod server_tests;
