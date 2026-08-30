use super::*;

use libproiectio::PathState;
use serde_json::json;

/// The document `status` renders, from the rows a case states by path.
fn status(rows: JsonValue) -> StatusLines {
    let records: Vec<JsonValue> = rows
        .as_object()
        .expect("rows stated by path")
        .iter()
        .map(|(path, row)| {
            let mut record = row.as_object().expect("a row").clone();
            record.insert("path".to_owned(), json!(path));
            JsonValue::Object(record)
        })
        .collect();
    lines(&json!({ "rows": records }), AmbiguousWidth::Narrow)
}

fn only(document: StatusLines) -> StateView {
    let mut rows = document.rows;
    assert_eq!(rows.len(), 1, "one row");
    rows.remove(0)
}

/// One classified path reads as one word in one style, for every state the
/// library declares.
#[test]
fn each_state_spells_one_style_and_one_word() {
    for (state, style, word) in [
        (PathState::Clean, "clean", "clean"),
        (PathState::Drifted, "drifted", "drifted"),
        (PathState::Missing, "missing", "missing"),
        (PathState::Foreign, "foreign", "foreign"),
    ] {
        let verdict = serde_json::to_value(state).expect("a serialized state");
        let row = only(status(
            json!({ "one": { "verdict": verdict, "facts": null } }),
        ));

        assert_eq!((row.style, row.state.as_str()), (style, word), "{state:?}");
        // The column is a constant, and the words it has to hold come from the
        // library; a state spelled wider than `STATES` would leave no pad and
        // push its path out of line.
        assert_eq!(
            row.state.len() + row.state_pad.len(),
            STATES,
            "{state:?} fits the state column"
        );
    }
}

/// A verdict that carries fields is still a verdict: the row reads its name
/// and stays in the listing, rather than dropping out of it.
#[test]
fn a_verdict_carrying_fields_reads_as_its_name() {
    let row = only(status(
        json!({ "one": { "verdict": { "Drifted": { "reason": "ContentChanged" } }, "facts": null } }),
    ));

    assert_eq!((row.style, row.state.as_str()), ("drifted", "drifted"));
}

/// A row stating no verdict keeps its line too: the path is what the listing
/// is about, and a row is not the place to lose one.
#[test]
fn a_row_stating_no_verdict_keeps_its_path() {
    let row = only(status(json!({ "one": { "facts": null } })));

    assert_eq!((row.style, row.path.as_str()), ("unknown", "one"));
}

/// A verdict this CLI has no word for reads as its own name rather than
/// vanishing from the listing.
#[test]
fn an_unknown_state_reads_as_its_own_name() {
    let row = only(status(
        json!({ "one": { "verdict": "Invented", "facts": null } }),
    ));

    assert_eq!((row.style, row.state.as_str()), ("unknown", "Invented"));
}

/// The state column is as wide as the widest word, so every path starts in
/// the same place.
#[test]
fn the_state_column_aligns_the_paths() {
    let document = status(json!({
        "long.txt": { "verdict": "Drifted", "facts": null },
        "short.txt": { "verdict": "Clean", "facts": null },
    }));

    let widths: Vec<usize> = document
        .rows
        .iter()
        .map(|row| row.state.len() + row.state_pad.len())
        .collect();
    assert_eq!(widths, vec![STATES, STATES]);
}

/// A path is data. One spelled as a style tag reaches the line as the
/// characters it is rather than as markup.
#[test]
fn a_path_spelled_like_a_style_tag_reads_as_itself() {
    let row = only(status(
        json!({ "[clean]/y": { "verdict": "Foreign", "facts": null } }),
    ));

    assert_eq!(row.path, "\\[clean\\]/y");
}

/// A document with no rows prints nothing.
#[test]
fn a_document_carrying_no_rows_prints_no_lines() {
    assert_eq!(
        lines(&json!({}), AmbiguousWidth::Narrow),
        StatusLines::default()
    );
    assert_eq!(
        lines(&json!({ "rows": [] }), AmbiguousWidth::Narrow),
        StatusLines::default()
    );
}

/// The CSV columns over one classified path: the row states its own path, and
/// the facts spread into the columns the header names.
#[test]
fn csv_writes_one_row_per_path_under_a_fixed_header() {
    let document = json!({
        "rows": [
            {
                "path": "bin/tool",
                "verdict": "Clean",
                "facts": {
                    "shape": { "File": { "executable": true } },
                    "owners": ["harness", "site"],
                    "origin": null,
                },
            },
            { "path": "bin_tool", "verdict": "Foreign", "facts": null },
        ]
    });

    let csv = csv()
        .csv_projection()
        .render(&document)
        .expect("a CSV projection");

    assert_eq!(
        csv,
        "path,verdict,shape,executable,owners\n\
         bin/tool,Clean,file,true,\"[\"\"harness\"\",\"\"site\"\"]\"\n\
         bin_tool,Foreign,,,\n"
    );
}

/// The owners cell is the JSON array, so two owner sets that a joined cell
/// would spell alike stay two cells. An owner name is opaque: nothing stops
/// one from carrying whatever character a join would use.
#[test]
fn owner_names_carrying_a_separator_stay_the_owners_they_are() {
    let cells = |owners| {
        let document = json!({
            "rows": [{
                "path": "one",
                "verdict": "Clean",
                "facts": { "shape": null, "owners": owners, "origin": null },
            }]
        });
        csv()
            .csv_projection()
            .render(&document)
            .expect("a CSV projection")
    };

    assert_ne!(cells(json!(["a+b", "c"])), cells(json!(["a", "b+c"])));
    assert_eq!(
        cells(json!(["a+b", "c"])),
        "path,verdict,shape,executable,owners\n\
         one,Clean,,,\"[\"\"a+b\"\",\"\"c\"\"]\"\n"
    );
}

/// A shape with no executable bit leaves that column empty rather than
/// claiming a value the row does not state.
#[test]
fn a_link_row_states_its_shape_and_no_executable_bit() {
    let document = json!({
        "rows": [{
            "path": "current",
            "verdict": "Missing",
            "facts": {
                "shape": { "Symlink": { "target": null } },
                "owners": ["site"],
                "origin": null,
            },
        }]
    });

    let csv = csv()
        .csv_projection()
        .render(&document)
        .expect("a CSV projection");

    assert_eq!(
        csv,
        "path,verdict,shape,executable,owners\n\
         current,Missing,symlink,,\"[\"\"site\"\"]\"\n"
    );
}
