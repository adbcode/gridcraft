//! Environment-only service configuration; a private local dotenv file is optional.
//! Never derive Debug for credentials or print connection/provider errors verbatim.

use std::{
    collections::BTreeMap,
    io::Read,
    net::IpAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use sqlx::{
    ConnectOptions,
    postgres::{PgConnectOptions, PgSslMode},
};
use uuid::Uuid;

use crate::providers::{ElevenLabsConfig, OpenAiConfig};

pub struct Config {
    pub bind: IpAddr,
    pub port: u16,
    pub static_dir: PathBuf,
    pub workspace_id: Uuid,
    pub migrate_on_start: bool,
    pub database: Option<DatabaseConfig>,
    pub openai: Option<OpenAiConfig>,
    pub elevenlabs: Option<ElevenLabsConfig>,
}

pub struct DatabaseConfig {
    url: String,
    ssl_mode: PgSslMode,
    root_cert: Option<PathBuf>,
    root_cert_pem: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid service configuration: {0}")]
pub struct ConfigError(pub &'static str);

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let explicit = std::env::var_os("GRIDCRAFT_ENV_FILE");
        let path = explicit.as_deref().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".env"));
        let mut values = if path.exists() {
            read_dotenv(&path)?
        } else if explicit.is_some() {
            return Err(ConfigError("GRIDCRAFT_ENV_FILE does not exist"));
        } else {
            BTreeMap::new()
        };
        // Do not mutate process environment (or source dotenv as executable shell code).
        for (key, value) in std::env::vars_os() {
            if let (Ok(key), Ok(value)) = (key.into_string(), value.into_string()) {
                values.insert(key, value);
            }
        }
        Self::from_values(&values)
    }

    fn from_values(values: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        let value = |key: &str| values.get(key).filter(|v| !v.trim().is_empty()).map(String::as_str);
        let bind = value("GRIDCRAFT_BIND").unwrap_or("127.0.0.1").parse().map_err(|_| ConfigError("GRIDCRAFT_BIND must be an IP address"))?;
        let port = value("PORT").unwrap_or("8080").parse::<u16>().map_err(|_| ConfigError("PORT must be 1..65535"))?;
        if port == 0 {
            return Err(ConfigError("PORT must be 1..65535"));
        }
        let workspace_id = value("GRIDCRAFT_WORKSPACE_ID")
            .unwrap_or("00000000-0000-4000-8000-000000000001")
            .parse()
            .map_err(|_| ConfigError("GRIDCRAFT_WORKSPACE_ID must be a UUID"))?;
        let migrate_on_start = match value("GRIDCRAFT_MIGRATE_ON_START").unwrap_or("false") {
            "true" => true,
            "false" => false,
            _ => return Err(ConfigError("GRIDCRAFT_MIGRATE_ON_START must be true or false")),
        };
        let database = if let Some(url) = value("DATABASE_URL") {
            let parsed = reqwest::Url::parse(url).map_err(|_| ConfigError("DATABASE_URL must be a PostgreSQL URI"))?;
            if !matches!(parsed.scheme(), "postgres" | "postgresql") || parsed.host_str().is_none() || parsed.path().trim_matches('/').is_empty() {
                return Err(ConfigError("DATABASE_URL must include a PostgreSQL host and database"));
            }
            let ssl_mode = match value("DATABASE_SSL_MODE").unwrap_or("verify-full") {
                "verify-full" => PgSslMode::VerifyFull,
                "disable" if is_loopback(parsed.host_str().unwrap_or_default()) => PgSslMode::Disable,
                _ => return Err(ConfigError("DATABASE_SSL_MODE must be verify-full; disable is allowed only for loopback PostgreSQL")),
            };
            let root_cert = value("DATABASE_SSL_ROOT_CERT").map(PathBuf::from);
            let root_cert_pem = value("DATABASE_SSL_ROOT_CERT_PEM").map(str::to_owned);
            if root_cert.is_some() && root_cert_pem.is_some() {
                return Err(ConfigError("configure only one PostgreSQL CA source"));
            }
            let db = DatabaseConfig { url: url.to_owned(), ssl_mode, root_cert, root_cert_pem };
            db.options()?;
            Some(db)
        } else {
            None
        };
        if migrate_on_start && database.is_none() {
            return Err(ConfigError("startup migrations require DATABASE_URL"));
        }
        let openai = match (value("OPENAI_API_KEY"), value("OPENAI_MODEL")) {
            (Some(key), Some(model)) => Some(OpenAiConfig {
                api_key: key.to_owned(),
                model: model.to_owned(),
                base_url: value("OPENAI_BASE_URL").unwrap_or("https://api.openai.com/v1").to_owned(),
                reasoning_effort: value("OPENAI_REASONING_EFFORT").map(str::to_owned),
            }),
            (None, None) => None,
            _ => return Err(ConfigError("set both OPENAI_API_KEY and OPENAI_MODEL, or leave both empty")),
        };
        let elevenlabs = value("ELEVENLABS_API_KEY").map(|key| ElevenLabsConfig {
            api_key: key.to_owned(),
            base_url: value("ELEVENLABS_BASE_URL").unwrap_or("https://api.elevenlabs.io/v1").to_owned(),
            stt_model: value("ELEVENLABS_STT_MODEL").unwrap_or("scribe_v2").to_owned(),
            tts_model: value("ELEVENLABS_TTS_MODEL").unwrap_or("eleven_multilingual_v2").to_owned(),
            voice_id: value("ELEVENLABS_VOICE_ID").map(str::to_owned),
        });
        Ok(Self {
            bind,
            port,
            static_dir: value("GRIDCRAFT_STATIC_DIR").unwrap_or("dist/web").into(),
            workspace_id,
            migrate_on_start,
            database,
            openai,
            elevenlabs,
        })
    }
}

