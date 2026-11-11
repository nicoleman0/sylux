use axum::{
    body::Body,
    http::{Request, Response, StatusCode, header},
};

include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

/// Serve only files captured from `web/dist` at compile time.
pub async fn serve(request: Request<Body>) -> Response<Body> {
    let path = request.uri().path();
    let key = match path {
        "/" => "index.html",
        _ => path.strip_prefix('/').unwrap_or(path),
    };
    let Some((_, bytes, content_type)) = WEB_ASSETS.iter().find(|(name, _, _)| *name == key) else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("valid not-found response");
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, *content_type)
        .body(Body::from(*bytes))
        .expect("valid static asset response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_index_with_html_content_type() {
        let request = Request::builder().uri("/").body(Body::empty()).unwrap();
        let response = serve(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn missing_asset_returns_not_found() {
        let request = Request::builder()
            .uri("/missing.js")
            .body(Body::empty())
            .unwrap();
        assert_eq!(serve(request).await.status(), StatusCode::NOT_FOUND);
    }
}
