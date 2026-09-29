// The web app is built separately (`npm run build` in panel/web) and embedded from
// panel/web/dist. Without it (a plain `cargo build` or `cargo test`, no Node), a
// one-page placeholder stands in, so Rust work never needs Node.
use std::path::Path;

fn main() {
    let dist = Path::new("web/dist");
    let index = dist.join("index.html");
    if !index.exists() {
        std::fs::create_dir_all(dist).expect("create web/dist");
        std::fs::write(&index, PLACEHOLDER).expect("write the placeholder page");
    }
    println!("cargo:rerun-if-changed=web/dist");
    println!("cargo:rerun-if-changed=build.rs");
}

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Kariz</title>
<style>
  html,body{height:100%;margin:0;background:#081a2e;color:#eef2f7;font-family:system-ui,sans-serif}
  body{display:grid;place-items:center;text-align:center}
  h1{margin:0 0 .3em;font-size:3rem;color:#e9c46a;letter-spacing:.06em}
  p{margin:0;color:#a9b8cc}
</style></head>
<body><main><h1>Kariz</h1><p>The panel is running. Its web app is not built into this binary.</p></main></body></html>
"#;
