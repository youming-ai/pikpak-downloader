//! Persisting the rotated refresh token back into the `.env` file it came from.

use std::path::{Path as StdPath, PathBuf};

/// Pick the `.env` file a rotated token should be written back to.
///
/// `loaded` is the file dotenvy actually read — it searches the working
/// directory *and its parents* — so preferring it keeps reads and writes on the
/// same file. Only when no file was loaded do we fall back to a `.env` in the
/// working directory, and only if one already exists: we never create one.
pub(crate) fn env_file_for_persistence(
    loaded: Option<&StdPath>,
    local: &StdPath,
) -> Option<PathBuf> {
    match loaded {
        Some(path) => Some(path.to_path_buf()),
        None => local.exists().then(|| local.to_path_buf()),
    }
}

/// Persist a rotated refresh token and report the outcome.
///
/// Runs from the client's rotation hook, so it executes as soon as the server
/// issues a new token; `None` means there is nowhere to write.
pub(crate) fn persist_rotated_token(env_path: Option<&StdPath>, token: &str) {
    let Some(path) = env_path else {
        eprintln!("note: refresh token rotated; set PIKPAK_REFRESH_TOKEN to:\n  {token}");
        return;
    };
    match update_env_token(path, token) {
        Ok(true) => eprintln!("note: refresh token rotated; updated {}", path.display()),
        Ok(false) => {
            eprintln!("note: refresh token rotated; set PIKPAK_REFRESH_TOKEN to:\n  {token}")
        }
        Err(e) => eprintln!(
            "warning: refresh token rotated but {} could not be updated ({e}); new token:\n  {token}",
            path.display()
        ),
    }
}

