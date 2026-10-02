//! A machine's web UI as a browser drives it: raw requests, with or
//! without the cookie that opens it, pages, form posts, the configuration
//! editor's previews, and the event stream of the group's pages.

use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use serde_json::Value;

use super::{GROUP, Machine};

/// What a raw HTTP request got back.
pub struct Answer {
    pub status: u16,
    pub location: Option<String>,
    pub cookie: Option<String>,
    pub body: String,
}

/// A client that follows no redirect and takes every status as an answer.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into()
}

impl Machine {
    /// The URL of `path` on this machine's server.
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.running().address)
    }

    /// The token the API asks for.
    pub fn token(&self) -> String {
        self.running().token.clone()
    }

    /// The cookie a browser holds once it opened the web UI.
    pub fn cookie(&self) -> (&'static str, String) {
        ("cookie", format!("pigeon_token={}", self.token()))
    }

    /// Sends one request to the server, following no redirect.
    pub async fn send(
        &self,
        method: &'static str,
        path: &str,
        headers: Vec<(&'static str, String)>,
        body: Option<(String, Vec<u8>)>,
    ) -> Answer {
        let url = self.url(path);
        tokio::task::spawn_blocking(move || {
            let mut request = ureq::http::Request::builder().method(method).uri(&url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let mut response = match body {
                Some((kind, bytes)) => agent()
                    .run(request.header("content-type", kind).body(bytes).unwrap())
                    .unwrap(),
                None => agent().run(request.body(()).unwrap()).unwrap(),
            };
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .map(|value| value.to_str().unwrap().to_owned())
            };
            Answer {
                status: response.status().as_u16(),
                location: header("location"),
                cookie: header("set-cookie"),
                body: response.body_mut().read_to_string().unwrap(),
            }
        })
        .await
        .unwrap()
    }

    /// The page at `path`, which must be there.
    pub async fn page(&self, path: &str) -> String {
        let answer = self.send("GET", path, vec![self.cookie()], None).await;
        assert_eq!(answer.status, 200, "{path}");
        answer.body
    }

    /// Posts a web form of `fields` to `/act/<action>`, as a browser does.
    pub async fn post_form(&self, action: &str, fields: &[(&str, &str)]) -> Answer {
        self.post_upload(action, fields, None).await
    }

    /// Posts a web form of `fields` and, when given, the file `(name,
    /// content)` chosen in it, to `/act/<action>`.
    pub async fn post_upload(
        &self,
        action: &str,
        fields: &[(&str, &str)],
        file: Option<(&str, &[u8])>,
    ) -> Answer {
        self.post_multipart(action, fields, file, Vec::new()).await
    }

    /// Posts a web form of `fields` to `/act/<action>` with the headers
    /// `extra` besides the cookie, as a page of another site would.
    pub async fn post_form_with(
        &self,
        action: &str,
        fields: &[(&str, &str)],
        extra: Vec<(&'static str, String)>,
    ) -> Answer {
        self.post_multipart(action, fields, None, extra).await
    }

    async fn post_multipart(
        &self,
        action: &str,
        fields: &[(&str, &str)],
        file: Option<(&str, &[u8])>,
        extra: Vec<(&'static str, String)>,
    ) -> Answer {
        let boundary = "pigeonboundary";
        let mut form = Vec::new();
        for (name, value) in fields {
            form.extend(
                format!(
                    "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .bytes(),
            );
        }
        if let Some((name, content)) = file {
            form.extend(
                format!(
                    "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"; filename=\"upload\"\r\ncontent-type: application/octet-stream\r\n\r\n"
                )
                .bytes(),
            );
            form.extend(content);
            form.extend(b"\r\n");
        }
        form.extend(format!("--{boundary}--\r\n").bytes());
        let kind = format!("multipart/form-data; boundary={boundary}");
        self.send(
            "POST",
            &format!("/act/{action}"),
            [vec![self.cookie()], extra].concat(),
            Some((kind, form)),
        )
        .await
    }

    /// What the group's configuration editor shows of `text`.
    pub async fn preview(&self, text: &str) -> (u16, Value) {
        let url = self.url(&format!("/g/{GROUP}/config/preview"));
        let (cookie, text) = (self.cookie(), text.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut response = agent()
                .post(&url)
                .header(cookie.0, cookie.1)
                .send_form([("text", text)])
                .unwrap();
            let status = response.status().as_u16();
            (status, response.body_mut().read_json().unwrap())
        })
        .await
        .unwrap()
    }

    /// Opens the event stream of the group's pages, checks that it is one
    /// and that it first names the program serving it, and returns its
    /// lines, lowercased.
    pub async fn listen(&self) -> Receiver<String> {
        let (sender, lines) = std::sync::mpsc::channel();
        let (address, cookie) = (self.running().address, self.cookie());
        let request = format!(
            "GET /g/{GROUP}/events HTTP/1.1\r\nhost: {address}\r\n{}: {}\r\n\r\n",
            cookie.0, cookie.1
        );
        std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else {
                    return;
                };
                if sender.send(line.to_lowercase()).is_err() {
                    return;
                }
            }
        });
        tokio::task::spawn_blocking(move || {
            assert_eq!(next_line(&lines), "http/1.1 200 ok");
            let mut kind = String::new();
            loop {
                let line = next_line(&lines);
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("content-type: ") {
                    value.clone_into(&mut kind);
                }
            }
            assert_eq!(kind, "text/event-stream");
            while next_line(&lines) != "event: program" {}
            let program = next_line(&lines);
            assert!(
                program
                    .strip_prefix("data: ")
                    .is_some_and(|hash| hash.len() == 64),
                "{program}"
            );
            lines
        })
        .await
        .unwrap()
    }
}

/// The next line of an event stream.
fn next_line(lines: &Receiver<String>) -> String {
    lines.recv_timeout(Duration::from_secs(20)).unwrap()
}

/// Waits for the next event of a stream.
pub async fn next_event(lines: Receiver<String>) -> Receiver<String> {
    tokio::task::spawn_blocking(move || {
        while !next_line(&lines).starts_with("data:") {}
        lines
    })
    .await
    .unwrap()
}
