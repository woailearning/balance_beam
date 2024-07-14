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

// #[derive(Parser, Debug)]
// #[command(about = "Fun with load balancing")]

/// Contains information parsed from the command-line invocation of balancebeam. The Clap macros
/// provide a fancy way to automatically construct a command-line argument parser.
#[derive(Parser)]
struct CmdOption {
    /// "IP/port to bind on"
    bind: String,

    /// "Upstream hsot to forward request to"
    upstream: Vec<String>,

    /// "Perform active health checks on this intervals (in seconds)"
    active_health_check_interval: usize,

    /// "Path to send request to for active health checks"
    active_health_check_path: String,

    /// "Maximum members of request"
    max_request_per_minute: usize,
}

#[derive(Clone)]
struct ProxyState {
    /// How franquly we check whether upstream server are alive
    active_health_check_interval: usize,

    /// Where we should send request when doing active health checks
    active_health_check_path: String,

    /// Maximum number of requests an individual IP can make it in a minute.
    max_requests_per_minutes: usize,

    /// Addresses of servers that we can proxying to
    upstream_addresses: Vec<String>,

    /// Addresses of servers that they are alive
    live_upstream_addresses: Arc<RwLock<Vec<String>>>,

    /// Rate limiting counter
    rate_limiting_counter: Arc<Mutex<HashMap<String, usize>>>,
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
    let options = CmdOption::parse();
    if options.upstream.len() < 1 {
        log::error!("At least one upstream server must be specified using the --upstream option.");
        std::process::exit(1);
    }

    // Start listening for connection
    let listener = match TcpListener::bind(&options.bind).await {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("Could not bind to {}:{}", options.bind, err);
            std::process::exit(1);
        }
    };

    log::info!("Listening for request on {}", options.bind);

    let state = ProxyState {
        live_upstream_addresses: Arc::new(RwLock::new(options.upstream.clone())),
        upstream_addresses: options.upstream,
        active_health_check_interval: options.active_health_check_interval,
        active_health_check_path: options.active_health_check_path,
        max_requests_per_minutes: options.max_request_per_minute,
        rate_limiting_counter: Arc::new(Mutex::new(HashMap::new())),
    };

    // start active health check
    let state_temp = state.clone();
    tokio::spawn( async move { 
        tokio::spawn( async move {
            active_health_check(&state).await
        })
     });

    // Start cleaning up rate limiting counter every minute
    let state_temp = state.clone();
    tokio::spawn( async move {
        rate_limiting_counter_clearer(&state, 60).await;
    });

    // Handle incoming connections
}