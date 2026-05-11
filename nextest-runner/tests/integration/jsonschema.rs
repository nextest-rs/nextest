use camino::{Utf8Path, Utf8PathBuf};

fn repository_root() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("nextest-runner should be within the repository root")
        .to_path_buf()
}

#[test]
fn validates_nextest_config() {
    // Read JSON Schema
    let schema_path = repository_root().join("jsonschemas/nextest.json");
    let Ok(schema) = std::fs::read_to_string(schema_path) else {
        panic!("Failed to read schema file");
    };
    let Ok(schema) = serde_json::from_str(&schema) else {
        panic!("Failed to parse schema file");
    };
    let Ok(validator) = jsonschema::validator_for(&schema) else {
        panic!("Failed to create validator");
    };

    // Read `nextest.toml`
    let config_path = repository_root().join(".config/nextest.toml");
    let Ok(config) = std::fs::read_to_string(config_path) else {
        panic!("Failed to read config file");
    };
    let Ok(config) = toml::from_str(&config) else {
        panic!("Failed to parse config file");
    };

    // Validate `nextest.toml`
    assert!(
        validator.validate(&config).is_ok(),
        "Config file is not valid"
    );
}
