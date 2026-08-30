use super::*;

use tempfile::TempDir;

/// Loads a `proiectio.toml` from `dir` alone: [`builder`]'s own scopes reach
/// the platform config directory, which a unit test must not read.
fn load_from(dir: &TempDir) -> Result<ProiectioConfig, ClapfigError> {
    Clapfig::typed::<ProiectioConfig>()
        .app_name(APP)
        .search_paths(vec![SearchPath::Path(dir.path().to_path_buf())])
        .no_env()
        .on_unknown_key(|key| {
            if key.leaf.starts_with(COMMENT_KEY_PREFIX) {
                UnknownKeyDecision::Accept
            } else {
                UnknownKeyDecision::Reject
            }
        })
        .load()
}

fn config(dir: &TempDir, contents: &str) {
    std::fs::write(dir.path().join(FILE), contents).expect("a config file");
}

#[test]
fn an_omitted_key_takes_its_compiled_default() {
    let dir = TempDir::new().expect("a temporary directory");

    assert_eq!(
        load_from(&dir).expect("a loaded configuration").owner,
        "default"
    );
}

#[test]
fn a_file_declaring_the_owner_loads_it() {
    let dir = TempDir::new().expect("a temporary directory");
    config(&dir, "owner = \"site\"\n");

    assert_eq!(
        load_from(&dir).expect("a loaded configuration").owner,
        "site"
    );
}

/// `config schema` allowlists `^//` on every object, so a file spelling a note
/// that way validates against the emitted schema; the loader accepts the same
/// file.
#[test]
fn a_comment_key_the_schema_allowlists_loads() {
    let dir = TempDir::new().expect("a temporary directory");
    config(&dir, "\"//\" = \"a note\"\nowner = \"site\"\n");

    assert_eq!(
        load_from(&dir).expect("a loaded configuration").owner,
        "site"
    );
}

/// Only the allowlisted prefix: a stray key is still the typo it looks like.
#[test]
fn a_key_the_schema_does_not_allowlist_is_still_unknown() {
    let dir = TempDir::new().expect("a temporary directory");
    config(&dir, "onwer = \"site\"\n");

    let error = load_from(&dir).expect_err("a rejected config file");

    assert!(error.is_strict_violation(), "{error}");
}

#[test]
fn a_declared_key_passes_the_check_set_and_get_make() {
    assert!(require_key("owner").is_ok());
}

#[test]
fn an_undeclared_key_is_refused_with_the_nearest_one() {
    let error = require_key("onwer").expect_err("a key the schema does not declare");

    match error {
        ClapfigError::KeyNotFound { key, suggestion } => {
            assert_eq!(key, "onwer");
            assert_eq!(suggestion.as_deref(), Some("owner"));
        }
        other => panic!("expected KeyNotFound, got {other:?}"),
    }
}

#[test]
fn a_comment_key_is_one_wherever_the_schema_allowlists_it() {
    assert!(is_comment_key("//"));
    assert!(is_comment_key("// a note"));
    assert!(is_comment_key("//a.b"));
    assert!(is_comment_key("database.//"));
    assert!(!is_comment_key("owner"));
    assert!(!is_comment_key(""));
}

#[test]
fn the_schemas_leaf_types_are_what_a_rendered_line_is_spelled_from() {
    assert!(matches!(leaf_type("owner"), Some(LeafType::String)));
    assert!(leaf_type("onwer").is_none());
    assert!(leaf_type("owner.deeper").is_none());
}
