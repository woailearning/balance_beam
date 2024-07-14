use crate::request;
use crate::response;

use rand::Rng;
use rand::SeedableRng;
use std::{
    collections::HashMap,
    io::{Error, ErrorKind},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock},
    time::sleep,
};

#[derive(Clone)]
pub(crate) struct ProxyState {
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
    ///
    /// This structure maps IP addresses (as strings) to the number of requests made within
    /// a specific timeframe, used primarily to enforce rate limits.
    ///
    /// It utilizes a mutex for thread-safe concurrent access from multiple asynchronous tasks.
    /// Each modification to the counter acquires a lock,
    rate_limiting_counter: Arc<Mutex<HashMap<String, usize>>>,
}

///
/// This asynchronous function continuously clears the rate limiting counter at a specified
/// interval. It runs an infinite loop that sleeps for the given interval and then clears
/// the rate limiting counter.
///
/// # Brief
/// Asynchronous clears the rate limiting counter at a specified interval.
///
/// # Param
/// - `state`: A reference to the `ProxyState` which contains the rates limiting counter.
/// - `clear_interval`: The interval in seconds at which the rate limiting counter should be cleared.
///
/// # Return
/// Nothing
///
/// # Error
/// Nothing
async fn rate_limiting_counter_clearer(state: &ProxyState, clear_interval: u64) {
    loop {
        sleep(Duration::from_secs(clear_interval)).await;
        // Clean up counter evert minute
        let mut rate_limiting_counter = state.rate_limiting_counter.clone().lock_owned().await;
        rate_limiting_counter.clear();
    }
}

///
/// Performs active health checks on upstream servers in a loop.
/// Clears the list of live upstream addresses and checks each upstream servers
/// by sending an HTTP GET request. Upstreams that return non-200 status codes
/// are marked as failed, while those returning HTTP 200 are considered healthy
/// and added back to the rotation of live upstream servers.
///
/// # Brief
/// Continuously performs active health checks on upstream servers.
///
/// # Param
/// -`state`: A reference to `ProxyState` containing the current proxy configuration and state.
///
/// # Return
/// Nothing, But it it rewrite the value of ProxyState.live_upstream_
///
/// # Error
/// 
pub(crate) async fn active_health_check(state: &ProxyState) {
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
                Ok(mut conn) => {
                    // Write to stream and read from stream
                    if let Err(error) = request::write_to_stream(&request, &mut conn).await {
                        // std::io::Error
                        log::error!(
                            "Failed to send request to upstream {}: {}",
                            upstream_ip,
                            error
                        );
                        return;
                    }

                    let response = match response::read_from_stream(&mut conn, &request.method()).await {
                        Ok(response) => response,
                        Err(error) => {
                            // Response::Error
                            log::error!("Error reading response from server: {:?}", error);
                            return;
                        }
                    };

                    // Handle the statusCode of response
                    match response.status().as_u16() {
                        200 => {
                            live_upstream_addresses.push(upstream_ip.clone());
                        }
                        status @ _ => {
                            log::error!(
                                "upstream server {} is not working: {}",
                                upstream_ip,
                                status
                            );
                            return;
                        }
                    }
                }
                Err(err) => {
                    // std::io::Error
                    log::error!("Failed to connect to upstream {}: {}", upstream_ip, err);
                    return;
                }
            }
        }
    }
}

///
///
/// # Brief
///
/// # Param
///
/// # Return
///
async fn connect_to_upstream(state: &ProxyState) -> Result<TcpStream, std::io::Error> {
    let mut rng = rand::rngs::StdRng::from_entropy();

    loop {
        let live_upstream_addresses = state.live_upstream_addresses.read().await;
        let upstream_idx = rng.gen_range(0..live_upstream_addresses.len());
        let upstream_ip = &live_upstream_addresses.get(upstream_idx).unwrap().clone();

        drop(live_upstream_addresses); // release; read lock

        match TcpStream::connect(upstream_ip).await {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                log::error!("Failed to connect to upstream {}: {}", upstream_ip, err);

                let mut live_upstream_addresses = state.live_upstream_addresses.write().await;
                live_upstream_addresses.swap_remove(upstream_idx); // remove the address from live_upstream_address.

                // All upstreams are dead, return Err
                if live_upstream_addresses.len() == 0 {
                    log::error!("All upstreams are dead");
                    return Err(Error::new(ErrorKind::Other, "All upstreams are dead"));
                }
            }
        }
    }
}

