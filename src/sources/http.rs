//! Shared HTTP plumbing for the copyright sources.
//!
//! All four sources do the same thing: send a GET, stay inside a published
//! rate limit, back off when the server says it is being asked too often, and
//! decode JSON. Only the URLs, the headers, and the shapes differ, so that
//! machinery lives here once instead of four times.

use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

use crate::LookupError;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
/// Longest wait to honour before treating the source as spent. A server that
/// asks for more than this is telling us to come back another day, not to
/// sleep on it.
pub const DEFAULT_MAX_WAIT: u64 = 60;

/// A blocking JSON client that keeps one source inside its rate limit.
pub struct Http {
    /// The source's name, used to say who failed.
    label: &'static str,
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    /// Smallest gap to leave between two requests.
    min_interval: Duration,
    last_request: Option<Instant>,
    /// Whether a 403 means "slow down" rather than "not allowed".
    forbidden_is_throttling: bool,
    /// Longest `Retry-After` to sit through before giving up on the source.
    max_wait: u64,
}

impl Http {
    pub fn new(
        label: &'static str,
        user_agent: &str,
        min_interval: Duration,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the {label} lookup runtime: {error}"))?;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(user_agent)
            .build()
            .map_err(|error| format!("cannot create the {label} HTTP client: {error}"))?;
        Ok(Self {
            label,
            runtime,
            client,
            min_interval,
            last_request: None,
            forbidden_is_throttling: false,
            max_wait: DEFAULT_MAX_WAIT,
        })
    }

    /// How long a `Retry-After` may be before the source is treated as spent
    /// rather than merely busy.
    pub fn waiting_at_most(mut self, seconds: u64) -> Self {
        self.max_wait = seconds;
        self
    }

    /// Treat a 403 as throttling. The iTunes Search API answers 403 as well as
    /// 429 when it wants to be left alone; the keyed APIs mean it literally.
    pub fn forbidden_is_throttling(mut self) -> Self {
        self.forbidden_is_throttling = true;
        self
    }

    /// GET one JSON document.
    ///
    /// `Ok(None)` is a 404: the thing asked for is simply not there, which
    /// every caller here treats as "no copyright" rather than as a failure.
    pub fn get_json<T: DeserializeOwned>(
        &mut self,
        url: &str,
        query: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> Result<Option<T>, LookupError> {
        let Some(body) = self.get_text(url, query, headers)? else {
            return Ok(None);
        };
        // Some of these endpoints answer with text/javascript rather than
        // application/json, so the body is decoded and then parsed.
        serde_json::from_str(&body).map(Some).map_err(|error| {
            LookupError::Album(format!("{} returned malformed JSON: {error}", self.label))
        })
    }

    fn get_text(
        &mut self,
        url: &str,
        query: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> Result<Option<String>, LookupError> {
        for retry in 0..=MAX_RATE_LIMIT_RETRIES {
            self.wait_for_the_rate_limit();

            let mut request = self.client.get(url).query(query);
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            // The request is built out here but sent in there: reqwest arms
            // its timeout when the future is created, and a tokio timer can
            // only be created from inside the runtime.
            let response = self
                .runtime
                .block_on(async move { request.send().await })
                .map_err(|error| {
                    LookupError::Album(format!("{} request failed: {error}", self.label))
                })?;

            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            // A token does not repair itself, so every remaining album would
            // fail the same way. Spend no more requests on it.
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(LookupError::Exhausted(format!(
                    "{} rejected the token (HTTP 401); it is wrong or has expired",
                    self.label
                )));
            }

            let throttled = status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || (self.forbidden_is_throttling && status == reqwest::StatusCode::FORBIDDEN);
            if throttled {
                if retry == MAX_RATE_LIMIT_RETRIES {
                    return Err(LookupError::Exhausted(format!(
                        "{} kept throttling the lookup after {} attempts",
                        self.label,
                        MAX_RATE_LIMIT_RETRIES + 1
                    )));
                }
                let wait = retry_after(&response).unwrap_or(2 << retry).max(1);
                if wait > self.max_wait {
                    return Err(LookupError::Exhausted(format!(
                        "{} is rate limited for another {}, far past the {}-second limit \
                         this run will wait",
                        self.label,
                        readable(wait),
                        self.max_wait,
                    )));
                }
                eprintln!(
                    "{} throttled the lookup; waiting {wait}s before retrying...",
                    self.label
                );
                self.runtime
                    .block_on(async move { tokio::time::sleep(Duration::from_secs(wait)).await });
                continue;
            }

            if !status.is_success() {
                return Err(LookupError::Album(format!(
                    "{} lookup failed (HTTP {status})",
                    self.label
                )));
            }

            let body = response.text();
            return self.runtime.block_on(body).map(Some).map_err(|error| {
                LookupError::Album(format!(
                    "{} returned an unreadable response: {error}",
                    self.label
                ))
            });
        }

        unreachable!("the throttling loop always returns or continues")
    }

    /// Sleep just long enough that the published request rate is respected.
    fn wait_for_the_rate_limit(&mut self) {
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                let gap = self.min_interval - elapsed;
                self.runtime
                    .block_on(async move { tokio::time::sleep(gap).await });
            }
        }
        self.last_request = Some(Instant::now());
    }
}

