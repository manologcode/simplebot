use simplebot::config::ProviderConfig;
use simplebot::provider::openai::OpenAiProvider;
use simplebot::provider::Provider;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn read_request(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).unwrap();
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buffer);
        if let Some(header_end) = text.find("\r\n\r\n") {
            let headers = &text[..header_end];
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buffer.len() >= header_end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

fn write_response(stream: &mut TcpStream, status: &str, extra_headers: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra_headers}Connection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}

fn provider_config(port: u16) -> ProviderConfig {
    ProviderConfig {
        base_url: format!("http://127.0.0.1:{port}/v1"),
        model: "mock-model".to_string(),
        timeout_secs: 5,
        ..ProviderConfig::default()
    }
}

#[test]
fn talks_to_openai_compatible_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        assert!(request.contains("Authorization: Bearer test-key"));
        assert!(request.contains("x-opencode-session: test-session"));
        assert!(request.contains("\"model\":\"mock-model\""));

        let body = r#"{"choices":[{"message":{"content":"hola desde el mock"}}],"usage":{"total_tokens":7}}"#;
        write_response(&mut stream, "200 OK", "", body);
    });

    let provider = OpenAiProvider::new(
        &provider_config(port),
        "test-key".to_string(),
        "test-session".to_string(),
    );
    let turn = provider.complete(&[], &[]).unwrap();
    assert_eq!(turn.content.as_deref(), Some("hola desde el mock"));
    server.join().unwrap();
}

#[test]
fn retries_after_rate_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        read_request(&mut first);
        write_response(
            &mut first,
            "429 Too Many Requests",
            "Retry-After: 0\r\n",
            r#"{"error":{"type":"server_error","message":"Endpoint is unavailable."}}"#,
        );

        let (mut second, _) = listener.accept().unwrap();
        read_request(&mut second);
        let body = r#"{"choices":[{"message":{"content":"recuperado"}}]}"#;
        write_response(&mut second, "200 OK", "", body);
    });

    let provider = OpenAiProvider::new(
        &provider_config(port),
        "test-key".to_string(),
        "test-session".to_string(),
    );
    let turn = provider.complete(&[], &[]).unwrap();
    assert_eq!(turn.content.as_deref(), Some("recuperado"));
    server.join().unwrap();
}
