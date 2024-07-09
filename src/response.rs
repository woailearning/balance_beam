use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt};

const MAX_HEADERS_SIZE: usize = 8000;
const MAX_BODY_SIZE: usize = 10000000;
const MAX_NUM_HEADERS: usize = 32;

/// # Brief
/// 
/// # Param
/// 
/// # Return
/// 
#[derive(Debug)]
pub enum Error {
    /// Client hung up before sending a complete request.
    IncompleteResponse,

    /// Client sent an invalid HTTP request. httparse::Error contains more details
    MalformeResponse(httparse::Error),

    /// The Content-Length header is present, but does not contain a valid numeric value
    InvalidContentLength,
    
    /// The request body is bigger than MAX_BODY_SIZE
    ResponseBodyTooLarge,

    /// The Content-Length header does not match the size of the request body that was sent
    ContentLengthMismatch,

    /// Encounter an I/O error when reading/writing a TcpStream
    ConnetionError(std::io::Error),
}

fn get_content_length(response: &http::Response<Vec<u8>>) -> Result<Option<usize>, Error> {
    // Look for content-length header
    if let Some(header_value) = response.headers().get("content-length") {
        Ok(Some(
            header_value
                .to_str()
                .or(Err(Error::InvalidContentLength))?
                .parse::<usize>()
                .or_else(|err| Err(Error::InvalidContentLength))?
        ))
    } else {
        Ok(None)
    }
}

/// # Brief
/// 
/// # Param
/// 
/// # Return
/// 
fn parse_response(buffer: &[u8]) -> Result<Option<(http::Response<Vec<u8>>, usize)>, Error> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_NUM_HEADERS];
    let mut resp = httparse::Response::new(&mut headers);
    
    let res = resp
        .parse(buffer)
        .or_else(|err| Err(Error::MalformeResponse(err)))?;
    
    if let httparse::Status::Complete(len) = res {
        let mut response = http::Response::builder()
            .status(resp.code.unwrap())
            .version(http::Version::HTTP_11);
        for header in resp.headers {
            response = response.header(header.name, header.value);
        }
        let response = response.body(Vec::new()).unwrap();
        Ok(Some((response, len)))
    } else {
        Ok(None)
    }
}

/// # Brief
/// 
/// # Param
/// 
/// # Return
/// 
async fn read_headers(stream: &mut TcpStream) -> Result<http::Response<Vec<u8>>, Error> {
    // Try reading the headers from the response. We may not receive all the headers in one shot
    // sent. This function only reads the response line and headers; the read_body function can
    // subsequently be called in order to read the response body.
    let mut response_buffer = [0_u8; MAX_HEADERS_SIZE];
    let mut bytes_read = 0;
    loop {
        // Read bytes from the connection into the buffer, starting at position bytes_read
        let new_bytes = stream
            .read(&mut response_buffer[bytes_read..])
            .await
            .or_else(|err| Err(Error::ConnetionError(err)));

        if new_bytes == 0 {
            return Err(Error::IncompleteResponse);
        }
        bytes_read += new_bytes;

        // See if we've read a valid response so fat
        if let Some((mut response, headers_len)) = parse_response(&response_buffer[..bytes_read])? {
            // We've read a complete set of headers. We may have also read the first part of the 
            // response body; take whatever is left over in the response buffer and save that as 
            // the start of the response body.
            response
                .body_mut()
                .extend_from_slice(&response_buffer[headers_len..bytes_read]);
            return Ok(response);
        }
    }
}

/// # Brief
/// 
/// # Param
/// 
/// # Return
async fn read_body(stream: &mut TcpStream, response: &mut http::Response<Vec<u8>>) -> Result<(), Error> {
    let content_length = get_content_length(response)?;

    while content_length.is_none() || response.body().len() < content_length.unwrap() {
        let mut buffer = [0_u8; 512];
        let bytes_read = stream
           .read(&mut buffer)
           .await
           .or_else(|err| Err(Error::ConnetionError(err)))?;

        if bytes_read == 0 {
                // The server has hung up
            if content_length.is_none() {
                // We've reached the end of the response
                break;
            } else {
                // content-Length was set, but the server hung up before we manageed to read
                // that number of bytes
                return Err(Error::ContentLengthMismatch)
            }
        }
    }
    Ok(())
}

async fn write_to_stream (stream: &mut TcpStream) -> Result<(), std::io::Error> {

}

/// # Brief
/// 
/// # Param
/// 
/// # Return
/// 
pub async fn read_from_stream(stream: &mut TcpStream, request_method: &http::Method) -> Result<http::Response<Vec<u8>>, Error> {
    let mut response = read_headers(stream).await?;
    
    if !( request_method == http::Method::HEAD
        || response.status().as_u16() < 200
        || response.status() == http::StatusCode::NO_CONTENT
        || response.status() == http::StatusCode::NOT_MODIFIED) 
    {
        read_body(stream, &mut response).await?;
    }
    Ok(response)
}

/// # Brief
/// This is a helper function that creates an http::Response containing an HTTP error
/// that can be send a client.
///
/// # Param
/// -`status`: The HTTP status code indcating the error to the returned.
///
/// # Return
/// An 'http::Response<Vec<u8>>' containing the formatted HTTP error response.
///
pub async fn make_http_error(status: http::StatusCode) -> http::Response<Vec<u8>> {
    let body = format!(
        "HTTP {}, {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
    )
    .into_bytes();

    http::Response::builder()
        .status(status)
        .header("Content-Type", "test/plain")
        .header("Content-Length", body.len().to_string())
        .version(http::Version::HTTP_11)
        .body(body)
        .unwrap()
}