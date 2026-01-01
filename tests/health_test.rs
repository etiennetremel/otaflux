//! Health endpoint integration tests.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use common::{create_app, MockRegistryBuilder};

#[tokio::test]
async fn test_health_endpoint_returns_ok() {
    let registry = MockRegistryBuilder::new().await.build().await;
    let app = create_app(registry.firmware_manager());

    let request = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::OK);
}
