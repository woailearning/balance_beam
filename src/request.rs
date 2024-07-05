use http::request;
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
pub async fn get_content_length(request: &http::Request<Vec<u8>>) -> Result<Option<usize>, Error> {
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

/// # Beief
/// 
/// # Param
/// 
/// # Return
/// 
pub async fn read_header(stream: &mut TcpStream) -> Result<http::Response<Vec<u8>>, Error> {

}

/// 
pub async fn read_body(stream: &mut TcpStream) -> Result<(), Error> {

}

/// # Brief
/// 
/// # Param
/// 
/// # Return
/// 
pub async fn read_from_stream(stream: &mut TcpStream) -> Result<http::Response<Vec<u8>>, Error> {
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

// fn read_to_stream() -> {
// 
// }

/// #Beief 
/// This is a helper function that creates an http::Response containing an HTTP error 
/// that can be send a client.
/// 
/// # Param
/// -`status` 
/// 
/// # Return 
/// 
pub async fn make_http_error(status: http::StatusCode) -> http::Response<Vec<u8>> {
    let body = format!(
        "HTTP {}, {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
    ) .into_bytes();

    http::Response::builder()
        .status(status)
        .header("Content-Type", "test/plain")
        .header("Content-Length", body.len().to_string())
        .version(http::Version::HTTP_11)
        .body(body)
        .unwrap()
}


/// # Brief
/// 
/// # Param
/// - `response`
/// 
/// # Return
/// 
pub fn format_response_lines(response: &http::Response<Vec<u8>>) -> String {
    format!(
        "{:?} {} {}",
        response.version(),
        response.status().as_str(),
        response.status().canonical_reason().unwrap_or(""),
    )
}

