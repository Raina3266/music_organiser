//! A local HTTP server for the source tests.
//!
//! None of the four catalogues can be reached from a test run, so what is
//! checked here is the half this crate is responsible for: the URL each source
//! asks for, the credential it sends, and what it does with the answer.

use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    thread,
};

/// One request the server was sent.
pub struct Request {
    /// The path and query, as asked for.
    pub target: String,
    headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A server that answers a scripted sequence of responses and remembers what
/// it was asked.
///
/// It stops listening once the script runs out, so a client that sends more
/// requests than expected fails rather than hanging.
pub struct Server {
    pub address: String,
    handle: thread::JoinHandle<Vec<Request>>,
}

impl Server {
    /// Answer each body in turn with 200 OK.
    pub fn answering(bodies: &[&str]) -> Self {
        Self::replying(&bodies.iter().map(|body| ok(body)).collect::<Vec<_>>())
    }

    /// Answer with raw responses, for the statuses [`ok`] cannot express.
    pub fn replying(responses: &[String]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let script: Vec<String> = responses.to_vec();
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for response in script {
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                requests.push(answer(stream, &response));
            }
            requests
        });
        Self { address, handle }
    }

    /// Everything the server was asked, once the client is done with it.
    pub fn requests(self) -> Vec<Request> {
        self.handle.join().unwrap()
    }
}

/// A 200 response carrying `body`.
pub fn ok(body: &str) -> String {
    status("200 OK", body)
}

/// A response with any status line, such as `404 Not Found`.
pub fn status(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Read one request, reply, and hand back what was asked.
fn answer(mut stream: TcpStream, response: &str) -> Request {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }

    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();

    Request {
        target: request_line.split_whitespace().nth(1).unwrap().to_owned(),
        headers,
    }
}
