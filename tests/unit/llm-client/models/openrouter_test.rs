use super::*;

#[tokio::test]
async fn fetch_error_redacts_url_and_response_credentials() {
    let home = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", mockito::Matcher::Any)
        .with_status(400)
        .with_header("x-api-key", "SENTINEL-ECHO-HEADER")
        .with_body("SENTINEL-USERINFO SENTINEL-KEY SENTINEL-ECHO-HEADER KEEP-BODY")
        .create_async()
        .await;
    let url = format!(
        "{}/models?key=SENTINEL-KEY",
        server.url().replace("http://", "http://SENTINEL-USERINFO@")
    );
    let catalog =
        OpenRouterCatalog::with_url_and_cache(url, Some(home.path().join("catalog.json")));
    let error = catalog.fetch().await.unwrap_err();
    let full = format!("{error}\n{error:?}");
    if let Some(out) = std::env::var_os("XIAOO_ERROR_TEST_EVIDENCE") {
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(
            std::path::Path::new(&out).join("openrouter_catalog_credentials.txt"),
            &full,
        )
        .unwrap();
    }
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    for secret in ["SENTINEL-USERINFO", "SENTINEL-KEY", "SENTINEL-ECHO-HEADER"] {
        assert!(!full.contains(secret));
    }
    assert!(full.contains("HTTP 400"));
    assert!(full.contains("KEEP-BODY"));
}
