use super::*;

use clap::error::ErrorKind;

/// The two options every command reads, on the root so each leaf carries them.
#[test]
fn the_destination_and_the_state_directory_are_global() {
    let matches = command()
        .try_get_matches_from([
            "proiectio",
            "status",
            "--dest",
            "/srv/www",
            "--state-dir",
            "/var/lib/proiectio",
        ])
        .expect("a parsed command line");
    let status = matches.subcommand_matches("status").expect("the leaf");

    assert_eq!(
        status.get_one::<String>("dest").map(String::as_str),
        Some("/srv/www")
    );
    assert_eq!(
        status.get_one::<String>("state-dir").map(String::as_str),
        Some("/var/lib/proiectio")
    );
}

#[test]
fn the_destination_defaults_to_the_working_directory() {
    let matches = command()
        .try_get_matches_from(["proiectio", "status"])
        .expect("a parsed command line");
    let status = matches.subcommand_matches("status").expect("the leaf");

    assert_eq!(
        status.get_one::<String>("dest").map(String::as_str),
        Some(".")
    );
    assert_eq!(status.get_one::<String>("state-dir"), None);
}

/// Standout dispatches on the parsed subcommand path, so the config leaves
/// must parse under the canonical name whichever spelling names the group.
#[test]
fn the_config_group_parses_under_its_alias() {
    for group in ["config", "conf"] {
        let matches = command()
            .try_get_matches_from(["proiectio", group, "set", "owner", "site"])
            .expect("a parsed command line");
        let (name, config) = matches.subcommand().expect("the group");
        assert_eq!(name, "config");
        let (leaf, set) = config.subcommand().expect("the leaf");
        assert_eq!(leaf, "set");
        assert_eq!(
            set.get_one::<String>("key").map(String::as_str),
            Some("owner")
        );
        assert_eq!(
            set.get_one::<String>("value").map(String::as_str),
            Some("site")
        );
    }
}

/// Standout owns `--output`, so clapfig's file destination is `--file`.
#[test]
fn config_gen_writes_through_the_renamed_flag() {
    let matches = command()
        .try_get_matches_from(["proiectio", "config", "gen", "--file", "proiectio.toml"])
        .expect("a parsed command line");
    let generated = matches
        .subcommand_matches("config")
        .and_then(|config| config.subcommand_matches("gen"))
        .expect("the leaf");

    assert_eq!(
        generated.get_one::<std::path::PathBuf>("output"),
        Some(&std::path::PathBuf::from("proiectio.toml"))
    );
}

#[test]
fn a_command_line_naming_no_command_is_refused() {
    let error = command()
        .try_get_matches_from(["proiectio"])
        .expect_err("a usage error");

    assert_eq!(
        error.kind(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
}
