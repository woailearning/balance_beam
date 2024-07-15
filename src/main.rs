use clap::Parser;
use std::{collections::HashMap, sync::Arc, time::Duration, io::{Error, ErrorKind}};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock},
    time::sleep,
    io::{AsyncReadExt, AsyncWriteExt},
};

mod response;
mod request;
mod proxy;

use proxy::ProxyState;

// #[derive(Parser, Debug)]
// #[command(about = "Fun with load balancing")]
/// Contains information parsed from the command-line invocation of balancebeam. The Clap macros
/// provide a fancy way to automatically construct a command-line argument parser.
#[derive(Parser)]
pub struct CmdOptions {
    /// "IP/port to bind on"
    bind: String,

    /// "Upstream hsot to forward request to"
    upstream: Vec<String>,

    /// "Perform active health checks on this intervals (in seconds)"
    active_health_check_interval: usize,

    /// "Path to send request to for active health checks"
    active_health_check_path: String,

    /// "Maximum members of request"
    max_requests_per_minute: usize,
}

#[tokio::main]
async fn main() {
    // Initialize the logging library. You can print log messages using the `log` macros:
    // https://docs.rs/log/0.4.8/log/ You are welcome to continue using print! statements; this
    // just looks a little prettier.
    if let Err(_) = std::env::var("RUST_LOG") {
        std::env::set_var("RUST_LOG", "debug");
    }
    pretty_env_logger::init();

    // Parse the command line arguments passed to this program
    let options = CmdOptions::parse();
    if options.upstream.len() < 1 {
        log::error!("At least one upstream server must be specified using the --upstream option.");
        std::process::exit(1);
    }

    log::info!("Listening for request on {}", options.bind);

    // Start listening for connection
    let listener = match TcpListener::bind(&options.bind).await {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("Could not bind to {}:{}", options.bind, err);
            std::process::exit(1);
        }
    };

    let state = ProxyState::new(options);

    // start active health check
    let state_temp = state.clone();
    tokio::spawn( async move { 
        proxy::active_health_check(&state_temp).await
     });

    // Start cleaning up rate limiting counter every minute
    let state_temp = state.clone();
    tokio::spawn( async move {
        proxy::rate_limiting_counter_clearer(&state_temp, 60).await
    });

 
    // Handle incoming connections
    loop {
        if let Ok((stream, _)) = listener.accept().await {
            let state: proxy::ProxyState = state.clone();
            // new tokio task
            tokio::spawn(async move {
                proxy::handle_connection(stream, &state).await;
            });
        }
    }

}