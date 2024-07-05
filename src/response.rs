

/// # Brief
/// 
/// # Param
/// 
/// # Return
/// 
pub async fn read_from_stream(stream: &mut TcpStream) -> Result<http::Response<Vec<u8>>, Error> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_NUM_HEADERS];

    if !( request_method == http::Method::HEAD
        || response.status().as_u16() < 200
        || response.status() == http::StatusCode::NO_CONTENT
        || response.status() == http::StatusCode::NOT_MODIFIED
    ) 
    {
    }
}
