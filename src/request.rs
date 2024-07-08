use std::cmp::min;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_BODY_SIZE: usize = 10000;
const MAX_NUM_HEADERS: usize = 32;

#[derive(Debug)]
pub enum Error {
    /// Client hung up before sending a complete request.
    /// IncompleteRequest contains the number of bytes that were
    /// successfully read before the client hung up
    IncompleteRequest(usize),

    /// Client sent on invaild HTTP request. httparse::Error contains more details
    MalformaedRequest(httparse::Error),

    /// The Content-Length header is present, but does not contain a valid numeric value
    InvaildContentLength,

    /// The Content-Length header does is not match the size of the request body that was sent
    ContentLengthMismatch,

    /// The request body is bigger than MAX_BODY_SIZE
    RequestBodyTooLarge,

    /// Encountered an I/O error when reading/writing a TcpStream
    ConnectionError(std::io::Error),
}


/// # Brief
/// Extracts the Content-Length header value from the provided request. Return Ok(Some(usize)) if
/// the Content-Length is present and vaild, Ok(None) if Content-Length is not present, or
/// Err(Error) If Content-Length is present but invaild.
///
/// # Param
/// - `request`: A reference to an HTTP request of type `http::Request<Vec<u8>>` from which the content
/// length is to be retrieved.
///
/// # Return
/// - `Result<Option<usize>, Error>`: Returns `Ok(Some(content_length))` if the `Content-Length` is present
/// and successfully parsed as a `usize`. Return `Ok(None)` if the `Content-Length` header is not present.
/// Returns `Err(Error::InvaildContentLength)` if the header value cannot be parsed as a `usize`.
///
fn get_content_length(request: &http::Request<Vec<u8>>) -> Result<Option<usize>, Error> {
    // look for content-length header.
    if let Some(header_value) = request.headers().get("content-length") {
        // If it exists, parse it as a usize(or return InvalidContentLength if it can't be parse as such)
        Ok(Some(
            header_value
                .to_str()
                .or(Err(Error::InvaildContentLength))?
                .parse::<usize>()
                .or(Err(Error::InvaildContentLength))?,
        ))
    } else {
        Ok(None)
    }
}

/// # Brief
/// Parses an HTTP request from the given buffer and constructs an `http::Request` object.
/// 
/// # Param
/// - `buffer`: A slice of `u8` bytes containing the HTTP request to be parsed
/// 
/// # Return
/// Return a `Result`:
/// - `Ok(Some((request, len)))`: if parseing succeeds and the request is complete. `request` is an
/// `http::Request` containing parsed headers and an empty body. `len` is the Length of the parsed 
/// request in bytes.
/// - `Ok(None)`: if parsing is incomplete and requires more data.
/// - `Err(Error)`: if there was an error parsing the request, encapsulated in the `Error` enum.
/// 
fn parse_request(buffer: &[u8]) -> Result<Option<(http::Request<Vec<u8>>, usize)>, Error> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_NUM_HEADERS];
    let mut req = httparse::Request::new(&mut headers);

    let res = req
        .parse(buffer)
        .or_else(|err| Err(Error::MalformaedRequest(err)))?;

    if let httparse::Status::Complete(len) = res {
        let mut request = http::Request::builder()
            .method(req.method.unwrap())
            .uri(req.path.unwrap())
            .version(http::Version::HTTP_11);

        for header in req.headers {
            request = request.header(header.name, header.value);
            // The header of HTTP request have many pairs(name - value)
        }
        let request = request.body(Vec::new()).unwrap();

        Ok(Some((request, len)))
    } else {
        Ok(None)
    }
}

/// # Brief
/// Reads the HTTP request headers from a TCP stream an HTTP request object.
///
/// # Param
/// - `stream`: A mutable reference to the TCP stream object to reading from
///
/// # Return
/// - 'Result<http::Request<Vec<u8>>, Error>`: Return an HTTP request object
/// cantaining the request headers. 
/// - `Err(Error)`: If an error occurs during reading or parsing,
/// returns an error result.
///
async fn read_header(stream: &mut TcpStream) -> Result<http::Request<Vec<u8>>, Error> {
    // Try reading the headers from the request. We may not receive all the headers in one shot
    // (e.g. we might receive the first few bytes of a request, and then the rest follows later).
    // Try parsing repeatedly until we read a valid HTTP request.
    let mut request_buffer = [0_u8; MAX_BODY_SIZE];
    let mut bytes_read = 0;

    loop {
        // Read bytes from the connection into the buffer, starting at position bytes_read
        let new_bytes = stream
            .read(&mut request_buffer[bytes_read..])
            .await
            .or_else(|err| Err(Error::ConnectionError(err)))?;
            // TCP connection transmits a stream of data, not messages
            // Data can be split into fragments for transmission
            // Receiver handles order and completeness of fragments
            // Amount of data read per call to stream.read() is indeterminat
            
        if new_bytes == 0 {
            // We didn't manage to read a complete request
            return Err(Error::IncompleteRequest(bytes_read));
        }
        bytes_read += new_bytes;

        // See if we've read a vaild request so far
        if let Some((mut request, headers_len)) = parse_request(&request_buffer[..bytes_read])? {
            // we've read a complete set of headers. However if this was a POST request, a request
            // body might have been included as well, and we might read part of the body out of the
            // stream into header_buffer. We need to add those bytes to the Request body so that
            // we don't lose them.
            request
                .body_mut()
                .extend_from_slice(&request_buffer[headers_len..bytes_read]);

            return Ok(request);
        }
    }
}

