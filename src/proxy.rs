use crate::request;
use crate::response;

use std::{collections::HashMap, sync::Arc, time::Duration, io::{Error, ErrorKind}};
use hyper::client;
use rand::SeedableRng;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock},
    time::sleep,
    io::{AsyncReadExt, AsyncWriteExt},
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
    rate_limiting_counter: Arc<Mutex<HashMap<String, usize>>>,
}

///
/// Performs active health checks on upstream servers in a loop.
/// Clears the list of live upstream dddresses and checks each upstream server
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
/// Null
/// 
/// 
pub(crate) async fn active_health_check(state: &ProxyState) {
    loop {
        sleep(
            Duration::from_secs(
                state.active_health_check_interval.try_into().unwrap(),
            )
        ).await;

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
                    if let Err(error) = request::write_to_stream(&request, &mut conn).await { // Request::Error
                        log::error!(
                            "Failed to send request to upstream {}: {}",
                            upstream_ip,
                            error
                        );
                        return ;
                    }

                    let response = match response::read_from_stream(&mut conn, &request.method()).await {
                        Ok(response) => response,
                        Err(error) => { // Response::Error
                            log::error!("Error reading response from server: {:?}", error);
                            return;
                        }
                    };

                    // Handle the statusCode of response
                    match response.status().as_u16() {
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
                    }
                },
                Err(err) => { // std::io::Error
                    log::error!("Failed to connect to upstream {}: {}", upstream_ip, err);
                    return;
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
    if let Err(error) = response::write_to_stream(response, client_conn).await {
        log::warn!("All upstream are dead");
        return;
    }
}

///
/// 
/// # Brief
/// This asynchronous function checks if the rate limit for a client IP has been exceded.
/// If he client exceded the maximum allowed requests per minute, an HTTP error response
/// is sent back to the client and an error is retuned. Otherwise, the function returns Ok(()).
/// 
/// # Param
/// - `state`: A reference to the 'ProxyState' which contains the rate limiting counter and 
/// the maxium allowed request per minutes.
/// - `client_conn`: A mutable reference to the'TcpStream' representing the client connection.
/// 
/// # Return
/// - `Result<(), std::io::Error>`: Return `Ok(())`, if the client is within the allowed rate,
/// otherwises returns an error indicating that rate limiting has been enforced.     
/// 
async fn check_rate(state: &ProxyState, client_conn: &mut TcpStream) -> Result<(), std::io::Error> {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    let rate_limiting_counter = state.rate_limiting_counter.clone().lock_owned().await;
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

/// # Brief
/// This asynchronous function continuously clears the rate limiting counter at a specified
/// interval. It runs an infinite loop that sleeps for the given interval and then clears 
/// the rate limiting counter.
///
/// # Param
/// - `state`: A reference to the `ProxyState` which contains the rates limiting counter.
/// - `clear_interval`: The interval in seconds at which the rate limiting counter should be cleared.
/// 
async fn rate_limiting_counter_clearer(state: &ProxyState, clear_interval: u64) {
    loop {
        sleep(Duration::from_secs(clear_interval)).await;
        // Clean up counter evert minute
        let mut rate_limiting_counter = state.rate_limiting_counter.clone().lock_owned().await;
        rate_limiting_counter.clear();
    }
}


/// # Brief
///
/// # Param
///
/// # Return
/// 
///
/// 
/*
pub(crate) async fn active_health_check(state: &ProxyState) {
    loop {
        sleep(
            Duration::from_secs(
                state.active_health_check_interval.try_into().unwrap(),
            )
        ).await;

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

                    // Handle the statusCode of response
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
*/


/// # Brief
///
/// # Param
///
/// # Return
/// 
async fn connect_to_upstream() -> Result<> {

}