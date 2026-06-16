pub mod command;
pub mod host;
pub mod protocol;
pub mod server;

pub use command::{CommandServer, RemoteCommand};
pub use host::{
    host_command_from_name, host_command_names, host_help, HostCommand, HostCommandInfo, HostError,
    HostServices, SaveDest, SegmentSpec,
};
pub use protocol::{
    handle, handle_run_host_command, handle_with_host, QueryKind, Request, Response,
};
pub use server::ControlServer;

#[cfg(test)]
mod command_tests;

#[cfg(test)]
mod protocol_tests;

#[cfg(test)]
mod server_tests;
