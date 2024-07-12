use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

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

    /// The Content-Length header does not match the size of the request body that was sent
    ContentLengthMismatch,

    /// The request body is bigger than MAX_BODY_SIZE
    ResponseBodyTooLarge,

    /// Encounter an I/O error when reading/writing a TcpStream
    ConnectionError(std::io::Error),
}

///
/// Extracts the Content-Length header value from the provided response. Return Ok(Some(usize)) if
/// the Content-Length is present and vaild, Ok(None) if Content-Length is not present, or
/// Err(Error) If Content-Length is present but invaild.
///
/// # Brief
/// Retrieves the content length from the HTTP response headers, if available.
///
/// # Paoam
/// - `response`: A reference to an `http::Response<Vec<u8>>` from which the `Content-Length` header will be retrieved.
///
/// # Return
/// - `Result<Option<usize>, Error>`: 
/// - `Ok(Some(usize))`: if the `Content-Length` is present and successfully parsed as a `usize`.
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
                .or_else(|err| Err(Error::InvalidContentLength))?,
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
/// Parses an HTTP response from the given buffer and constructs an `http::Response` object.
///
/// # Param
/// - `buffer` A bytes slice containing the **HTTP response** to parsed.
///
/// # Return
/// Return a `Result<Option<(http::Response<_>, usize)>, Error>`:
/// - `Ok(Some((http::Response<Vec<u8>>, usize)))`: if the `Content-Length` is present and successfully parsed as a `usize`.
/// - `Ok(None)`: if the `Content-Length` header is not present.
/// - `Err(Error)`: if there was an error parsing the response, encapsulated in the `Error` enum.
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
            // The header of HTTP response have many pairs(name - value)
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
/// Reads the HTTP response headers from a TCP stream.
///
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the HTTP response headers will be read.
///
/// # Return
/// Return a 'Result<http::Response<Vec<u8>>, Error>`:
/// - `Ok(http::Response<Vec<u8>>` Return an HTTP response object cantaining the response headers.
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
            .or_else(|err| Err(Error::ConnectionError(err)))?;

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
/// it reads that many bytes; otherwise, it reads bytes until the connection is closed.
///
/// # Brief
/// Reads the body of an HTTP response from a TCP stream.
///
/// # Param
/// - `stream`: A mutable reference to a `TcpStream ` from which the HTTP resposne body will be present.
/// - `response`: A mutable reference to  an `http::Resposne<Vec<u8>>` which will hold the response body.
///
/// # Return
/// Returns a `Result<(), Error>`:
/// - `Ok(())` on success,indicating the body has been read successfully.
/// - `Err(Error)` if there is an error during the read process.
///
/// # Error
/// - `Error::ConnectionError`: if there is an issue reading from the TCP stream.
/// - `Error::ContentLengthMismatch`: If the number of bytes read does not match the expected content Length.
/// - `Error::ResponseBodyTooLarge`: If the body of response is too Large, then we throw that bytes away.
/// - `Error::InvalidContentLength`: the Content-Length header is present but can not parse a valid integer(from get_content_length()).
///
async fn read_body(
    stream: &mut TcpStream,
    response: &mut http::Response<Vec<u8>>,
) -> Result<(), Error> {
    let content_length = get_content_length(response)?;

    while content_length.is_none() || response.body().len() < content_length.unwrap() {
        let mut buffer = [0_u8; 512];
        let bytes_read = stream
            .read(&mut buffer)
            .await
            .or_else(|err| Err(Error::ConnectionError(err)))?;

        if bytes_read == 0 {
            // The server has hung up
            if content_length.is_none() {
                // We've reached the end of the response
                break;
            } else {
                // content-Length was set, but the server hung up before we manageed to read
                // that number of bytes
                return Err(Error::ContentLengthMismatch);
            }
        }
        // Make sure server doesn't send move bytes than it promised to send
        if content_length.is_some() && response.body().len() + bytes_read > content_length.unwrap() {
            return Err(Error::ContentLengthMismatch)
        }
        // Make sure server doesn't send more bytes than we allow
        if response.body().len() + bytes_read > MAX_BODY_SIZE {
            return Err(Error::ResponseBodyTooLarge)
        }
        // Append received bytes to the response body
        response.body_mut().extend_from_slice(&buffer[..bytes_read])
    }
    Ok(())
}

///
/// This function first reads the HTTP response headers using the `read_header` function.
/// Depending on the request method and status code, it may also read the response body using.
/// 
/// # Brief
/// Reads an HTTP response from a TCP stream, including headers and body.
///
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the HTTP response body will be read.
/// - `request_method`: A mutable reference to an `http::Response<Vec<u8>>` which will hold the response body.
///
/// # Return
/// Return a `Result`:
/// - `Ok(http::Response<Vec<u8>>)`: on success, containing the complete HTTP response with headers and body (if applicable)
/// - `Err(Error)`: if there is an error during the read process.
///
/// # Error
/// - `Error::ConnectionError`: if there is an error reading from the TCP stream.
/// - `Error::ContentLengthMismatch`: If the server closes the connection before the entire body(as specifie by `Content-Length`) is read.(from read_header)
/// - `Error::IncompleteResponse`: If the stream is closed before a compelte response is received. (from read_body)
/// - `Error::MalformadResponse`: If the response received is not valid HTTP response according to the `parse_response`(from read_body)
///
pub async fn read_from_stream(
    stream: &mut TcpStream,
    request_method: &http::Method,
) -> Result<http::Response<Vec<u8>>, Error> {
    let mut response = read_headers(stream).await?;

    if !(request_method == http::Method::HEAD
        || response.status().as_u16() < 200
        || response.status() == http::StatusCode::NO_CONTENT
        || response.status() == http::StatusCode::NOT_MODIFIED)
    {
        read_body(stream, &mut response).await?;
    }
    Ok(response)
}

/// 
/// This asynchronous function takes an `http::Response` object and writes its formatted
/// components to the provided TCP stream. It writes the response, headers, and body(is present)
/// to the stream.
/// 
/// # Brief
/// Writes an HTTP response to a TCP stream
///
/// # Param
/// - `response`: An reference `http::Response<Vec<u8>>` object representing the HTTP response to write.
/// - `stream`: A mutable reference to a `TcpStream` where the response will be written.
///
/// # Return
/// Return a `Result<(), std::io::Error>`:
/// - Ok(()): If the write operation was successful.
/// - Err(std::io::Error): If an I/O error occurred during the write operation.
/// 
/// # Error
/// - `std::io::Error`: Indicates that an I/O error occurred while writing to the stream.
///
pub async fn write_to_stream(
    response: &http::Response<Vec<u8>>,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    stream
        .write(&format_response_line(&response).into_bytes())
        .await?;
    stream.write(&['\r' as u8, '\n' as u8]).await?;

    for (header_name, header_value) in response.headers() {
        stream
            .write(&format!("{}:", header_name).into_bytes())
            .await?;
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
/// This function takes an `http::Response<Vec<u8>>` object and construct a formatted string
/// representing the response line, which includes HTTP Version, status code and reason pharse.
///
/// # Brief
/// Provides a brief summray of what the function does. In this case, it's
/// fotmatting response line of HTTP response into a String.
///
/// # Param
/// -`response`: An `http::Response` object representing the HTTP response.
///
/// # Return
/// A `String` containing the formatted response line.
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
/// This function generates an HTTP resposne indicating an error condition.
/// suitable for sending to clients. The response body includes the HTTP
/// status code and its corresponding reason phrase.
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
pub fn make_http_error(status: http::StatusCode) -> http::Response<Vec<u8>> {
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