/// A duration a person can read at a glance, for a wait worth complaining
/// about. `19302` means nothing; `5h 21m` explains itself.
pub fn readable(seconds: u64) -> String {
    match (seconds / 3600, (seconds % 3600) / 60, seconds % 60) {
        (0, 0, seconds) => format!("{seconds}s"),
        (0, minutes, seconds) => format!("{minutes}m {seconds}s"),
        (hours, minutes, _) => format!("{hours}h {minutes}m"),
    }
}

fn retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::sources::testing::{Server, ok, status};

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct Body {
        answer: String,
    }

    fn client() -> Http {
        Http::new("Test", "music-tag-transfer/test", Duration::from_millis(1)).unwrap()
    }

    #[test]
    fn decodes_a_body_and_sends_the_query_and_headers() {
        let server = Server::answering(&[r#"{"answer":"yes"}"#]);
        let mut http = client();

        let body: Option<Body> = http
            .get_json(
                &server.address,
                &[("term", "daft punk")],
                &[("Authorization", "token")],
            )
            .unwrap();

        assert_eq!(
            body,
            Some(Body {
                answer: "yes".to_owned()
            })
        );
        let asked = server.requests();
        assert_eq!(asked[0].target, "/?term=daft+punk");
        assert_eq!(asked[0].header("Authorization"), Some("token"));
        assert!(
            asked[0]
                .header("User-Agent")
                .is_some_and(|agent| agent.contains("music-tag-transfer"))
        );
    }

    #[test]
    fn a_missing_document_is_an_absence_rather_than_a_failure() {
        let server = Server::replying(&[status("404 Not Found", "")]);
        let mut http = client();

        let body: Option<Body> = http.get_json(&server.address, &[], &[]).unwrap();

        assert_eq!(body, None);
        server.requests();
    }

    #[test]
    fn a_rejected_token_says_so_instead_of_retrying() {
        let server = Server::replying(&[status("401 Unauthorized", "")]);
        let mut http = client();

        let error = http
            .get_json::<Body>(&server.address, &[], &[])
            .expect_err("a 401 must fail");

        // A dead token is the source's problem, not this album's: it must be
        // reported as exhaustion so the run stops asking.
        assert!(matches!(error, LookupError::Exhausted(_)), "{error}");
        assert!(error.to_string().contains("expired"), "{error}");
        // One attempt only: retrying a bad token just burns the rate limit.
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn throttling_is_retried_and_then_succeeds() {
        let server = Server::replying(&[
            status("429 Too Many Requests", ""),
            ok(r#"{"answer":"eventually"}"#),
        ]);
        let mut http = client();

        let body: Option<Body> = http.get_json(&server.address, &[], &[]).unwrap();

        assert_eq!(
            body,
            Some(Body {
                answer: "eventually".to_owned()
            })
        );
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn a_403_is_throttling_only_where_the_source_says_so() {
        let server = Server::replying(&[status("403 Forbidden", "")]);
        let mut http = client();
        let error = http
            .get_json::<Body>(&server.address, &[], &[])
            .expect_err("a plain 403 must fail");
        // A plain 403 is this request's failure, not the source giving up.
        assert!(matches!(error, LookupError::Album(_)), "{error}");
        assert!(error.to_string().contains("HTTP 403"), "{error}");
        assert_eq!(server.requests().len(), 1);

        let server = Server::replying(&[
            status("403 Forbidden", ""),
            ok(r#"{"answer":"eventually"}"#),
        ]);
        let mut http = client().forbidden_is_throttling();
        assert!(
            http.get_json::<Body>(&server.address, &[], &[])
                .unwrap()
                .is_some()
        );
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn a_body_that_is_not_the_expected_shape_is_reported_as_malformed() {
        let server = Server::answering(&["not json at all"]);
        let mut http = client();

        let error = http
            .get_json::<Body>(&server.address, &[], &[])
            .expect_err("garbage must not parse");

        assert!(error.to_string().contains("malformed JSON"), "{error}");
        server.requests();
    }

    #[test]
    fn requests_are_spaced_by_the_rate_limit() {
        let server = Server::answering(&[r#"{"answer":"one"}"#, r#"{"answer":"two"}"#]);
        let mut http = Http::new(
            "Test",
            "music-tag-transfer/test",
            Duration::from_millis(300),
        )
        .unwrap();

        let started = Instant::now();
        http.get_json::<Body>(&server.address, &[], &[]).unwrap();
        http.get_json::<Body>(&server.address, &[], &[]).unwrap();

        // The first request goes out at once; only the second waits.
        assert!(started.elapsed() >= Duration::from_millis(300), "too quick");
        server.requests();
    }

    /// The failure from a real library scan: Spotify answered 429 with a
    /// Retry-After of five hours. That is the source giving up on the run, not
    /// one album failing, and it must be reported as such so the caller can
    /// stop instead of asking hundreds more times.
    #[test]
    fn a_wait_beyond_the_budget_retires_the_source() {
        let mut throttled = status("429 Too Many Requests", "");
        throttled = throttled.replace("\r\nConnection:", "\r\nRetry-After: 19302\r\nConnection:");
        let server = Server::replying(&[throttled]);
        let mut http = client();

        let error = http
            .get_json::<Body>(&server.address, &[], &[])
            .expect_err("a five-hour wait must not be sat through");

        assert!(matches!(error, LookupError::Exhausted(_)), "{error}");
        let message = error.to_string();
        // The number of seconds means nothing to a person reading a log.
        assert!(message.contains("5h 21m"), "{message}");
        // Crucially: one request, not a retry storm.
        assert_eq!(server.requests().len(), 1);
    }

    /// The budget is a policy, not a constant: a caller willing to wait can.
    #[test]
    fn a_raised_budget_sits_through_the_wait_instead() {
        let mut throttled = status("429 Too Many Requests", "");
        throttled = throttled.replace("\r\nConnection:", "\r\nRetry-After: 1\r\nConnection:");
        let server = Server::replying(&[throttled, ok(r#"{"answer":"waited"}"#)]);
        let mut http = client().waiting_at_most(5);

        let body: Option<Body> = http.get_json(&server.address, &[], &[]).unwrap();

        assert_eq!(
            body,
            Some(Body {
                answer: "waited".to_owned()
            })
        );
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn persistent_throttling_retires_the_source_too() {
        let throttled = status("429 Too Many Requests", "");
        let server = Server::replying(&[
            throttled.clone(),
            throttled.clone(),
            throttled.clone(),
            throttled,
        ]);
        let mut http = client();

        let error = http
            .get_json::<Body>(&server.address, &[], &[])
            .expect_err("a source that only ever throttles is spent");

        assert!(matches!(error, LookupError::Exhausted(_)), "{error}");
        server.requests();
    }

    #[test]
    fn spells_a_wait_the_way_a_person_reads_one() {
        assert_eq!(readable(45), "45s");
        assert_eq!(readable(90), "1m 30s");
        assert_eq!(readable(19302), "5h 21m");
    }
}
