use super::*;

use camino::Utf8PathBuf;
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
        generated.get_one::<Utf8PathBuf>("output"),
        Some(&Utf8PathBuf::from("proiectio.toml"))
    );
}

/// Clapfig writes the file before it names the path, so a path this CLI could
/// not render has to be refused at the command line rather than after the
/// write. Both leaves that take one read it as UTF-8.
#[cfg(unix)]
#[test]
fn a_config_file_path_that_is_not_utf8_is_refused_at_the_command_line() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    for leaf in ["gen", "schema"] {
        let error = command()
            .try_get_matches_from([
                OsString::from("proiectio"),
                OsString::from("config"),
                OsString::from(leaf),
                OsString::from("--file"),
                OsString::from_vec(vec![0x2f, 0xff]),
            ])
            .expect_err("a usage error");

        assert_eq!(error.kind(), ErrorKind::InvalidUtf8, "{leaf}: {error}");
    }
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

/// The mode rule as the command line states it: one positional, several
/// positionals, or `--tree`. Nothing reads the file to decide.
#[test]
fn write_reads_its_mode_off_the_command_line() {
    for (argv, paths, tree) in [
        (
            vec!["proiectio", "write", "deploy.toml"],
            vec!["deploy.toml"],
            None,
        ),
        (
            vec!["proiectio", "write", "./motd", "./banner.txt"],
            vec!["./motd", "./banner.txt"],
            None,
        ),
        (
            vec!["proiectio", "write", "--tree", "./skeleton"],
            vec![],
            Some("./skeleton"),
        ),
    ] {
        let matches = command()
            .try_get_matches_from(&argv)
            .unwrap_or_else(|error| panic!("{argv:?}: {error}"));
        let write = matches.subcommand_matches("write").expect("the leaf");

        assert_eq!(
            write
                .get_many::<Utf8PathBuf>("paths")
                .map(|values| values.map(|value| value.as_str()).collect::<Vec<_>>())
                .unwrap_or_default(),
            paths,
            "{argv:?}"
        );
        assert_eq!(
            write
                .get_one::<Utf8PathBuf>("tree")
                .map(|value| value.as_str()),
            tree,
            "{argv:?}"
        );
    }
}

/// A command line naming no desired tree, and one naming two, are both usage
/// errors rather than a guess.
#[test]
fn write_refuses_a_command_line_that_names_no_tree_or_two() {
    for argv in [
        vec!["proiectio", "write"],
        vec!["proiectio", "write", "--strip", "1"],
        vec!["proiectio", "write", "deploy.toml", "--tree", "./skeleton"],
        vec!["proiectio", "write", "deploy.toml", "--strip", "1"],
    ] {
        let error = command()
            .try_get_matches_from(&argv)
            .expect_err(&format!("a usage error for {argv:?}"));

        assert!(
            matches!(
                error.kind(),
                ErrorKind::MissingRequiredArgument | ErrorKind::ArgumentConflict
            ),
            "{argv:?}: {:?}",
            error.kind()
        );
    }
}

/// Every permission the tour names is a flag on the invocation.
#[test]
fn write_carries_every_permission_on_the_invocation() {
    let matches = command()
        .try_get_matches_from([
            "proiectio",
            "write",
            "deploy.toml",
            "--owner",
            "site",
            "--dry-run",
            "--force",
            "--allow-external-targets",
        ])
        .expect("a parsed command line");
    let write = matches.subcommand_matches("write").expect("the leaf");

    assert_eq!(
        write.get_one::<String>("owner").map(String::as_str),
        Some("site")
    );
    assert!(write.get_flag("dry-run"));
    assert!(write.get_flag("force"));
    assert!(write.get_flag("allow-external-targets"));
}

