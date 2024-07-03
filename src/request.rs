const MAX_BODY_SIZE: usize = 10000;

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
/// - `request`: 
///
/// # Return 
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