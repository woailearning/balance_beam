use std::cmp::min;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_BODY_SIZE: usize = 10000;
const MAX_NUM_HEADERS: usize = 32;
const MAX_HEADERS_SIZE: usize = 8000;

#[derive(Debug)]
pub enum Error {
    /// Client hung up before sending a complete request.
    /// IncompleteRequest contains the number of bytes that were
    /// successfully read before the client hung up
    IncompleteRequest(usize),

    /// Client sent on invaild HTTP request. httparse::Error contains more details
    MalformedRequest(httparse::Error),

    /// The Content-Length header is present, but does not contain a valid numeric value
    InvalidContentLength,

    /// The Content-Length header does is not match the size of the request body that was sent
    ContentLengthMismatch,

    /// The request body is bigger than MAX_BODY_SIZE
    RequestBodyTooLarge,

    /// Encountered an I/O error when reading/writing a TcpStream
    ConnectionError(std::io::Error),
}


///
/// Extracts the Content-Length header value from the provided request. Return Ok(Some(usize)) if
/// the Content-Length is present and vaild, Ok(None) if Content-Length is not present, or
/// Err(Error) If Content-Length is present but invaild.
/// 
/// # Brief
/// Retrieves the content length from the HTTP request headers, if available.
///
/// # Param
/// - `request`: A reference to an `http::Request<Vec<u8>>` from which the `Content-Length` header will be retrieved.
///
/// # Return
/// Return a `Result<Option<usize>, Error>`: 
/// - `Ok(Some(usize))` if the `Content-Length` is present and successfully parsed as a `usize`. 
/// - `Ok(None)` if the `Content-Length` header is not present.
/// - `Err(Error)` if the header value cannot be parsed as a `usize`.
/// 
/// # Error
/// - `Error::InvalidContentLength`: the `Content-Length` header is present but can not parse a valid integer.
///
fn get_content_length(request: &http::Request<Vec<u8>>) -> Result<Option<usize>, Error> {
    // look for content-length header.
    if let Some(header_value) = request.headers().get("content-length") {
        // If it exists, parse it as a usize(or return InvalidContentLength if it can't be parse as such)
        Ok(Some(
            header_value
                .to_str()
                .or(Err(Error::InvalidContentLength))?
                .parse::<usize>()
                .or(Err(Error::InvalidContentLength))?,
        ))
    } else {
        // If it doesn't exist, return None.
        Ok(None)
    }
}

///
/// This function attempts to parse HTTP request from the provided buffer.
/// It processes the request line and **headers**. If the parsing is complete,
/// it returns the request and the length of the parsed data.
///
/// # Brief
/// Parses an HTTP request from the given buffer and constructs an `http::Request` object.
/// 
/// # Param
/// - `buffer` A bytes slice containing the **HTTP request** to parsed.
/// 
/// # Return
/// Return a `Result<Option<(http::Request<_>, usize)>, Error>`:
/// - `Ok(Some((http::Request<Vec<u8>>, usize)))`: if the `Content-Length` is present and successfully parsed as a `usize`.
/// - `Ok(None)`: if the `Content-Length` header is not present.
/// - `Err(Error)`: if there was an error parsing the request, encapsulated in the `Error` enum.
/// 
/// # Error
/// - `Error::MalformeRequest`: If the reqeust is not HTTP request according to the `httparse` crate
/// 
fn parse_request(buffer: &[u8]) -> Result<Option<(http::Request<Vec<u8>>, usize)>, Error> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_NUM_HEADERS];
    let mut req = httparse::Request::new(&mut headers);

    let res = req
        .parse(buffer)
        .or_else(|err| Err(Error::MalformedRequest(err)))?;

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

/// This function attempts to read the request line and headers from the provided `TcpStream`.
/// It handles the case where not all headers may be received in single read operation. After
/// successfully reading the headers, any remaining data in the buffer is considered the start of
/// the request body.
/// 
/// # Brief
/// Reads the HTTP request headers from a TCP stream.
///
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the HTTP reqeust headers will be read.
///
/// # Return
/// Return a 'Result<http::Request<Vec<u8>>, Error>`:
/// - `Ok(http::Request<Vec<u8>>` : Return an HTTP request object cantaining the request headers. 
/// - `Err(Error)`: if there is an error during reading or parsing the headers.
/// 
/// # Error
/// - `Error::ConnectionError`: If there was an error reading from the TCP stream.
/// - `Error::IncompleteRequest`: If the stream is closed before a complete request is received.
/// - `Error::MalformadRequest`: If the request received is not a valid HTTP according to the `parse_request`
///
async fn read_header(stream: &mut TcpStream) -> Result<http::Request<Vec<u8>>, Error> {
    // Try reading the headers from the request. We may not receive all the headers in one shot
    // (e.g. we might receive the first few bytes of a request, and then the rest follows later).
    // Try parsing repeatedly until we read a valid HTTP request.
    let mut request_buffer = [0_u8; MAX_HEADERS_SIZE];
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
            // bytes_read used as index

            return Ok(request);
        }
    }
}

