//! Thin blocking HTTP client. Sinks go through this so tests can point them
//! at a local listener and assert on the request.

use std::time::Duration;

pub struct Response {
    pub status: u16,
    pub body: String,
}

pub struct Client {
    agent: ureq::Agent,
}

impl Client {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            // Non-2xx is data here: Telegram returns retry_after in a 429 body.
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }

    pub fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<Response, String> {
        let response = self
            .agent
            .post(url)
            .send_json(body)
            .map_err(|err| format!("POST {url}: {err}"))?;
        read(response, url)
    }

    pub fn post_text(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> Result<Response, String> {
        let mut request = self.agent.post(url);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let response = request
            .send(body)
            .map_err(|err| format!("POST {url}: {err}"))?;
        read(response, url)
    }
}

fn read(mut response: ureq::http::Response<ureq::Body>, url: &str) -> Result<Response, String> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|err| format!("POST {url}: read body: {err}"))?;
    Ok(Response { status, body })
}

/// A local HTTP server for sink tests. Answers a fixed sequence of replies,
/// one per connection, and records every request.
#[cfg(test)]
pub mod mock {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    pub struct Request {
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: String,
    }

    pub struct Server {
        pub url: String,
        handle: JoinHandle<Vec<Request>>,
    }

    impl Server {
        pub fn respond(status: u16, reply_body: &'static str) -> Self {
            Self::respond_seq(vec![(status, reply_body)])
        }

        pub fn respond_seq(replies: Vec<(u16, &'static str)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let handle = std::thread::spawn(move || {
                replies
                    .into_iter()
                    .map(|(status, reply_body)| Self::serve_one(&listener, status, reply_body))
                    .collect()
            });
            Self { url, handle }
        }

        fn serve_one(listener: &TcpListener, status: u16, reply_body: &'static str) -> Request {
            {
                let (mut stream, _) = listener.accept().unwrap();
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let head_len = loop {
                    let n = stream.read(&mut buf).unwrap();
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(idx) = find(&raw, b"\r\n\r\n") {
                        break idx + 4;
                    }
                };
                let head = String::from_utf8_lossy(&raw[..head_len]).to_string();
                let mut lines = head.lines();
                let path = lines.next().unwrap().split(' ').nth(1).unwrap().to_string();
                let headers: Vec<(String, String)> = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
                    .collect();
                let content_len = headers
                    .iter()
                    .find(|(k, _)| k == "content-length")
                    .map(|(_, v)| v.parse::<usize>().unwrap())
                    .unwrap_or(0);
                while raw.len() < head_len + content_len {
                    let n = stream.read(&mut buf).unwrap();
                    raw.extend_from_slice(&buf[..n]);
                }
                let body =
                    String::from_utf8_lossy(&raw[head_len..head_len + content_len]).to_string();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply_body}",
                    reply_body.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
                Request {
                    path,
                    headers,
                    body,
                }
            }
        }

        pub fn request(self) -> Request {
            self.requests().remove(0)
        }

        pub fn requests(self) -> Vec<Request> {
            self.handle.join().unwrap()
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}
