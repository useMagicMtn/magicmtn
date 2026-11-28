use magicmtn_sdk::{BoxFuture, SearchEngineBackend, SearchRequest, SearchResponse};
use serde_json::json;

#[test]
fn query_only_requests_have_portable_defaults_and_reject_unknown_options() {
    let request: SearchRequest = serde_json::from_value(json!({"query": "Rust async"}))
        .expect("a query is sufficient to construct a search request");
    assert_eq!(request, SearchRequest::new("Rust async"));
    assert_eq!(request.max_results, 10);
    assert!(request.include_domains.is_empty());
    assert!(request.exclude_domains.is_empty());
    assert!(request.search_queries.is_empty());
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"query": "Rust async", "max_results": 10})
    );
    assert!(serde_json::from_value::<SearchRequest>(json!({
        "query": "Rust async",
        "provider_option": true
    }))
    .is_err());
}

#[test]
fn keyword_queries_round_trip_as_optional_retrieval_hints() {
    let encoded = json!({
        "query": "Find the airlines flying nonstop between Seattle and Tokyo.",
        "max_results": 5,
        "search_queries": ["Seattle Tokyo nonstop flights", "Seattle Haneda Narita airlines"]
    });
    let request: SearchRequest = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(request.search_queries.len(), 2);
    assert_eq!(serde_json::to_value(request).unwrap(), encoded);
}

#[test]
fn search_contract_is_usable_through_a_backend_trait_object() {
    struct Search;

    impl SearchEngineBackend for Search {
        fn search<'a>(&'a self, request: &'a SearchRequest) -> BoxFuture<'a, SearchResponse> {
            Box::pin(async move {
                assert_eq!(request.include_domains, ["rust-lang.org"]);
                assert_eq!(request.exclude_domains, ["users.rust-lang.org"]);
                Ok(serde_json::from_value(json!({"results": [{
                    "url": "https://doc.rust-lang.org/book/",
                    "title": "The Rust Programming Language",
                    "content": "Learn to use async and await."
                }]}))?)
            })
        }
    }

    let backend: Box<dyn SearchEngineBackend> = Box::new(Search);
    let request: SearchRequest = serde_json::from_value(json!({
        "query": "Rust async",
        "max_results": 1,
        "include_domains": ["rust-lang.org"],
        "exclude_domains": ["users.rust-lang.org"]
    }))
    .expect("domain filters deserialize");
    let mut future = backend.search(&request);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(Ok(response)) = future.as_mut().poll(&mut context) else {
        panic!("the in-memory backend should immediately return results");
    };
    let encoded = serde_json::to_value(&response).expect("search responses serialize");
    let decoded: SearchResponse =
        serde_json::from_value(encoded).expect("search responses deserialize");
    assert_eq!(decoded, response);
    assert_eq!(decoded.results[0].url, "https://doc.rust-lang.org/book/");
}