/// # Brief
/// Reads the body of an HTTP request from a TCP stream.
///
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the body will be read
/// - `request`: A mutable reference to an HTTP `Request` object where the body will be shared.
/// - `content_length`: The expcted Length of the request body in bytes.
///
/// # Return
/// - `Error::ConnectionError`: if there is an issue reading from the TCP stream.
/// - `Error::ContentLengthMismatch`: If the number of bytes read does not match the expected content Length.
/// 
async fn read_body(
    stream: &mut TcpStream,
    request: &mut http::Request<Vec<u8>>,
    content_length: usize,
) -> Result<(), Error> {
    // Keep reading data until we read the full body length, or until we hit an error.
    while request.body().len() < content_length {
        // Read up to 512 bytes at a time. (If the client only sent a small body, then only allocate
        // space to read that body )
        let mut buffer = vec![0_u8; min(512, content_length)];
        let bytes_read = stream
            .read(&mut buffer)
            .await
            .or_else(|err| Err(Error::ConnectionError(err)))?;

        // Make sure the client is still sending us bytes.
        if bytes_read == 0 {
            log::debug!(
                "Client hung up after sending a body of length {}, even though it said the current \
                length is {}",
                request.body().len(),
                content_length,
            );
            // We didn't manage to read a complete request.
            return Err(Error::ContentLengthMismatch);
        }

        // Make sure the client didn't send us *too many* bytes
        if request.body().len() + bytes_read > content_length {
            log::debug!(
                "Client sent more bytes than we expected based on the given content length"
            );
            return Err(Error::ContentLengthMismatch);
        }

        // Store the received by in the request body.
        request.body_mut().extend_from_slice(&buffer[..bytes_read]);
    }
    // get content fulled body length
    Ok(())
}

/// # Brief
/// Reads an HTTP request from a TCP stream, including headers and optional body.
///
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the HTTP request will be read.
///
/// # Return
/// Returns `Result` which, on success, contains on `http::Request` which the request headers and body.
/// on failure, it returns an `Error` indicating what went wrong during the read process.
/// - `http::Request<Vec<u8>>`: Retrieve the request from read_header(), then pass it as a parameter to read_body()
/// - `Err(Error)`: When read_header() or read_body() encounters an error, it should throw an Error to the caller.
/// 
pub async fn read_from_stream(stream: &mut TcpStream) -> Result<http::Request<Vec<u8>>, Error> {
    // Read headers
    let mut request = read_header(stream).await?;
    // Read body if the client supplied the Content-Length header (which it does for POST requests)
    if let Some(content_length) = get_content_length(&request)? {
        if content_length > MAX_BODY_SIZE {
            return Err(Error::RequestBodyTooLarge);
        } else {
            read_body(stream, &mut request, content_length).await?
        }
    }
    Ok(request)
}

/// # Brief 
/// Writes an HTTP request to a TCP stream.
/// 
/// # Param
/// - `request`: A `http::Request` containing the HTTP request to be written to the stream. The body is 
/// represented as a `Vec<u8>`,
/// - `stream`: A mutable reference to a `TcpStream` to which the HTTP request will be written.
/// 
/// # Return
/// Returns a `Result` which, on success, is `Ok(())`. On failure, it returns an `std::io::Error`
/// indicating what went wrong during the write process.
/// 
pub async fn write_to_stream(
    request: http::Request<Vec<u8>>,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    stream
        .write(&format_request_line(&request).into_bytes())
        .await?;
    stream.write(&['\r' as u8, '\n' as u8]).await?;
    for (header_name, header_value) in request.headers() {
        stream.write(&format!("{}", header_name).as_bytes()).await?;
        stream.write(header_value.as_bytes()).await?;
        stream.write(&['\r' as u8, '\n' as u8]).await?;
    }
    stream.write(&['\r' as u8, '\n' as u8]).await?;
    if request.body().len() > 0 {
        stream.write(request.body()).await?;
    }
    Ok(())
}

/// # Brief
/// Provides a brief summary of what the function does. In this case, it's about 
/// fomatting request line of HTTP request into a string.
///
/// # Param
/// - `response`: An 'http::Request' object representing the HTTP request
///
/// # Return
/// A 'String' containing the formatted request line
///
pub fn format_request_line(request: &http::Request<Vec<u8>>) -> String {
    format!(
        "{} {} {:?}",
        request.method(),
        request.uri(),
        request.version()
    )
}

/// # Brief
/// 
/// # Param
/// 
/// # Return
pub fn extend_header_value(request: &mut http::Request<Vec<u8>>, name: &'static str, extend_value: &str) {

}