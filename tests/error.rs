use crate::error::LarpshellError;

#[test]
fn from_http_status_rounds_retry_after_up() {
    let err = LarpshellError::from_http_status(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "gemini",
        "rate limited, retry in 1.5s",
    );

    assert!(matches!(
        err,
        LarpshellError::RateLimitExceeded {
            retry_after: Some(2)
        }
    ));
}

#[test]
fn display_examples() {
    let err = LarpshellError::ConnectionFailed {
        provider: "ollama".into(),
        message: "cannot connect".into(),
    };
    assert_eq!(
        err.to_string(),
        "failed to connect to ollama: cannot connect"
    );

    let err = LarpshellError::RateLimitExceeded {
        retry_after: Some(10),
    };
    assert_eq!(
        err.to_string(),
        "rate limit exceeded; retry after 10 seconds"
    );

    let err = LarpshellError::RateLimitExceeded { retry_after: None };
    assert_eq!(
        err.to_string(),
        "rate limit exceeded; please try again later"
    );

    let err = LarpshellError::AgentMaxIterations(10);
    assert_eq!(
        err.to_string(),
        "agent reached maximum iterations (10) without producing a final response"
    );
}