/// This function read the body for a request from TcpStream. If the Content-Length header is present,
/// it reads that many bytes; otherwise, it reads bytes until the connection is closed.
/// 
/// # Brief
/// Reads the body of an HTTP request from a TCP stream.
///
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the body will be read
/// - `request`: A mutable reference to an HTTP `Request` object where the body will be shared.
/// - `content_length`: The expcted Length of the request body in bytes.
///
/// # Return
/// Returns a `Result<(), Error>`:
/// - `Ok(())` on success,indicating the body has been read successfully.
/// - `Err(Error)` if there is an error during the read process.
/// 
/// # Error
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
        // content_length - bytes_read < body.len() < content_length : Error::ContentLengthMismatch
        // content_length - bytes_read >= body().len() & body.len() < content_length : continue to extend
        // (after extend) body.len() == content_length: break

        // Read up to 512 bytes at a time. (If the client only sent a small body, then only allocate
        // space to read that body )
        let mut buffer = vec![0_u8; min(512, content_length)];
        let bytes_read = stream
            .read(&mut buffer)
            .await
            .or_else(|err| Err(Error::ConnectionError(err)))?;

        // Make sure the client is still sending us bytes.(too few)
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
        // Store the received bytes in the request body.
        request.body_mut().extend_from_slice(&buffer[..bytes_read]);
    } 
    // get content fulled body length
    Ok(())
}

/// 
/// This function first reads the HTTP request headers using the `read_header` function.
/// 
/// # Brief
/// Reads an HTTP request from a TCP stream, including headers and optional body.
/// It will throw `Error::RequestBodyTooLarge` awary, when it receive too much content.
///
/// # Param
/// - `stream`: A mutable reference to a `TcpStream` from which the HTTP request will be read.
///
/// # Return
/// Return a `Result<http::Request<Vec<u8>>, Error>`
/// on success, contains on `http::Request` which the request headers and body.
/// on failure, it returns an `Error` indicating what went wrong during the read process.
/// - `http::Request<Vec<u8>>`: Retrieve the request from read_header(), then pass it as a parameter to read_body()
/// - `Err(Error)`: When read_header() or read_body() encounters an error, it should throw an Error to the caller.
/// 
/// # Error
/// - `Error::ConnectionError`: if there is an error reading from the TCP stream.
/// - `Error::ContentLengthMismatch`: If the server closes the connection before the entire body(as specifie by `Content-Length`) is read.(from read_header())
/// - `Error::MalformadRequest`: If the request received is not valid HTTP request according to the `parse_request`(from read_header())
/// - `Error::IncompleteRequest`: If the stream is closed before a compelte request is received. (from read_body())
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

///
/// This asynchronous function takes an `http::Request` object and writes its formatted.
/// components to the provided TCP stream. It writes the request, headers, and body(is present)
/// to the stream.
/// 
/// # Brief 
/// Writes an HTTP request to a TCP stream.
/// 
/// # Param
/// - `request`: A `http::Request` containing the HTTP request to be written to the stream. The body is 
/// represented as a `Vec<u8>`,
/// - `stream`: A mutable reference to a `TcpStream` to which the HTTP request will be written.
/// 
/// # Return
/// Returns a `Result<(), std::io::Error>` 
/// which, on success, is `Ok(())`. 
/// On failure, it returns an `std::io::Error` indicating what went wrong during the write process.
/// 
/// # Error
/// - `std::io::Error`: Indicates that an I/O error occurred while writing to the stream.
/// 
pub async fn write_to_stream(
    request: &http::Request<Vec<u8>>,
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

/// 
/// This function takes an `http::request<Vec<u8>>` object and construct a formatted string
/// representing the request line, which includes HTTP Version, status code and reason pharse.
///
/// # Brief
/// Provides a brief summary of what the function does. In this case, it's about 
/// fomatting request line of HTTP request into a string.
///
/// # Param
/// - `request`: An 'http::Request' object representing the HTTP request
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

/// 
/// This function appends to a header value (adding a new header if header is not already 
/// present). This is used to add the client's IP address to the end of the X-forwarded-For
/// list, or to add a new X-forwards-For header if one is not already present.
/// 
/// # Brief
///  Modifies or creates an HTTP header in the provided request.
/// 
/// # Param
/// - `request`: A mutable reference to an `http::Request<Vec<u8>>` object to modify.
/// - `name`: The name of the header to modify or crate.
/// - `extend_value`: The value to append or set for t he specified header.
/// 
/// # Return
/// Nothing
/// 
pub fn extend_header_value(request: &mut http::Request<Vec<u8>>, name: &'static str, extend_value: &str) {
    let new_value = match request.headers().get(name) {
        Some(existing_value) => {
            [existing_value.as_bytes(), b"," , extend_value.as_bytes()].concat()
        },
        None => extend_value.as_bytes().to_owned(),
    };
    request
        .headers_mut()
        .insert(name, http::HeaderValue::from_bytes(&new_value).unwrap());
}