use clap::Parser;
use std::{collections::HashMap, sync::Arc, time::Duration, io::{Error, ErrorKind}};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock},
    time::sleep,
    io::{AsyncReadExt, AsyncWriteExt},
};

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

/// # Brief
/// This function performs on active health check on the upstream servers periodically.
/// It sends requests to each upstream server and updates the list of live upstream addresses
/// based on their responses.
/// 
/// # Param
/// -`state`: A reference to the `ProxyState` which holds the configuration and state.
/// of the proxy, including the interval for health checks, the list of upstream addresses,
/// and the path for health check requests.
/// 
/// # Return
/// This function does not return a vlaue, It runs indefinitely, performing health checks 
///  at the specified interval.
/// 
async fn active_health_check(state: &ProxyState) {
    loop {
        sleep(Duration::from_secs(
            state.active_health_check_interval.try_into().unwrap(),
        ))
        .await;

        let mut live_upstream_addresses = state.live_upstream_addresses.write().await;
        live_upstream_addresses.clear();

        // send a request to each upstream
        // If a failed upstream return HTTP 200, put it back in the roation of upstream servers
        // If an online upstream returns a not-200 status code, mark that server as failed
        for upstream_ip in &state.upstream_addresses {
            let request: http::Request<Vec<u8>> = http::Request::builder()
                .method(http::Method::GET)
                .uri(&state.active_health_check_path)
                .header("Host", upstream_ip)
                .body(Vec::<u8>::new())
            .unwrap();

            // Open a connection to a destination server
            match TcpStream::connect(upstream_ip).await {
                Ok( mut conn ) => {
                    // Write to stream and read from stream
                    if let Err(error) = request::write_to_stream(&request, &mut conn).await {
                        log::error!(
                            "Failed to send request to upstream {}: {}",
                            upstream_ip,
                            error
                        );
                        return ;
                    }

                    let response = match response::read_from_stream(&mut conn, &request.method()).await {
                        Ok(response) => response,
                        Err(error) => {
                            log::error!("Error reading response from server: {:?}", error);
                            return;
                        }
                    };

                    /// Handle the statusCode of response
                    match response.staus().as_u16() {
                        200 => {
                            live_upstream_addresses.push(upstream_ip.clone());
                        },
                        status @ _ => {
                            log::error!(
                                "upstream server {} is not working: {}",
                                upstream_ip,
                                status
                            );
                            return;
                        }
                    };
                },
                Err(err) => {
                    log::error!("Failed to connect to upstream {}: {}", upstream_ip, err);
                    return;
                }
            }
        }
    }
}

/// # Brief
/// This asynchronous function checks if the rate limit for a client IP has been exceded.
/// If he client exceded the maximum allowed requests per minute, an HTTP error response
/// is sent back to the client and an error is retuned. Otherwise, the function returns Ok(()).
/// 
/// # Param
/// 
/// # Return
/// 
async fn check_rate(state: &ProxyState, client_conn: &mut TcpStream) -> Result<(), std::io::Error> {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    let rate_limiting_counter = state.rate_limiting_counter.clone().lock_owned().await;
    let cnt = rate_limiting_counter.entry(client_ip).or_insert(0);

    if *cnt > state.max_requests_per_minutes {
        let response = response::make_http_error(http::StatusCode::TOO_MANY_REQUESTS);

        // send_response(&mut client_con, &response).await;

        if let Err(error) = response::write_to_stream(&response, client_conn).await {
            log::warn!("Failed to send response to client: {}", error);
        }
        return Err(Error::new(ErrorKind::Other, "Rate limiting"));
    }
    Ok(())
}

async fn rate_limiting_counter_clearer(state: &ProxyState, clear_interval: u64) {
    loop {
        sleep(Duration::from_secs(clear_interval)).await;
        // Clean up counter evert minute
        let mut rate_limiting_counter = state.rate_limiting_counter.clone().lock_owned().await;
        rate_limiting_counter.clear();
    }
}

async fn connect_to_upstream() {

}

async fn handle_connection(mut client_conn: TcpStream, state: &ProxyState) {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!("Connection received from {}", client_ip);

    // Open a connection to a random destination server
    let mut upstream_conn = match connect_to_upstream(state).await {
        Ok(stream) => stream,
        Err(_error) => {
            // handle dead upstream_address
            log::debug!("Client finished sending requests. Shutting down connection");
            return;
        }
    }
    loop{

    }
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
