use super::*;

const SCOPE: &str = "/home/u/.config/proiectio/proiectio.toml";

/// A view of a result whose edit left a file at the path it names.
fn view(result: ConfigResult) -> ConfigView {
    ConfigView::of(result, || Ok(edit(true))).expect("a view")
}

fn edit(present: bool) -> Edit {
    Edit {
        path: Utf8PathBuf::from(SCOPE),
        present,
    }
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

/// Clapfig treats an unset with no file to read as a successful no-op, so the
/// view carries whether the file was written rather than assuming it was.
#[test]
fn an_unset_that_wrote_no_file_says_so() {
    let unset = ConfigView::of(
        ConfigResult::ValueUnset {
            key: "owner".into(),
        },
        || Ok(edit(false)),
    )
    .expect("a view");

    let value = serde_json::to_value(&unset).expect("a serialized view");
    assert_eq!(value["wrote"], false);
    assert_eq!(value["path"], SCOPE, "the view names no file");
}

/// A read edits nothing, so it never pays for the platform lookup that names
/// an edited file — and renders on a machine where that lookup fails.
#[test]
fn a_result_that_edited_no_file_never_resolves_one() {
    for result in [
        ConfigResult::Listing {
            entries: vec![("owner".into(), "site".into())],
            rendered: "owner = site".into(),
        },
        ConfigResult::KeyValue {
            key: "owner".into(),
            value: "site".into(),
            doc: Vec::new(),
            rendered: "owner = site".into(),
        },
        ConfigResult::Template("owner = \"site\"".into()),
        ConfigResult::Schema("{}".into()),
    ] {
        ConfigView::of(result, || panic!("a read asked for the edited file"))
            .expect("a view built without one");
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

/// A scoped listing carries keys the schema does not name, already
/// stringified; every line parses whatever the writer put there.
#[test]
fn a_line_for_a_key_the_schema_does_not_name_still_parses() {
    let listing = view(ConfigResult::Listing {
        entries: vec![
            ("a b".into(), "hello".into()),
            ("count".into(), "12".into()),
            ("flag".into(), "true".into()),
            ("empty".into(), String::new()),
        ],
        rendered: String::new(),
    });

    let ConfigView::Listing { rendered, .. } = &listing else {
        panic!("a listing");
    };
    let parsed: toml::Table = rendered
        .parse()
        .unwrap_or_else(|error| panic!("the listing printed {rendered:?}: {error}"));
    assert_eq!(parsed["a b"].as_str(), Some("hello"));
    assert_eq!(parsed["empty"].as_str(), Some(""));
    assert_eq!(parsed["count"].as_integer(), Some(12));
    assert_eq!(parsed["flag"].as_bool(), Some(true));
}

/// A set that came back is a file clapfig created, so the view reads the write
/// off the result rather than off a later look at the path.
#[test]
fn a_set_reports_the_write_its_result_already_stands_for() {
    let set = ConfigView::of(
        ConfigResult::ValueSet {
            key: "owner".into(),
            value: "site".into(),
            rendered: "owner = site".into(),
        },
        || Ok(edit(false)),
    )
    .expect("a view");

    assert_eq!(
        serde_json::to_value(&set).expect("a serialized view")["wrote"],
        true
    );
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

    let error = ConfigView::of(ConfigResult::SchemaWritten { path }, || Ok(edit(true)))
        .expect_err("a path that is not UTF-8");

    assert!(matches!(error, Error::PathNotUtf8 { .. }), "{error}");
}
