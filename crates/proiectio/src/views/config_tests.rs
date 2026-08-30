use super::*;

#[test]
fn config_views_stay_typed_for_structured_consumers() {
    let listing = ConfigView::try_from(ConfigResult::Listing {
        entries: vec![("owner".into(), "default".into())],
        rendered: "owner = \"default\"".into(),
    })
    .expect("a view");
    let value = serde_json::to_value(&listing).expect("a serialized view");
    assert_eq!(value["kind"], "listing");
    assert_eq!(value["entries"][0]["key"], "owner");
    assert_eq!(value["entries"][0]["value"], "default");

    let set = ConfigView::try_from(ConfigResult::ValueSet {
        key: "owner".into(),
        value: "site".into(),
        rendered: "owner = \"site\"".into(),
    })
    .expect("a view");
    let value = serde_json::to_value(&set).expect("a serialized view");
    assert_eq!(value["kind"], "value_set");
    assert_eq!(value["value"], "site");

    let unset = ConfigView::try_from(ConfigResult::ValueUnset {
        key: "owner".into(),
    })
    .expect("a view");
    assert_eq!(
        serde_json::to_value(&unset).expect("a serialized view")["kind"],
        "value_unset"
    );
}

/// The written-file variants carry a path structured output has to spell, so
/// the view holds one that always serializes.
#[test]
fn a_written_path_serializes_as_the_text_it_is() {
    let written = ConfigView::try_from(ConfigResult::TemplateWritten {
        path: PathBuf::from("/srv/proiectio.toml"),
    })
    .expect("a view");
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

    let error = ConfigView::try_from(ConfigResult::SchemaWritten { path })
        .expect_err("a path that is not UTF-8");

    assert!(matches!(error, Error::PathNotUtf8 { .. }), "{error}");
}
