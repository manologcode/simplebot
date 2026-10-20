use simplebot::config::ProviderConfig;
use simplebot::provider::openai::OpenAiProvider;
use simplebot::provider::Provider;
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn talks_to_openai_compatible_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
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
        let request = String::from_utf8_lossy(&buffer);
        assert!(request.contains("Authorization: Bearer test-key"));
        assert!(request.contains("x-opencode-session: test-session"));
        assert!(request.contains("\"model\":\"mock-model\""));

        let body = r#"{"choices":[{"message":{"content":"hola desde el mock"}}],"usage":{"total_tokens":7}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let config = ProviderConfig {
        base_url: format!("http://127.0.0.1:{port}/v1"),
        model: "mock-model".to_string(),
        timeout_secs: 5,
        ..ProviderConfig::default()
    };
    let provider = OpenAiProvider::new(&config, "test-key".to_string(), "test-session".to_string());
    let turn = provider.complete(&[], &[]).unwrap();
    assert_eq!(turn.content.as_deref(), Some("hola desde el mock"));
    server.join().unwrap();
}
