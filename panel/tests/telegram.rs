//! The Telegram bot against a stand-in for Telegram on this machine: connecting a chat with
//! a code, answering a command, refusing strangers, a failing API.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use kariz_panel::db::Db;
use kariz_panel::hub::Hub;
use kariz_panel::telegram::{self, Settings};
use serde_json::{json, Value};

const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

/// Answers every request with `(status, body)` and remembers `(path, body)` of each.
struct Fake {
    base: String,
    seen: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Fake {
    fn start(status: u16, answer: Value) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Headers, then as many body bytes as Content-Length says.
                let body = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break String::new();
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some(at) = text.find("\r\n\r\n") {
                        let len = text[..at]
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if text.len() >= at + 4 + len {
                            break text[at + 4..at + 4 + len].to_owned();
                        }
                    }
                };
                let head = String::from_utf8_lossy(&buf).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                log.lock()
                    .unwrap()
                    .push((path, serde_json::from_str(&body).unwrap_or(Value::Null)));
                let out = answer.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                    out.len()
                );
            }
        });
        Fake { base, seen }
    }

    fn sent(&self) -> Vec<(String, Value)> {
        self.seen.lock().unwrap().clone()
    }
}

fn hub() -> (Arc<Hub>, Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::in_memory().unwrap();
    let hub = Hub::new(db.clone(), dir.path().join("kariz"));
    (hub, db, dir)
}

fn configured(db: &Db, fake: &Fake) -> Settings {
    let s = Settings {
        enabled: true,
        token: TOKEN.into(),
        lang: "en".into(),
        api_base: fake.base.clone(),
        ..Settings::default()
    };
    s.save(db).unwrap();
    s
}

fn message(chat: i64, text: &str) -> Value {
    json!({ "update_id": 1, "message": { "chat": { "id": chat, "first_name": "Ada" }, "text": text } })
}

fn texts(fake: &Fake) -> Vec<(i64, String)> {
    fake.sent()
        .into_iter()
        .filter(|(p, _)| p.ends_with("/sendMessage"))
        .map(|(_, b)| {
            (
                b["chat_id"].as_i64().unwrap(),
                b["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_chat_connects_with_the_code_and_then_gets_answers() {
    let fake = Fake::start(200, json!({ "ok": true, "result": {} }));
    let (hub, db, _dir) = hub();
    let s = configured(&db, &fake);
    let (code, _) = hub.telegram.new_pair().unwrap();

    // A stranger gets nothing, with or without a command.
    telegram::handle(&hub, &s, &message(99, "/status")).await;
    telegram::handle(&hub, &s, &message(99, "/start WRONGCODE")).await;
    assert!(texts(&fake).is_empty());
    assert!(Settings::load(&db).chats.is_empty());

    // The right code connects the chat, and says so.
    telegram::handle(&hub, &s, &message(42, &format!("/start {code}"))).await;
    let after = Settings::load(&db);
    assert_eq!(after.chats.len(), 1);
    assert_eq!(after.chats[0].id, 42);
    assert_eq!(after.chats[0].name, "Ada");
    let sent = texts(&fake);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.contains("connected"));

    // The code works once.
    telegram::handle(&hub, &after, &message(7, &format!("/start {code}"))).await;
    assert_eq!(Settings::load(&db).chats.len(), 1);

    // The connected chat gets the status, and an answer to what it does not know.
    telegram::handle(&hub, &after, &message(42, "/status")).await;
    telegram::handle(&hub, &after, &message(42, "/server nothing-like-it")).await;
    telegram::handle(&hub, &after, &message(42, "hello")).await;
    let sent = texts(&fake);
    assert!(sent[1].1.contains("Kariz status"), "{}", sent[1].1);
    assert!(
        sent[2].1.contains("No server with that name"),
        "{}",
        sent[2].1
    );
    assert!(sent[3].1.contains("/status"), "{}", sent[3].1);
    // Every request went to the bot's own path.
    assert!(fake
        .sent()
        .iter()
        .all(|(p, _)| p.starts_with(&format!("/bot{TOKEN}/"))));
}

#[tokio::test]
async fn mute_silences_and_unmute_brings_it_back() {
    let fake = Fake::start(200, json!({ "ok": true, "result": {} }));
    let (hub, db, _dir) = hub();
    let mut s = configured(&db, &fake);
    s.chats.push(telegram::Chat {
        id: 42,
        name: "Ada".into(),
    });
    s.save(&db).unwrap();

    telegram::handle(&hub, &s, &message(42, "/mute 2h")).await;
    let view = hub.telegram.view(&Settings::load(&db), &db);
    assert!(view["muted_until"].as_i64().is_some_and(|t| t > 0));
    telegram::handle(&hub, &s, &message(42, "/unmute")).await;
    let view = hub.telegram.view(&Settings::load(&db), &db);
    assert!(view["muted_until"].is_null());
}

#[tokio::test]
async fn the_test_button_reports_the_bot_and_sends_to_every_chat() {
    let fake = Fake::start(
        200,
        json!({ "ok": true, "result": { "username": "my_kariz_bot" } }),
    );
    let (hub, db, _dir) = hub();
    let mut s = configured(&db, &fake);
    s.chats = vec![
        telegram::Chat {
            id: 1,
            name: "a".into(),
        },
        telegram::Chat {
            id: 2,
            name: "b".into(),
        },
    ];
    s.save(&db).unwrap();
    assert_eq!(telegram::check(&hub).await.unwrap(), "my_kariz_bot");
    let to: Vec<i64> = texts(&fake).iter().map(|(c, _)| *c).collect();
    assert_eq!(to, vec![1, 2]);
}

#[tokio::test]
async fn a_refusal_by_telegram_is_reported_without_the_token() {
    let fake = Fake::start(
        401,
        json!({ "ok": false, "description": "Unauthorized", "error_code": 401 }),
    );
    let (hub, db, _dir) = hub();
    configured(&db, &fake);
    let why = telegram::check(&hub).await.unwrap_err();
    assert!(why.contains("Unauthorized"), "{why}");
    assert!(!why.contains(TOKEN), "{why}");
    let view = hub.telegram.view(&Settings::load(&db), &db).to_string();
    assert!(view.contains("Unauthorized"));
    assert!(!view.contains(TOKEN));
}

#[tokio::test]
async fn an_api_that_is_not_there_is_reported_without_the_token() {
    // Nothing listens here.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let (hub, db, _dir) = hub();
    Settings {
        enabled: true,
        token: TOKEN.into(),
        api_base: base,
        ..Settings::default()
    }
    .save(&db)
    .unwrap();
    let why = telegram::check(&hub).await.unwrap_err();
    assert!(!why.is_empty());
    assert!(!why.contains(TOKEN), "{why}");
}

#[tokio::test]
async fn without_a_token_there_is_nothing_to_test() {
    let (hub, _db, _dir) = hub();
    assert_eq!(telegram::check(&hub).await.unwrap_err(), "no_token");
}