/// An unset shell variable interpolates to an empty argument, and no argument
/// carrying a path or a name reads one as a value: `--dest ""` is not the
/// working directory, `--owner ""` is not an owner, and `config get ""` names
/// no key. Every one of them is a usage error at the command line, before a
/// run reaches the destination or clapfig is asked for anything. The last six
/// are clapfig's own arguments, which [`command`] patches.
#[test]
fn no_argument_reads_an_empty_string_as_a_value() {
    for argv in [
        vec!["proiectio", "status", "--dest", ""],
        vec!["proiectio", "status", "--state-dir", ""],
        vec!["proiectio", "write", "deploy.toml", "--dest", ""],
        vec!["proiectio", "write", "deploy.toml", "--state-dir", ""],
        vec!["proiectio", "write", "deploy.toml", "--owner", ""],
        vec!["proiectio", "write", ""],
        vec!["proiectio", "write", "--tree", ""],
        vec!["proiectio", "rm", "--dest", ""],
        vec!["proiectio", "rm", "--state-dir", ""],
        vec!["proiectio", "rm", "--owner", ""],
        vec!["proiectio", "rm", ""],
        vec!["proiectio", "config", "gen", "--file", ""],
        vec!["proiectio", "config", "schema", "--file", ""],
        vec!["proiectio", "config", "get", ""],
        vec!["proiectio", "config", "set", "", "site"],
        vec!["proiectio", "config", "unset", ""],
        vec!["proiectio", "config", "list", "--scope", ""],
    ] {
        let error = command()
            .try_get_matches_from(&argv)
            .expect_err(&format!("a usage error for {argv:?}"));

        assert_eq!(
            error.kind(),
            ErrorKind::ValueValidation,
            "{argv:?}: {error}"
        );
    }
}

/// An owner is refused blank as well as empty: it is a name the manifest
/// records and a listing prints, so a blank one is an owner no reader of that
/// file can see. A path is left to the filesystem, which answers for a blank
/// name itself.
#[test]
fn an_owner_that_is_nothing_but_whitespace_is_refused() {
    for argv in [
        vec!["proiectio", "write", "deploy.toml", "--owner", "  "],
        vec!["proiectio", "rm", "--owner", "\t"],
    ] {
        let error = command()
            .try_get_matches_from(&argv)
            .expect_err(&format!("a usage error for {argv:?}"));

        assert_eq!(
            error.kind(),
            ErrorKind::ValueValidation,
            "{argv:?}: {error}"
        );
        assert!(
            error.to_string().contains(libproiectio::OWNER_RULE),
            "{argv:?}: {error}"
        );
    }
}

/// The rule refuses a name with nothing in it, not a name with a space in it:
/// an owner, a destination and a path each keep every character they carry.
#[test]
fn a_value_with_a_space_in_it_is_a_value() {
    let matches = command()
        .try_get_matches_from([
            "proiectio",
            "write",
            "my mapping.toml",
            "--dest",
            "/srv/my site",
            "--owner",
            "my site",
        ])
        .expect("a parsed command line");
    let write = matches.subcommand_matches("write").expect("the leaf");

    assert_eq!(
        write.get_one::<String>("owner").map(String::as_str),
        Some("my site")
    );
    assert_eq!(
        write.get_one::<String>("dest").map(String::as_str),
        Some("/srv/my site")
    );
    assert_eq!(
        write
            .get_many::<Utf8PathBuf>("paths")
            .map(|values| values.map(|value| value.as_str()).collect::<Vec<_>>())
            .unwrap_or_default(),
        ["my mapping.toml"]
    );
}

// The `config` help names its keys in prose, and prose does not follow a
// schema that gains one — the count in front of them least of all. This is
// the pair that keeps the two together: every key the schema declares is
// named in the help, and the number promised is the number there are.
#[test]
fn the_config_help_names_every_key_the_schema_declares() {
    let keys = crate::settings::declared_keys();

    assert!(
        CONFIG_ABOUT.contains(&format!("Available keys ({}):", keys.len())),
        "{CONFIG_ABOUT}"
    );
    for key in &keys {
        assert!(
            CONFIG_ABOUT.contains(key.as_str()),
            "the config help does not name {key}"
        );
    }
}
