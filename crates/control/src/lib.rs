pub mod command;
pub mod host;
pub mod protocol;
pub mod server;

pub use command::{CommandServer, RemoteCommand};
pub use host::{
    dispatch, host_command_from_name, host_command_help, host_command_names, HostCommand,
    HostCommandError, HostCommandInfo, HostResult, HostServices,
};
pub use protocol::{handle, help_response, run_host_command, QueryKind, Request, Response};
pub use server::ControlServer;

#[cfg(test)]
mod command_tests;

#[cfg(test)]
mod host_tests;

#[cfg(test)]
mod protocol_tests;

#[cfg(test)]
mod server_tests;
