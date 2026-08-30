use super::*;

use tempfile::TempDir;

/// Loads a `proiectio.toml` from `dir` alone: [`builder`]'s own scopes reach
/// the platform config directory, which a unit test must not read.
fn load_from(dir: &TempDir) -> ProiectioConfig {
    Clapfig::typed::<ProiectioConfig>()
        .app_name("proiectio")
        .search_paths(vec![SearchPath::Path(dir.path().to_path_buf())])
        .no_env()
        .load()
        .expect("a loaded configuration")
}

#[test]
fn an_omitted_key_takes_its_compiled_default() {
    let dir = TempDir::new().expect("a temporary directory");

    assert_eq!(load_from(&dir).owner, "default");
}

#[test]
fn a_file_declaring_the_owner_loads_it() {
    let dir = TempDir::new().expect("a temporary directory");
    std::fs::write(dir.path().join("proiectio.toml"), "owner = \"site\"\n").expect("a config file");

    assert_eq!(load_from(&dir).owner, "site");
}
