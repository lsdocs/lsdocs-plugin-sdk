//! An echo peer for testing transports.
//!
//! ```text
//! echo stdio   # talk over stdin/stdout
//! echo pipe    # connect to the pipe named in LSDOCS_PIPE
//! ```
//!
//! It prints a line to stdout after taking the transport. With `stdio`, that
//! line must end up on stderr rather than in the protocol stream.

mod router;

use std::process::ExitCode;

use lsdocs_rpc::{Options, connect, transport};

/// Printed after the transport is set up; see the module docs.
const STRAY_LINE: &str = "echo: stray stdout line";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let transport = match mode.as_str() {
        "stdio" => transport::stdio(),
        "pipe" => transport::pipe::connect_from_env().await,
        _ => {
            eprintln!("usage: echo <stdio|pipe>");
            return ExitCode::FAILURE;
        }
    };
    let transport = match transport {
        Ok(t) => t,
        Err(e) => {
            eprintln!("echo: failed to open transport: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{STRAY_LINE}");

    let (_peer, connection) = connect(transport, router::router(None), Options::default());
    match connection.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("echo: connection failed: {e}");
            ExitCode::FAILURE
        }
    }
}
