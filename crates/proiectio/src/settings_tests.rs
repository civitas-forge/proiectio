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
/// constant. This is what holds the two to each other: the bound a run takes
/// with nothing configured is the one `libproiectio` would have applied on
/// its own.
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

/// The preflight resolves a file only for an edit that lands in the scope this
/// CLI resolves. A `--scope` naming anything else is clapfig's to refuse by
/// name, and a read edits nothing — resolving for either would answer with a
/// complaint about a file neither one meant.
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

#[test]
fn the_schemas_leaf_types_are_what_a_rendered_line_is_spelled_from() {
    assert!(matches!(leaf_type("owner"), Some(LeafType::String)));
    assert!(leaf_type("onwer").is_none());
    assert!(leaf_type("owner.deeper").is_none());
}

/// A listing flattens a map's entries into dotted keys, so the walk drops the
/// entry key the writer chose and reads the item shape underneath it — the
/// same step clapfig's own `doc_for_shape` takes.
#[test]
fn a_key_under_a_map_resolves_to_the_item_shapes_leaf() {
    let shape = Shape::Object(
        RuntimeSchema::object("Demo")
            .field("owner", Field::string())
            .map_of(
                "hosts",
                RuntimeSchema::object("Host")
                    .field("label", Field::string())
                    .field("port", Field::integer()),
            )
            .build(),
    );

    let segments = |key: &'static str| key.split('.').collect::<Vec<_>>();
    assert!(matches!(
        leaf_type_in(&shape, &segments("hosts.a.label")),
        Some(LeafType::String)
    ));
    assert!(matches!(
        leaf_type_in(&shape, &segments("hosts.a.port")),
        Some(LeafType::Integer { .. })
    ));
    assert!(leaf_type_in(&shape, &segments("hosts.a.nope")).is_none());
    assert!(leaf_type_in(&shape, &segments("hosts")).is_none());
}

/// An optional field is a leaf carrying `optional`, not a shape wrapped around
/// one, so the walk reaches its type the way it reaches a required field's.
#[test]
fn an_optional_field_resolves_to_the_leaf_type_it_wraps() {
    let shape = Shape::Object(
        RuntimeSchema::object("Demo")
            .field("nickname", Field::string().optional())
            .build(),
    );

    assert!(matches!(
        leaf_type_in(&shape, &["nickname"]),
        Some(LeafType::String)
    ));
}

/// `require_key` asks the schema whether the path resolves, not whether anyone
/// documented it: clapfig answers `Some(vec![])` for a declared field with no
/// doc comment, so an undocumented key is still a key.
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
