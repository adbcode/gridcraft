//! Configuration is tested in subprocesses to avoid mutating shared process environment.
use std::process::Command;

#[test]
fn invalid_ca_is_reported_before_connection_without_disclosing_it() -> Result<(), Box<dyn std::error::Error>> {
    let file = tempfile::NamedTempFile::new()?;
    for (key, value, expected) in [
        ("DATABASE_SSL_ROOT_CERT_PEM", "-----BEGIN CERTIFICATE-----\\nsensitive-certificate\\n-----END CERTIFICATE-----", "literal newline escapes"),
        ("DATABASE_SSL_ROOT_CERT_BASE64", "sensitive-certificate!", "not valid standard base64"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gridcraft-server"))
            .arg("migrate")
            .env_clear()
            .env("GRIDCRAFT_ENV_FILE", file.path())
            .env("DATABASE_URL", "postgresql://user:sensitive-password@127.0.0.1:1/private-db")
            .env(key, value)
            .output()?;
        assert!(!output.status.success());
        let combined = format!("{}{}", String::from_utf8(output.stdout)?, String::from_utf8(output.stderr)?);
        assert!(combined.contains("invalid service configuration") && combined.contains(expected));
        assert!(!combined.contains("during connection"));
        for forbidden in ["sensitive-certificate", "sensitive-password", "private-db", "BEGIN CERTIFICATE"] {
            assert!(!combined.contains(forbidden));
        }
    }
    Ok(())
}

#[test]
fn runtime_environment_overrides_private_dotenv_without_exposing_values() -> Result<(), Box<dyn std::error::Error>> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "DATABASE_URL='invalid-local-secret'\nOPENAI_API_KEY='local-secret'\nOPENAI_MODEL='local-model'\n")?;
    let output = Command::new(env!("CARGO_BIN_EXE_gridcraft-server"))
        .arg("check-config")
        .env_clear()
        .env("GRIDCRAFT_ENV_FILE", file.path())
        .env("DATABASE_URL", "")
        .env("OPENAI_API_KEY", "runtime-secret")
        .env("OPENAI_MODEL", "runtime-model")
        .output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stdout.contains("Database configured: false; OpenAI configured: true"));
    assert!(!stdout.contains("secret") && !stderr.contains("secret"));
    Ok(())
}

#[test]
fn invalid_configuration_does_not_print_secrets_or_make_calls() -> Result<(), Box<dyn std::error::Error>> {
    let file = tempfile::NamedTempFile::new()?;
    let output = Command::new(env!("CARGO_BIN_EXE_gridcraft-server"))
        .arg("check-config")
        .env_clear()
        .env("GRIDCRAFT_ENV_FILE", file.path())
        .env("DATABASE_URL", "do-not-print-this-password")
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("invalid service configuration"));
    assert!(!stderr.contains("do-not-print"));
    Ok(())
}
