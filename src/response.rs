use http::request;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_HEADERS_SIZE: usize = 8000;
const MAX_BODY_SIZE: usize = 10000000;
const MAX_NUM_HEADERS: usize = 32;

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

/// This function attempts to retrieve the `Content-Length` header from the provided HTTP response.
///
/// # Brief
/// Retrieves the content length from the HTTP response headers, if available.
/// 
/// # Paoam
/// - `response`: A reference to an `http::Response<Vec<u8>>` from which the `Content-Length` header will be retrieved.
/// 
/// # Return
/// - `Ok(Some(usize))`: if the `Content-Length` header is found and is a valid integer.
/// - `Ok(None)`: if the `Content-Length` header is not present.
/// - `Err(Error)`: if the `Content-Length` header is present but is not a valid integer.
/// 
/// # Error
/// - `Error::InvalidContentLength`: the `Content-Length` header is present but can not parse a valid integer.
/// 
fn get_content_length(response: &http::Response<Vec<u8>>) -> Result<Option<usize>, Error> {
    // Look for content-length header
    if let Some(header_value) = response.headers().get("content-length") {
        // If it exists, parse it as a usize (or return InvalidResponseError if it can't be parsed as such)
        Ok(Some(
            header_value
                .to_str()
                .or(Err(Error::InvalidContentLength))?
                .parse::<usize>()
                .or_else(|err| Err(Error::InvalidContentLength))?
        ))
    } else {
        // If it doesn't exist, return None.
        Ok(None)
    }
}

/// This function attempts to parse HTTP response from the provided buffer.
/// It processes the response line and headers. If the parsing is complete,
/// it returns the response and the length of the parsed data.
/// 
/// # Brief
/// Parse an HTTP response from a buffer.
/// 
/// # Param
/// - `buffer` A bytes slice containing the **HTTP response** to parsed.
/// 
/// # Return
/// - `Ok(Some(http::Response<Vec<u8>>))`: If a valid HTTP response is successfully parsed.
/// - `Ok(None)`: if the response is not complete.
/// - `Err(Error)`: if the response is not valid HTTP response.
/// 
/// # Error
/// - `Error::MalformeResponse`: If the response is not HTTP response according to the `httparse` crate
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

/// This function attempts to read the response line and headers from the provided `TcpStream`.
/// It handles the case where not all headers may be received in single read operation. After
/// successfully reading the headers, any remaining data in the buffer is considered the start of
/// the response body.
/// 
/// # Brief
/// Reads the HTTP respnse headers from a TCP stream.
/// 
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the HTTP response headers will be read.
/// 
/// # Return
/// - `Ok(http::Response<Vec<u8>>` an success, cantaining the response with headers and any initial part 
/// - `Err(Error)` if there is an error during reading or parsing the headers.
/// 
/// # Error
/// - `Error::ConnectionError`: If there was an error reading from the TCP stream.
/// - `Error::IncompleteResponse`: If the stream is closed before a complete response is received.
/// - `Error::MalformadResponse`: If the response received is not a valid HTTP according to the `parse_response`
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
            .or_else(|err| Err(Error::ConnetionError(err)))?;

        if new_bytes == 0 {
            return Err(Error::IncompleteResponse);
        }
        bytes_read += new_bytes;

        // See if we've read a valid response (usize) so far
        if let Some((mut response, headers_len)) = parse_response(&response_buffer[..bytes_read])? {
            // We've read a complete set of headers. We may have also read the first part of the 
            // response body; take whatever is left over in the response buffer and save that as 
            // the start of the response body.
            response
                .body_mut()
                .extend_from_slice(&response_buffer[headers_len..bytes_read]);
            return Ok(response);
        }
        // continue, when we got None from parse_response()
    }
}

/// This function reads the body for a response from TcpStream. If the Content-Length header is present,
/// it reads that many bytes; otherwise, it reads bytes until the connectino is closed.
/// 
/// # Brief
/// Reads the body of an HTTP response from a TCP stream.
/// 
/// # Param
/// - `stream`: A mutable reference to a `TcpStream ` from which the HTTP resposne body will be present.
/// - `response`: A mutable reference to  an `http::Resposne<Vec<u8>>` which will hold the response body.
/// 
/// # Return
/// Returns a `Result`:
/// - `Ok(())` on success,indicating the body has been read successfully.
/// - `Err(Error)` if there is an error during the read process.
///
/// # Error
/// - `Error::ConnectionError`: If there is an error, reading from the TCP stream
/// - `Error::ContentLengthMismatch`: If the server closes the connection before the entire body (as specified by `Content-Length`) is read.
///
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

/// 
/// # Brief
/// 
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the HTTP response body will be read.
/// - `request_method`: A mutable reference to an `http::Response<Vec<u8>>` which will hold the response body.
/// 
/// # Return
/// Return a `Result`:
/// 
/// # Error
/// 
/// - `Error::ConnectionError`: if there is an error reading from the TCP stream.
/// 
/// - `Error::ContentLengthMismatch`: If the server closes the connection before the entire body(as specifie by `Content-Length`) is read.(from read_header)
/// - `Error::IncompleteResponse`: If the stream is closed before a compelte response is received. (from read_body)
/// - `Error::MalformadResponse`: If the response received is not valid HTTP response according to the `parse_response`(from read_body)
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

///
/// # Brief
/// 
/// # Param
/// 
/// # Return
/// 
async fn write_to_stream (response: &http::Response<Vec<u8>> , stream: &mut TcpStream) -> Result<(), std::io::Error> {
    stream
        .write(&format_response_line(&response).into_bytes())
        .await?;
    stream.write(&['\r' as u8, '\n' as u8]).await?;

    for (header_name, header_value) in response.headers() {
        stream.write(&format!("{}:", header_name).into_bytes()).await?;
        stream.write(header_value.as_bytes()).await?;
        stream.write(&['\r' as u8, '\n' as u8]).await?;
    }
    stream.write(&['\r' as u8, '\n' as u8]).await?;
    if response.body().len() > 0 {
        stream.write(response.body()).await?;
    }
    Ok(())
}

/// 
/// # Brief
/// Provides a brief summray of what the function does. In this case, it's
/// about fotmatting request line of HTTP request into a String.
/// 
/// # Param
/// -`response`: An `http::Response` object representing the HTTP request.
/// 
/// # Return
/// A `String` containing the formatted request line.
/// 
pub fn format_response_line(response: &http::Response<Vec<u8>>) -> String {
    format!(
        "{:?} ,{} {}",
        response.version(),
        response.status().as_str(),
        response.status().canonical_reason().unwrap_or("")
    )
}

/// 
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