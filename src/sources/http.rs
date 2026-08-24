//! Shared HTTP plumbing for the copyright sources.
//!
//! All four sources do the same thing: send a GET, stay inside a published
//! rate limit, back off when the server says it is being asked too often, and
//! decode JSON. Only the URLs, the headers, and the shapes differ, so that
//! machinery lives here once instead of four times.

use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

use crate::LookupError;

/// How long to let one request hang before calling it a timeout.
///
/// Generous, because MusicBrainz in particular does answer slowly under load
/// and a timeout that fires early turns a slow success into a retried failure.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How many times to wait out a throttled request before giving up on that
/// album.
///
/// Generous, because throttling passes: the waits are seconds and the album
/// after this one will very likely go through. Running out is a reason to skip
/// one album, never a reason to stop.
pub const DEFAULT_MAX_THROTTLE_RETRIES: u32 = 30;
/// Longest wait to honour before treating the source as spent. A server that
/// asks for more than this is telling us to come back another day, not to
/// sleep on it.
pub const DEFAULT_MAX_WAIT: u64 = 60;
/// How many times to try one request before giving up on that album.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 5;
/// Longest backoff between attempts at a request that keeps failing.
const MAX_BACKOFF: u64 = 30;
/// How many albums may give up in a row before the source is presumed
/// unreachable.
///
/// One album timing out says nothing; a run of them says the network is down
/// or the catalogue is refusing everyone, and grinding through a whole library
/// at five attempts each would take hours to discover that.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

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
    /// How many times to try one request before giving up on that album.
    max_attempts: u32,
    /// How many times to wait out throttling before giving up on that album.
    max_throttle_retries: u32,
    /// Requests that have given up since the last success.
    consecutive_failures: u32,
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
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            max_throttle_retries: DEFAULT_MAX_THROTTLE_RETRIES,
            consecutive_failures: 0,
        })
    }

    /// How long a `Retry-After` may be before the source is treated as spent
    /// rather than merely busy.
    pub fn waiting_at_most(mut self, seconds: u64) -> Self {
        self.max_wait = seconds;
        self
    }

    /// How many times to try one request before giving up on that album.
    pub fn attempting_at_most(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts.max(1);
        self
    }

    /// How many times to wait out throttling before giving up on that album.
    pub fn waiting_out_throttling(mut self, retries: u32) -> Self {
        self.max_throttle_retries = retries.max(1);
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
        // Two ways to fail, counted separately. Throttling is the server
        // telling us to wait, and is answered by waiting. Everything else --
        // a timeout, a refused connection, a 502 from a load balancer -- is
        // transient by nature, and answered by trying again a little later.
        let mut throttles = 0u32;
        let mut attempts = 0u32;

        loop {
            self.wait_for_the_rate_limit();

            let mut request = self.client.get(url).query(query);
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            attempts += 1;
            // The request is built out here but sent in there: reqwest arms
            // its timeout when the future is created, and a tokio timer can
            // only be created from inside the runtime.
            let sent = self.runtime.block_on(async move { request.send().await });

            let response = match sent {
                Ok(response) => response,
                Err(error) => {
                    // A timeout or a dropped connection. Neither says anything
                    // about the album, so it is worth asking again.
                    if attempts >= self.max_attempts {
                        return Err(self.gave_up(&format!(
                            "{} request failed after {attempts} attempts: {error}",
                            self.label
                        )));
                    }
                    self.back_off(attempts, &format!("{error}"));
                    continue;
                }
            };

            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                self.succeeded();
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

            if self.is_throttling(status) {
                throttles += 1;
                // Running out of patience with one album says nothing about
                // the next: throttling passes, and the album after this one
                // will very likely go through. So this gives up on the album
                // and leaves the source in play.
                if throttles > self.max_throttle_retries {
                    return Err(LookupError::Album(format!(
                        "{} kept throttling through all {} retries",
                        self.label, self.max_throttle_retries
                    )));
                }
                let wait = retry_after(&response)
                    .unwrap_or_else(|| (1u64 << throttles.min(6)).min(MAX_BACKOFF))
                    .max(1);
                // A wait measured in hours cannot be sat through, and no
                // number of retries will shorten it. That is the one case
                // where the source is set aside -- the scan still finishes,
                // it simply stops asking this catalogue.
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
                    "{} throttled the lookup; waiting {wait}s before retry {throttles} of {}...",
                    self.label, self.max_throttle_retries
                );
                self.sleep(wait);
                continue;
            }

            // A server error is the catalogue having a bad moment, not an
            // answer about this album.
            if status.is_server_error() {
                if attempts >= self.max_attempts {
                    return Err(self.gave_up(&format!(
                        "{} answered HTTP {status} on all {attempts} attempts",
                        self.label
                    )));
                }
                self.back_off(attempts, &format!("HTTP {status}"));
                continue;
            }

            if !status.is_success() {
                return Err(LookupError::Album(format!(
                    "{} lookup failed (HTTP {status})",
                    self.label
                )));
            }

            let body = response.text();
            let text = self.runtime.block_on(body).map_err(|error| {
                LookupError::Album(format!(
                    "{} returned an unreadable response: {error}",
                    self.label
                ))
            })?;
            self.succeeded();
            return Ok(Some(text));
        }
    }

    /// Whether a status means "slow down" rather than "here is your answer".
    ///
    /// MusicBrainz answers a breached rate limit with 503 rather than 429, so
    /// a plain reading of the status would file its throttling under "the
    /// server is broken" and never wait at all.
    fn is_throttling(&self, status: reqwest::StatusCode) -> bool {
        status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
            || (self.forbidden_is_throttling && status == reqwest::StatusCode::FORBIDDEN)
    }

    /// Wait a little longer after each failed attempt, and say why.
    fn back_off(&mut self, attempts: u32, reason: &str) {
        let wait = (1u64 << attempts.min(6)).min(MAX_BACKOFF);
        eprintln!(
            "{} attempt {attempts} failed ({reason}); retrying in {wait}s...",
            self.label
        );
        self.sleep(wait);
    }

    fn sleep(&self, seconds: u64) {
        self.runtime
            .block_on(async move { tokio::time::sleep(Duration::from_secs(seconds)).await });
    }

    /// Give up on one album, and on the source once enough have gone that way.
    fn gave_up(&mut self, message: &str) -> LookupError {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            return LookupError::Exhausted(format!(
                "{message}. That is {} in a row, so {} is being treated as unreachable rather \
                 than retried for every album left",
                self.consecutive_failures, self.label
            ));
        }
        LookupError::Album(message.to_owned())
    }

    /// A request got through, so the run of failures is over.
    fn succeeded(&mut self) {
        self.consecutive_failures = 0;
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
        Http::new("Test", "music-tag-transfer/test", Duration::from_millis(1))
            .unwrap()
            // The default of thirty would make a throttling test crawl.
            .waiting_out_throttling(3)
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

    /// Throttling that never clears costs one album, not the run. The source
    /// keeps answering, because throttling passes and the next album will very
    /// likely go through.
    #[test]
    fn throttling_that_never_clears_only_costs_that_album() {
        let throttled = status("429 Too Many Requests", "");
        let server = Server::replying(&[throttled.clone(), throttled.clone(), throttled]);
        let mut http = client().waiting_out_throttling(2);

        let error = http
            .get_json::<Body>(&server.address, &[], &[])
            .expect_err("it never stopped throttling");

        assert!(matches!(error, LookupError::Album(_)), "{error}");
        assert!(error.to_string().contains("all 2 retries"), "{error}");
        // The retries were actually spent before giving up.
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn spells_a_wait_the_way_a_person_reads_one() {
        assert_eq!(readable(45), "45s");
        assert_eq!(readable(90), "1m 30s");
        assert_eq!(readable(19302), "5h 21m");
    }

    /// Retries are quick in a test, so the backoff does not dominate.
    fn quick() -> Http {
        Http::new("Test", "music-tag-transfer/test", Duration::from_millis(1))
            .unwrap()
            .attempting_at_most(3)
    }

    /// The failure that prompted this: MusicBrainz answers a breached rate
    /// limit with 503, not 429. Read literally that is "the server is broken",
    /// and the request would never wait at all.
    #[test]
    fn a_503_is_throttling_and_is_waited_out() {
        let mut throttled = status("503 Service Unavailable", "");
        throttled = throttled.replace("\r\nConnection:", "\r\nRetry-After: 1\r\nConnection:");
        let server = Server::replying(&[throttled, ok(r#"{"answer":"waited"}"#)]);
        let mut http = quick();

        let body: Option<Body> = http.get_json(&server.address, &[], &[]).unwrap();

        assert_eq!(
            body,
            Some(Body {
                answer: "waited".to_owned()
            })
        );
        assert_eq!(server.requests().len(), 2);
    }

    /// A 500 is the catalogue having a bad moment, not an answer about this
    /// album, so it is worth asking again.
    #[test]
    fn a_server_error_is_retried_and_then_succeeds() {
        let server = Server::replying(&[
            status("500 Internal Server Error", ""),
            ok(r#"{"answer":"second time"}"#),
        ]);
        let mut http = quick();

        let body: Option<Body> = http.get_json(&server.address, &[], &[]).unwrap();

        assert_eq!(
            body,
            Some(Body {
                answer: "second time".to_owned()
            })
        );
        assert_eq!(server.requests().len(), 2);
    }

    /// After the attempts run out the album is given up on -- an Album error,
    /// so the scan moves to the next album rather than ending the run.
    #[test]
    fn giving_up_on_one_album_does_not_end_the_run() {
        let broken = status("502 Bad Gateway", "");
        let server = Server::replying(&[broken.clone(), broken.clone(), broken]);
        let mut http = quick();

        let error = http
            .get_json::<Body>(&server.address, &[], &[])
            .expect_err("three server errors is enough");

        assert!(matches!(error, LookupError::Album(_)), "{error}");
        assert!(error.to_string().contains("all 3 attempts"), "{error}");
        assert_eq!(server.requests().len(), 3);
    }

    /// A connection that is refused outright is the same kind of transient
    /// failure as a timeout, and is retried the same way.
    #[test]
    fn an_unreachable_host_is_retried_then_given_up_on() {
        // A port nothing is listening on: every attempt fails at connect.
        let mut http = quick();
        let error = http
            .get_json::<Body>("http://127.0.0.1:1", &[], &[])
            .expect_err("nothing is listening there");

        assert!(error.to_string().contains("after 3 attempts"), "{error}");
    }

    /// One album failing says nothing; a run of them says the source is gone,
    /// and grinding through a whole library at five attempts each would take
    /// hours to discover that.
    #[test]
    fn a_run_of_failures_retires_the_source() {
        let mut http = quick();
        let mut last = None;
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            last = Some(
                http.get_json::<Body>("http://127.0.0.1:1", &[], &[])
                    .expect_err("nothing is listening there"),
            );
        }

        let error = last.unwrap();
        assert!(matches!(error, LookupError::Exhausted(_)), "{error}");
        assert!(error.to_string().contains("in a row"), "{error}");
    }

    /// A success clears the count, so an occasional blip never accumulates
    /// into a false verdict that the catalogue is down.
    #[test]
    fn a_success_forgives_the_earlier_failures() {
        let mut http = quick();
        for _ in 0..MAX_CONSECUTIVE_FAILURES - 1 {
            let _ = http.get_json::<Body>("http://127.0.0.1:1", &[], &[]);
        }

        let server = Server::answering(&[r#"{"answer":"fine"}"#]);
        assert!(
            http.get_json::<Body>(&server.address, &[], &[])
                .unwrap()
                .is_some()
        );
        server.requests();

        // Back to zero: the next failure is one album's problem, not the
        // source's.
        let error = http
            .get_json::<Body>("http://127.0.0.1:1", &[], &[])
            .expect_err("nothing is listening there");
        assert!(matches!(error, LookupError::Album(_)), "{error}");
    }

    /// A 404 counts as a success for this purpose: the server answered.
    #[test]
    fn a_missing_document_also_clears_the_failure_count() {
        let mut http = quick();
        for _ in 0..MAX_CONSECUTIVE_FAILURES - 1 {
            let _ = http.get_json::<Body>("http://127.0.0.1:1", &[], &[]);
        }

        let server = Server::replying(&[status("404 Not Found", "")]);
        assert!(
            http.get_json::<Body>(&server.address, &[], &[])
                .unwrap()
                .is_none()
        );
        server.requests();

        let error = http
            .get_json::<Body>("http://127.0.0.1:1", &[], &[])
            .expect_err("nothing is listening there");
        assert!(matches!(error, LookupError::Album(_)), "{error}");
    }

    /// A 4xx that is not throttling is a real answer about this request, and
    /// retrying it would only waste the rate limit.
    #[test]
    fn a_client_error_is_not_retried() {
        let server = Server::replying(&[status("400 Bad Request", "")]);
        let mut http = quick();

        let error = http
            .get_json::<Body>(&server.address, &[], &[])
            .expect_err("a 400 is an answer, not a blip");

        assert!(matches!(error, LookupError::Album(_)), "{error}");
        assert_eq!(server.requests().len(), 1);
    }
}
