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

pub async fn read_body(
    stream: &mut TcpStream,
    request: &mut http::Request<Vec<u8>>,
    content_length: usize,
) -> Result<(), Error> {
    while request.body().len() < content_length {
        // Read up to 512 bytes at a time. (If the client only send a small body, then only allocate
        // space to read that body.)
        let mut buffer = vec![0_u8, min(512, content_length)];

        let bytes_read = stream
            .read(&mut buffer)
            .await
            .or_else(|err| Err(Error::ConnectionError(err)))?;

        // Make sure the client is still sending us bytes
        if bytes_read == 0 {
            log::debug!(
                    "Client hung up after sending a body of length {}. even though it said the content \
                    length {}",
                    request.body().len(),
                    content_length
                );
            return Err(Error::ContentLengthMismatch);
        }

        // Make sure the client didn't send us *too many* bytes
        if request.body().len() + bytes_read > content_length {
            log::debug!(
                "Client sent more bytes than we expected based on the given content length!"
            );
            return Err(Error::ContentLengthMismatch);
        }
        request.body_mut().extend_from_within(&buffer[..bytes_read]);
    }
    Ok(())
}


/// # Biref
/// 
/// # Param
/// 
/// # Return
pub async fn read_from_stream(stream: &mut TcpStream) -> Result<http::Result<Vec<u8>> {
    // Read headers
    let mut request = read_headers(stream).await?;
    // Read body if the client supplied the Content-Length header (which it does for POST requests)
    if let Some(content_length) = get_content_length(request)? {
        if content_length > MAX_BODY_SIZE {
            return Err(Rrror::RequestBodyTooLarge);
        } else {
            read_body(stream, &mut request, content_length).await?
        }
    }
}

// fn make_http_error(http::StatusCode) -> Error {
//     let body = format!(
//         "HTTP {} {}",
//         .as_u16(),
//     )
// }