/// Rewrite the `PIKPAK_REFRESH_TOKEN=` line in `env_path`, preserving every
/// other line. Returns `Ok(true)` if the key was found and rewritten, or
/// `Ok(false)` if the file is absent or has no such key.
///
/// The rewrite is atomic (a sibling temporary file, then a rename), so an
/// interrupted process cannot leave a truncated `.env` behind — that file holds
/// the only copy of a credential the user can no longer recover.
fn update_env_token(env_path: &StdPath, new_token: &str) -> std::io::Result<bool> {
    if !env_path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(env_path)?;
    let mut found = false;
    let mut out = String::with_capacity(content.len() + new_token.len());
    for line in content.lines() {
        let Some(offset) = token_key_offset(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        // Keep whatever the user wrote before the key (`export `, indentation).
        out.push_str(&line[..offset]);
        out.push_str("PIKPAK_REFRESH_TOKEN=");
        out.push_str(&env_quote(new_token));
        out.push('\n');
        found = true;
    }
    if !found {
        return Ok(false);
    }
    write_atomically(env_path, out.as_bytes())?;
    Ok(true)
}

/// Byte offset of the `PIKPAK_REFRESH_TOKEN=` key within `line`, if present.
///
/// Tolerates leading indentation and an `export ` prefix, both of which must be
/// reproduced verbatim when the line is rewritten. `export` only counts as that
/// prefix when whitespace separates it from the key: dotenvy loads
/// `exportPIKPAK_REFRESH_TOKEN=…` under the *distinct* key
/// `exportPIKPAK_REFRESH_TOKEN`, so such a line must not be rewritten.
fn token_key_offset(line: &str) -> Option<usize> {
    let rest = line.trim_start_matches([' ', '\t']);
    let mut offset = line.len() - rest.len();
    let rest = match rest
        .strip_prefix("export")
        .filter(|after| after.starts_with([' ', '\t']))
    {
        Some(after) => {
            let after = after.trim_start_matches([' ', '\t']);
            offset = line.len() - after.len();
            after
        }
        None => rest,
    };
    rest.starts_with("PIKPAK_REFRESH_TOKEN=").then_some(offset)
}

/// Quote a value for a `.env` file when it contains characters dotenv would
/// otherwise treat as syntax, escaping what double quotes themselves treat as
/// syntax.
///
/// The rules come from dotenvy 0.15's actual parser (verified against it):
/// inside double quotes an unescaped `$` makes the whole line fail to parse —
/// as does any escape the parser does not know, such as `\s` or `\r` — while
/// `\\`, `\"`, `\$` and `\n` are honoured and a literal carriage return passes
/// through untouched. Getting this wrong would write a `.env` the next run
/// cannot parse at all, stranding the rotated token (the previous one is
/// already invalid server-side).
fn env_quote(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '#' | '"' | '\'' | '\\' | '$' | '`'));
    if !needs_quotes {
        return value.to_string();
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// Write `bytes` to `path` through a sibling temporary file and a rename.
///
/// Windows cannot rename over an existing file, so there the rewrite falls back
/// to an in-place write — correct, though not atomic.
fn write_atomically(path: &StdPath, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(path, bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::update_env_token;

    #[test]
    fn update_env_rewrites_token_line() {
        let dir = std::env::temp_dir().join(format!("pikpak-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(
            &env_path,
            "PIKPAK_REFRESH_TOKEN=old\nPIKPAK_PROXY=http://x\n",
        )
        .unwrap();

        let changed = update_env_token(&env_path, "brand-new").unwrap();
        assert!(changed);

        let content = std::fs::read_to_string(&env_path).unwrap();
        assert!(content.contains("PIKPAK_REFRESH_TOKEN=brand-new"));
        assert!(content.contains("PIKPAK_PROXY=http://x"));
        assert!(!content.contains("=old"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_env_absent_file_is_noop() {
        let path = std::env::temp_dir().join("pikpak-does-not-exist-abc.env");
        let _ = std::fs::remove_file(&path);
        assert!(!update_env_token(&path, "x").unwrap());
    }

    #[test]
    fn update_env_without_key_returns_false() {
        let dir = std::env::temp_dir().join(format!("pikpak-env2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(&env_path, "PIKPAK_PROXY=http://x\n").unwrap();
        assert!(!update_env_token(&env_path, "x").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_key_offset_recognises_export_and_indentation() {
        assert_eq!(super::token_key_offset("PIKPAK_REFRESH_TOKEN=x"), Some(0));
        assert_eq!(super::token_key_offset("  PIKPAK_REFRESH_TOKEN=x"), Some(2));
        assert_eq!(
            super::token_key_offset("export PIKPAK_REFRESH_TOKEN=x"),
            Some(7)
        );
        assert_eq!(
            super::token_key_offset("\texport  PIKPAK_REFRESH_TOKEN=x"),
            Some(9)
        );
        // A tab after `export` separates it from the key for dotenvy too.
        assert_eq!(
            super::token_key_offset("export\tPIKPAK_REFRESH_TOKEN=x"),
            Some(7)
        );
        // Glued to the key, `export` is part of a *different* variable's name:
        // dotenvy loads it as `exportPIKPAK_REFRESH_TOKEN`, so it must not be
        // rewritten.
        assert_eq!(
            super::token_key_offset("exportPIKPAK_REFRESH_TOKEN=x"),
            None
        );
        assert_eq!(super::token_key_offset("PIKPAK_PROXY=http://x"), None);
        assert_eq!(super::token_key_offset("export PIKPAK_PROXY=x"), None);
    }

    #[test]
    fn env_quote_only_quotes_when_needed() {
        assert_eq!(super::env_quote("plain-token_123"), "plain-token_123");
        assert_eq!(super::env_quote("has space"), "\"has space\"");
        assert_eq!(super::env_quote("trailing#comment"), "\"trailing#comment\"");
        assert_eq!(super::env_quote(""), "\"\"");
        assert_eq!(super::env_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
        // `$` would otherwise expand (or fail to parse when undefined).
        assert_eq!(super::env_quote("a$b"), "\"a\\$b\"");
        // A newline is written as the `\n` escape so the entry stays on one line.
        assert_eq!(super::env_quote("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn update_env_round_trips_through_dotenvy() {
        // Every value must come back out of dotenvy's own parser unchanged. The
        // rotated refresh token is the only valid credential after a rotation,
        // so a value the next run cannot parse — or parses differently — would
        // strand the account.
        for token in [
            "plain-token_123",
            "two words",
            "with$dollar", // undefined `$VAR` would fail the whole line
            "a$1b",        // `$1` would fail the whole line too
            "say \"hi\"",
            "back\\slash", // an unknown escape such as `\s` fails the line
            "a\\nb",       // literal backslash-n must not turn into a newline
            "a\nb",        // actual newline, must survive via the `\n` escape
            "a\rb",        // actual carriage return, dotenvy passes it raw
            "hash#inside",
            "tick`mark",
            "quote'inside",
            "",
        ] {
            let dir = std::env::temp_dir().join(format!("pikpak-roundtrip-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let env_path = dir.join(".env");
            std::fs::write(
                &env_path,
                "PIKPAK_REFRESH_TOKEN=old\nPIKPAK_PROXY=http://x\n",
            )
            .unwrap();

            assert!(
                update_env_token(&env_path, token).unwrap(),
                "the key line must be found for {token:?}"
            );

            std::env::remove_var("PIKPAK_REFRESH_TOKEN");
            std::env::remove_var("PIKPAK_PROXY");
            dotenvy::from_path_override(&env_path)
                .unwrap_or_else(|e| panic!("the rewritten .env must parse for {token:?}: {e:?}"));
            let loaded = std::env::var("PIKPAK_REFRESH_TOKEN").unwrap();
            std::env::remove_var("PIKPAK_REFRESH_TOKEN");
            std::env::remove_var("PIKPAK_PROXY");

            assert_eq!(loaded, token, "dotenvy must read the exact token back");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn update_env_keeps_an_export_prefix_and_indentation() {
        let dir = std::env::temp_dir().join(format!("pikpak-env3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(
            &env_path,
            "  export PIKPAK_REFRESH_TOKEN=old\n# keep me\nPIKPAK_PROXY=http://x\n",
        )
        .unwrap();

        assert!(update_env_token(&env_path, "new-token").unwrap());

        let content = std::fs::read_to_string(&env_path).unwrap();
        assert!(
            content.contains("  export PIKPAK_REFRESH_TOKEN=new-token"),
            "prefix must survive: {content:?}"
        );
        assert!(content.contains("# keep me"), "{content:?}");
        assert!(content.contains("PIKPAK_PROXY=http://x"), "{content:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_env_quotes_a_value_that_needs_it() {
        let dir = std::env::temp_dir().join(format!("pikpak-env4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(&env_path, "PIKPAK_REFRESH_TOKEN=old\n").unwrap();

        assert!(update_env_token(&env_path, "two words").unwrap());

        let content = std::fs::read_to_string(&env_path).unwrap();
        assert!(
            content.contains("PIKPAK_REFRESH_TOKEN=\"two words\""),
            "{content:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_env_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("pikpak-env5-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(&env_path, "PIKPAK_REFRESH_TOKEN=old\n").unwrap();

        assert!(update_env_token(&env_path, "new").unwrap());

        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files left behind: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_file_for_persistence_prefers_the_loaded_path() {
        let dir = std::env::temp_dir().join(format!("pikpak-envpath-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join(".env");
        let parent = dir.join("parent.env");
        std::fs::write(&local, "X=1\n").unwrap();
        std::fs::write(&parent, "X=1\n").unwrap();

        // dotenvy may have loaded a file in a parent directory; that file is the
        // one being read, so it must also be the one written.
        assert_eq!(
            super::env_file_for_persistence(Some(&parent), &local).unwrap(),
            parent
        );

        // With no loaded file, an existing local `.env` is used...
        assert_eq!(
            super::env_file_for_persistence(None, &local).unwrap(),
            local
        );

        // ...and a missing one is never created.
        let missing = dir.join("absent.env");
        assert_eq!(super::env_file_for_persistence(None, &missing), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persist_rotated_token_rewrites_the_loaded_file() {
        let dir = std::env::temp_dir().join(format!("pikpak-persist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(
            &env_path,
            "PIKPAK_REFRESH_TOKEN=old\nPIKPAK_PROXY=http://x\n",
        )
        .unwrap();

        super::persist_rotated_token(Some(&env_path), "rotated");

        let content = std::fs::read_to_string(&env_path).unwrap();
        assert!(content.contains("PIKPAK_REFRESH_TOKEN=rotated"));
        assert!(content.contains("PIKPAK_PROXY=http://x"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
