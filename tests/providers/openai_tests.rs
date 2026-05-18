//! Integration tests for `open_interceptor::providers::openai`.

use open_interceptor::providers::openai::build_upstream_url;

#[test]
fn upstream_url_concat() {
    assert_eq!(
        build_upstream_url("https://opencode.ai/zen/go/v1"),
        "https://opencode.ai/zen/go/v1/chat/completions"
    );
    assert_eq!(
        build_upstream_url("https://api.openai.com/v1/"),
        "https://api.openai.com/v1/chat/completions"
    );
}
