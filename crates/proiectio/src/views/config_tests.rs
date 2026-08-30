use super::*;

const SCOPE: &str = "/home/u/.config/proiectio/proiectio.toml";

fn view(result: ConfigResult) -> ConfigView {
    ConfigView::of(result, Utf8PathBuf::from(SCOPE)).expect("a view")
}

#[test]
fn config_views_stay_typed_for_structured_consumers() {
    let listing = view(ConfigResult::Listing {
        entries: vec![("owner".into(), "default".into())],
        rendered: "owner = default".into(),
    });
    let value = serde_json::to_value(&listing).expect("a serialized view");
    assert_eq!(value["kind"], "listing");
    assert_eq!(value["entries"][0]["key"], "owner");
    assert_eq!(value["entries"][0]["value"], "default");

    let set = view(ConfigResult::ValueSet {
        key: "owner".into(),
        value: "site".into(),
        rendered: "owner = site".into(),
    });
    let value = serde_json::to_value(&set).expect("a serialized view");
    assert_eq!(value["kind"], "value_set");
    assert_eq!(value["value"], "site");

    let unset = view(ConfigResult::ValueUnset {
        key: "owner".into(),
    });
    assert_eq!(
        serde_json::to_value(&unset).expect("a serialized view")["kind"],
        "value_unset"
    );
}

/// A set and an unset edited a file, so both name it — in every output mode,
/// which the structured field is what carries.
#[test]
fn an_edited_file_is_named_by_the_results_that_edited_it() {
    let set = view(ConfigResult::ValueSet {
        key: "owner".into(),
        value: "site".into(),
        rendered: "owner = site".into(),
    });
    let unset = view(ConfigResult::ValueUnset {
        key: "owner".into(),
    });

    for (case, rendered) in [("set", set), ("unset", unset)] {
        assert_eq!(
            serde_json::to_value(&rendered).expect("a serialized view")["path"],
            SCOPE,
            "the {case} view names no file"
        );
    }
}

/// Clapfig spells a value for a human to read, which leaves a string needing
/// quotes bare; what this CLI prints is a line of the config file.
#[test]
fn a_rendered_value_parses_as_the_toml_it_looks_like() {
    let awkward = r"a\b\[c]";
    let listing = view(ConfigResult::Listing {
        entries: vec![("owner".into(), awkward.into())],
        rendered: format!("owner = {awkward}"),
    });

    let ConfigView::Listing { rendered, .. } = &listing else {
        panic!("a listing");
    };
    let parsed: toml::Table = rendered.parse().expect("a config file line");
    assert_eq!(parsed["owner"].as_str(), Some(awkward));
}

/// A comment key is a note, not a setting: the loader accepts one, and a
/// listing of the configuration leaves it in the file it came from.
#[test]
fn a_comment_key_is_left_out_of_the_listing() {
    let listing = view(ConfigResult::Listing {
        entries: vec![
            ("//".into(), "a note".into()),
            ("owner".into(), "site".into()),
        ],
        rendered: "// = a note\nowner = site".into(),
    });

    let ConfigView::Listing { entries, rendered } = &listing else {
        panic!("a listing");
    };
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].key, "owner");
    assert_eq!(rendered, r#"owner = "site""#);
}

/// `get` prints the key's doc comment above the assignment, and the assignment
/// is the same line `list` prints.
#[test]
fn a_documented_value_keeps_its_doc_above_a_parseable_line() {
    let documented = view(ConfigResult::KeyValue {
        key: "owner".into(),
        value: "a b".into(),
        doc: vec!["The manifest owner.".into()],
        rendered: "owner = a b".into(),
    });

    let ConfigView::KeyValue { rendered, .. } = &documented else {
        panic!("a key and its value");
    };
    assert_eq!(rendered, "# The manifest owner.\nowner = \"a b\"");
    let parsed: toml::Table = rendered.parse().expect("a config file block");
    assert_eq!(parsed["owner"].as_str(), Some("a b"));
}

/// The written-file variants carry a path structured output has to spell, so
/// the view holds one that always serializes.
#[test]
fn a_written_path_serializes_as_the_text_it_is() {
    let written = view(ConfigResult::TemplateWritten {
        path: PathBuf::from("/srv/proiectio.toml"),
    });
    let value = serde_json::to_value(&written).expect("a serialized view");
    assert_eq!(value["kind"], "template_written");
    assert_eq!(value["path"], "/srv/proiectio.toml");
}

/// Clapfig can only reach this with a path it chose itself, since `--file` is
/// read as UTF-8; it is reported rather than failing the render.
#[cfg(unix)]
#[test]
fn a_written_path_that_is_not_utf8_is_reported_rather_than_rendered() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let path = PathBuf::from(OsString::from_vec(vec![0x2f, 0xff]));

    let error = ConfigView::of(
        ConfigResult::SchemaWritten { path },
        Utf8PathBuf::from(SCOPE),
    )
    .expect_err("a path that is not UTF-8");

    assert!(matches!(error, Error::PathNotUtf8 { .. }), "{error}");
}
