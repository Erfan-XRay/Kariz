//! The sample configs in `configs/` are valid, and each entry / exit pair fits together.

use std::path::Path;

use kariz::config::{Config, Role};

/// Parses a sample, with its placeholder token replaced (validation rejects it).
fn load(path: &Path) -> Config {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(
        text.contains("CHANGE-ME"),
        "{}: keep the placeholder token",
        path.display()
    );
    let text = text.replace("CHANGE-ME-run-kariz-token", "sample-token-0123456789abcdef");
    Config::parse(&text).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()))
}

#[test]
fn sample_configs_are_valid_and_paired() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("configs");
    let mut pairs = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        let config = load(&path);
        assert!(
            config.warnings().is_empty(),
            "{name}: {:?}",
            config.warnings()
        );
        let Some(rest) = name.strip_prefix("entry-") else {
            assert!(
                name.starts_with("exit-"),
                "{name}: name samples entry-* / exit-*"
            );
            assert_eq!(config.role, Role::Exit, "{name}");
            assert!(
                dir.join(format!("entry-{}", &name[5..])).exists(),
                "{name}: no pair"
            );
            continue;
        };
        assert_eq!(config.role, Role::Entry, "{name}");
        let exit = load(&dir.join(format!("exit-{rest}")));
        assert_eq!(config.mode, exit.mode, "{name}: mode");
        assert_eq!(
            config.tunnel.transport, exit.tunnel.transport,
            "{name}: transport"
        );
        assert_eq!(config.mux().enabled, exit.mux().enabled, "{name}: mux");
        assert_eq!(
            config.tunnel.encryption, exit.tunnel.encryption,
            "{name}: encryption"
        );
        let path_of = |c: &Config| c.tunnel.ws.as_ref().map(|w| w.path.clone());
        assert_eq!(path_of(&config), path_of(&exit), "{name}: ws.path");
        pairs += 1;
    }
    assert!(pairs >= 5, "found {pairs} pairs");
}