///
/// # Brief
///
/// # Param
///
/// # Return
///
async fn send_response(client_conn: &mut TcpStream, response: &http::Response<Vec<u8>>) {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!(
        "{} <- {}",
        client_ip,
        response::format_response_line(&response)
    );
    if let Err(err) = response::write_to_stream(response, client_conn).await {
        log::warn!("Failed to send response to the client {}", err);
        return;
    }
}

///
/// This asynchronous function checks if the rate limit for a client IP has been exceded.
/// If he client exceded the maximum allowed requests per minute, an HTTP error response
/// is sent back to the client and an error is retuned. Otherwise, the function returns Ok(()).
///
/// # Brief
///
/// # Param
/// - `state`: A reference to the 'ProxyState' which contains the rate limiting counter and
/// the maxium allowed request per minutes.
/// - `client_conn`: A mutable reference to the'TcpStream' representing the client connection.
///
/// # Return
/// - `Result<(), std::io::Error>`:
/// Return `()`, if the client is within the allowed rate,
/// Return `std::io::Error`, otherwises returns an error indicating that rate limiting has been enforced.     
///
/// - `Ok(())`:
/// - `Err(std::io::Error)`:
///
/// # Error
///
async fn check_rate(state: &ProxyState, client_conn: &mut TcpStream) -> Result<(), std::io::Error> {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    let mut rate_limiting_counter = state.rate_limiting_counter.clone().lock_owned().await;
    // need to wait for get owner for this variable and process it.
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

///
/// # Brief
///
/// # Param
///
/// # Return
///
async fn handle_connection(mut client_conn: TcpStream, state: &ProxyState) {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!("Connection received from {}", client_ip);

    // Open a connection to a random destination server.
    let mut upstream_conn = match connect_to_upstream(state).await {
        Ok(stream) => stream,
        Err(_error) => {
            let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
            send_response(&mut client_conn, &response).await;
            return;
        }
    };
    let upstream_ip = upstream_conn.peer_addr().unwrap().ip().to_string();

    // The client may now send us one or more requests. Keep trying to read requests until
    // the client hangs up or we get an error.
    loop {
        // read a request from the client.
        let mut request = match request::read_from_stream(&mut client_conn).await {
            Ok(request) => request,
            Err(request::Error::IncompleteRequest(0)) => {
                log::debug!("Client finished sending requests. Shutting down connection");
                return;
            }
            Err(request::Error::ConnectionError(io_err)) => {
                // handle I/O error in reading from the client
                log::info!("Error reading request from client stream {}", io_err);
                return;
            }
            Err(error) => {
                log::debug!("Error parsing request: {:?}", error);
                let response = response::make_http_error(match error {
                    request::Error::IncompleteRequest(_)
                    | request::Error::MalformaedRequest(_)
                    | request::Error::InvaildContentLength
                    | request::Error::ContentLengthMismatch => http::StatusCode::BAD_REQUEST,

                    request::Error::RequestBodyTooLarge => http::StatusCode::PAYLOAD_TOO_LARGE,

                    request::Error::ConnectionError(_) => http::StatusCode::SERVICE_UNAVAILABLE,
                });
                send_response(&mut client_conn, &response).await;
                continue;
            }
        };
        log::info!(
            "{} -> {}: {}",
            client_ip,
            upstream_ip,
            request::format_request_line(&request)
        );

        // rate limiting
        if state.max_requests_per_minutes > 0 {
            if let Err(_error) = check_rate(&state, &mut client_conn).await {
                log::error!("{} rate limiting", &client_ip);
                continue;
            }
        }

        // Add X-Forwarded-For header so that the upstrram server knows the client's IP address.
        // (we're the ones connecting directly to the upstream server, so without this header, the
        // upstream server will only know our IP, not the client's)
        request::extend_header_value(&mut request, "x-forwarded-for", &client_ip);

        // Forward the request to the server
        if let Err(err) = request::write_to_stream(&request, &mut upstream_conn).await {
            log::error!(
                "Filed to send request to the upstream {}: {}",
                upstream_ip,
                err
            );
            let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
            send_response(&mut client_conn, &response).await;
            return;
        }
        log::debug!("Forwarded request to server");

        let response = match response::read_from_stream(&mut upstream_conn, request.method()).await
        {
            Ok(response) => response,
            Err(error) => {
                log::error!("Error reading response from server: {:?}", error);
                let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
                send_response(&mut client_conn, &response).await;
                return;
            }
        };

        // Forward the response to the client.
        send_response(&mut client_conn, &response).await;
        log::debug!("Forwarded response to client");
    }
}