impl DatabaseConfig {
    pub fn options(&self) -> Result<PgConnectOptions, ConfigError> {
        // Force the verified policy even if the Aiven URI includes sslmode=require.
        // Explicit URI SSL parameters cannot weaken this setting.
        let mut options = PgConnectOptions::from_str(&self.url)
            .map_err(|_| ConfigError("DATABASE_URL could not be parsed"))?
            .ssl_mode(self.ssl_mode)
            .application_name("gridcraft-server")
            .disable_statement_logging();
        if matches!(self.ssl_mode, PgSslMode::Disable) && !is_loopback(options.get_host()) {
            return Err(ConfigError("PostgreSQL URI overrides must not disable TLS for a remote host"));
        }
        if let Some(path) = &self.root_cert {
            if !path.is_file() {
                return Err(ConfigError("DATABASE_SSL_ROOT_CERT is not a readable certificate file"));
            }
            options = options.ssl_root_cert(path);
        }
        if let Some(pem) = &self.root_cert_pem {
            options = options.ssl_root_cert_from_pem(pem.as_bytes().to_vec());
        }
        Ok(options)
    }
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.trim_matches(['[', ']']).parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn read_dotenv(path: &Path) -> Result<BTreeMap<String, String>, ConfigError> {
    let file = std::fs::File::open(path).map_err(|_| ConfigError("cannot read local dotenv file"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata().map_err(|_| ConfigError("cannot inspect dotenv permissions"))?.permissions().mode() & 0o077 != 0 {
            return Err(ConfigError("local dotenv file must be private; run chmod 600 .env"));
        }
    }
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes).map_err(|_| ConfigError("cannot read local dotenv file"))?;
    if bytes.len() > 65_536 {
        return Err(ConfigError("local dotenv file exceeds 64 KiB"));
    }
    dotenvy::from_read_iter(bytes.as_slice()).map(|pair| pair.map_err(|_| ConfigError("invalid local dotenv syntax"))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn offline_defaults_and_partial_provider_config() {
        let config = Config::from_values(&BTreeMap::new()).unwrap();
        assert!(config.bind.is_loopback());
        assert!(config.database.is_none() && config.openai.is_none());
        let error = Config::from_values(&values(&[("OPENAI_API_KEY", "sensitive-key")])).err().unwrap().to_string();
        assert!(!error.contains("sensitive-key"));
        assert!(Config::from_values(&values(&[("GRIDCRAFT_MIGRATE_ON_START", "true")])).is_err());
    }

    #[test]
    fn database_tls_cannot_be_downgraded_by_uri() {
        let config = Config::from_values(&values(&[("DATABASE_URL", "postgresql://user:secret@db.example/db?sslmode=disable")])).unwrap();
        assert!(matches!(config.database.unwrap().options().unwrap().get_ssl_mode(), PgSslMode::VerifyFull));
        let remote = values(&[("DATABASE_URL", "postgresql://user:secret@db.example/db"), ("DATABASE_SSL_MODE", "disable")]);
        assert!(Config::from_values(&remote).is_err());
        let local = values(&[("DATABASE_URL", "postgresql://user:secret@127.0.0.1/db"), ("DATABASE_SSL_MODE", "disable")]);
        assert!(Config::from_values(&local).is_ok());
        let overridden = values(&[("DATABASE_URL", "postgresql://user:secret@127.0.0.1/db?host=db.example"), ("DATABASE_SSL_MODE", "disable")]);
        assert!(Config::from_values(&overridden).is_err());
    }

    #[test]
    fn parsing_errors_do_not_expose_credentials() {
        let error = Config::from_values(&values(&[("DATABASE_URL", "sensitive-value")])).err().unwrap().to_string();
        assert!(!error.contains("sensitive-value"));
    }

    #[test]
    fn dotenv_is_data_and_external_empty_value_can_disable_it() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "OPENAI_API_KEY='local-secret'\nOPENAI_MODEL=test\n").unwrap();
        let mut config = read_dotenv(file.path()).unwrap();
        config.insert("OPENAI_API_KEY".into(), String::new());
        config.insert("OPENAI_MODEL".into(), String::new());
        assert!(Config::from_values(&config).unwrap().openai.is_none());
    }
}
