use super::*;

#[test]
fn config_views_stay_typed_for_structured_consumers() {
    let listing = ConfigView::from(ConfigResult::Listing {
        entries: vec![("owner".into(), "default".into())],
        rendered: "owner = \"default\"".into(),
    });
    let value = serde_json::to_value(&listing).expect("a serialized view");
    assert_eq!(value["kind"], "listing");
    assert_eq!(value["entries"][0]["key"], "owner");
    assert_eq!(value["entries"][0]["value"], "default");

    let set = ConfigView::from(ConfigResult::ValueSet {
        key: "owner".into(),
        value: "site".into(),
        rendered: "owner = \"site\"".into(),
    });
    let value = serde_json::to_value(&set).expect("a serialized view");
    assert_eq!(value["kind"], "value_set");
    assert_eq!(value["value"], "site");

    let unset = ConfigView::from(ConfigResult::ValueUnset {
        key: "owner".into(),
    });
    assert_eq!(
        serde_json::to_value(&unset).expect("a serialized view")["kind"],
        "value_unset"
    );
}
