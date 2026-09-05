use super::*;

use clapfig::runtime::{Field, Schema as RuntimeSchema};
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

/// A clapfig default is a literal, so the schema cannot spell the library's
/// constant; this holds the two to each other.
#[test]
fn the_size_bounds_default_is_the_librarys_own() {
    let dir = TempDir::new().expect("a temporary directory");

    assert_eq!(
        load_from(&dir)
            .expect("a loaded configuration")
            .max_source_size,
        libproiectio::Limits::DEFAULT_MAX_SOURCE_BYTES
    );
}

#[test]
fn a_file_declaring_the_size_bound_loads_it() {
    let dir = TempDir::new().expect("a temporary directory");
    config(&dir, "max_source_size = 4096\n");

    assert_eq!(
        load_from(&dir)
            .expect("a loaded configuration")
            .max_source_size,
        4096
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

/// A file spelling a note under the `^//` allowlist validates against the
/// emitted schema; the loader accepts the same file.
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

/// The preflight resolves a file only for an edit that lands in the scope
/// this CLI resolves; a wrong `--scope` is clapfig's to refuse by name.
#[test]
fn only_an_edit_through_the_registered_scope_resolves_a_file_first() {
    let unset = |scope: Option<&str>| ConfigAction::Unset {
        key: "owner".into(),
        scope: scope.map(str::to_owned),
    };
    let set = |scope: Option<&str>| ConfigAction::Set {
        key: "owner".into(),
        value: "site".into(),
        scope: scope.map(str::to_owned),
    };

    assert!(edits_the_user_scope(&unset(None)));
    assert!(edits_the_user_scope(&set(None)));
    assert!(edits_the_user_scope(&unset(Some(USER_SCOPE))));
    assert!(!edits_the_user_scope(&unset(Some("local"))));
    assert!(!edits_the_user_scope(&set(Some("local"))));
    assert!(!edits_the_user_scope(&ConfigAction::List { scope: None }));
    assert!(!edits_the_user_scope(&ConfigAction::Get {
        key: "owner".into(),
        scope: None,
    }));
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

/// `require_key` asks whether the path resolves, not whether anyone
/// documented it: an undocumented key is still a key.
#[test]
fn a_key_the_schema_declares_without_a_doc_comment_is_still_a_key() {
    let shape = Shape::Object(
        RuntimeSchema::object("Demo")
            .field("undocumented", Field::string())
            .build(),
    );

    assert_eq!(
        clapfig::meta::doc_for_shape(&shape, "undocumented"),
        Some(Vec::new())
    );
}

/// The key the owner rule is matched by name on is one the schema declares.
#[test]
fn the_owner_rule_names_a_key_the_schema_declares() {
    assert!(declared_keys().iter().any(|key| key == OWNER_KEY));
}

/// `config set` is where a non-value is refused: every key refuses an empty
/// string; `owner` refuses a blank one too.
#[test]
fn config_set_refuses_a_value_that_names_nothing() {
    for (key, value) in [
        ("owner", ""),
        ("owner", " "),
        ("owner", "\t\n"),
        ("max_source_size", ""),
    ] {
        let error = require_value(key, value)
            .expect_err(&format!("a refusal for {key} = {value:?}"))
            .to_string();

        assert!(error.contains(key), "{key} = {value:?}: {error}");
        assert!(
            error.contains(&format!("{value:?}")),
            "{key} = {value:?}: {error}"
        );
    }
}

/// A value with something in it is a value, spaces included: the rule is about
/// a name with nothing in it, not about how the name is spelled.
#[test]
fn config_set_takes_a_value_that_names_something() {
    for (key, value) in [("owner", "site"), ("owner", "my site"), ("owner", "0")] {
        require_value(key, value).unwrap_or_else(|error| panic!("{key} = {value:?}: {error}"));
    }
}

/// The owner a file or `PROIECTIO__OWNER` resolved reaches the run already
/// parsed, so the rule is kept again where the run reads it.
#[test]
fn a_configured_owner_that_names_nothing_is_refused_where_a_run_reads_it() {
    for configured in ["", " ", "   "] {
        let error = require_owner(configured.to_owned())
            .expect_err(&format!("a refusal for {configured:?}"))
            .to_string();

        assert!(error.contains(OWNER_RULE), "{configured:?}: {error}");
        assert!(
            error.contains(&format!("{configured:?}")),
            "{configured:?}: {error}"
        );
    }

    assert_eq!(
        require_owner("site".to_owned()).expect("a configured owner"),
        "site"
    );
}
