use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{AppError, ErrorCode, Result};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Oracle,
    Postgresql,
    Mysql,
    Sqlite,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oracle => "oracle",
            Self::Postgresql => "postgresql",
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
    ReadWriteDdl,
}

#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    pub config_version: u32,
    pub databases_dir: PathBuf,
    pub server: ServerConfig,
    #[serde(default)]
    pub execution: ExecutionLimits,
    #[serde(default)]
    pub connections: ConnectionLimits,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub transport: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionLimits {
    pub query_timeout_seconds: u64,
    pub max_rows: usize,
    pub max_result_bytes: usize,
    pub max_sql_bytes: usize,
    pub max_parameter_bytes: usize,
    pub max_batch_statements: usize,
    pub max_concurrent_operations: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            query_timeout_seconds: 30,
            max_rows: 500,
            max_result_bytes: 1_048_576,
            max_sql_bytes: 262_144,
            max_parameter_bytes: 262_144,
            max_batch_statements: 50,
            max_concurrent_operations: 4,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectionLimits {
    pub max_open_total: usize,
    pub max_open_per_database: usize,
    pub acquire_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_open_total: 16,
            max_open_per_database: 4,
            acquire_timeout_seconds: 10,
            idle_timeout_seconds: 300,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseFile {
    config_version: u32,
    id: String,
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    description: String,
    engine: Engine,
    access_mode: AccessMode,
    default_schema: Option<String>,
    connection: ConnectionFile,
    execution: Option<ExecutionOverrides>,
}

// Deliberately no Debug: this is unvalidated input and may contain literal secrets.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionFile {
    url: Option<String>,
    url_env: Option<String>,
    username: Option<String>,
    username_env: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
}

impl fmt::Debug for ConnectionFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConnectionFile([REDACTED])")
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionOverrides {
    query_timeout_seconds: Option<u64>,
    max_rows: Option<usize>,
    max_result_bytes: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct DatabaseConfig {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub engine: Engine,
    pub access_mode: AccessMode,
    pub default_schema: Option<String>,
    pub url: Secret,
    pub username: Option<Secret>,
    pub password: Option<Secret>,
    pub sqlite_path: Option<PathBuf>,
    pub limits: ExecutionLimits,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub global: GlobalConfig,
    pub databases: BTreeMap<String, Arc<DatabaseConfig>>,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self> {
        let global: GlobalConfig = read_toml(path)?;
        validate_global(&global)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let directory = parent.join(&global.databases_dir);
        let entries = fs::read_dir(&directory)
            .map_err(|_| config_error("Não foi possível ler databases_dir."))?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|_| config_error("Não foi possível ler uma entrada de databases_dir."))?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                let kind = entry
                    .file_type()
                    .map_err(|_| config_error("Arquivo de conexão ilegível."))?;
                if !kind.is_file() {
                    return Err(config_error(
                        "Cada conexão deve ser um arquivo regular, sem symlink.",
                    ));
                }
                paths.push(path);
            }
        }
        paths.sort();
        let mut databases = BTreeMap::new();
        for path in paths {
            let database = resolve_database(read_toml(&path)?, &path, &global.execution)?;
            if databases
                .insert(database.id.clone(), Arc::new(database))
                .is_some()
            {
                return Err(config_error("ID de conexão duplicado."));
            }
        }
        Ok(Self { global, databases })
    }

    pub fn database(&self, id: &str) -> Result<Arc<DatabaseConfig>> {
        self.databases.get(id).cloned().ok_or_else(|| {
            AppError::new(
                ErrorCode::DatabaseNotAvailable,
                "ID de banco não encontrado no catálogo local.",
            )
        })
    }
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path)
        .map_err(|_| config_error("Não foi possível ler um arquivo de configuração."))?;
    if text.len() > 1_048_576 {
        return Err(config_error("Arquivo de configuração excede 1 MiB."));
    }
    toml::from_str(&text).map_err(|_| {
        // TOML diagnostics may quote credentials. Never forward the parser error.
        config_error(format!(
            "TOML inválido ou campos incompatíveis em {}.",
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("configuração")
        ))
    })
}

fn config_error(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::ConfigInvalid, message)
}

fn validate_global(config: &GlobalConfig) -> Result<()> {
    let e = &config.execution;
    let c = &config.connections;
    if config.config_version != 1 || config.server.transport != "stdio" {
        return Err(config_error(
            "A configuração exige versão 1 e transporte stdio.",
        ));
    }
    if [
        e.max_rows,
        e.max_result_bytes,
        e.max_sql_bytes,
        e.max_parameter_bytes,
        e.max_batch_statements,
        e.max_concurrent_operations,
        c.max_open_total,
        c.max_open_per_database,
    ]
    .contains(&0)
        || e.query_timeout_seconds == 0
        || c.acquire_timeout_seconds == 0
        || c.idle_timeout_seconds == 0
        || e.max_result_bytes < 1024
    {
        return Err(config_error(
            "Limites devem ser positivos; max_result_bytes deve ser pelo menos 1024.",
        ));
    }
    if !["error", "warn", "info", "debug"].contains(&config.logging.level.as_str()) {
        return Err(config_error("Nível de log inválido."));
    }
    Ok(())
}

fn source(direct: Option<String>, env: Option<String>, required: bool) -> Result<Option<Secret>> {
    let resolved = match (direct, env) {
        (Some(_), Some(_)) => {
            return Err(config_error(
                "Fonte direta e referência de ambiente são mutuamente exclusivas.",
            ));
        }
        (Some(value), None) => Some(value),
        (None, Some(name)) => Some(
            std::env::var(name)
                .map_err(|_| config_error("Referência de ambiente ausente ou inválida."))?,
        ),
        (None, None) => None,
    };
    if resolved.as_ref().is_some_and(String::is_empty) || (required && resolved.is_none()) {
        return Err(config_error(
            "Campo de conexão obrigatório ausente ou vazio.",
        ));
    }
    Ok(resolved.map(Secret))
}

fn resolve_database(
    file: DatabaseFile,
    path: &Path,
    defaults: &ExecutionLimits,
) -> Result<DatabaseConfig> {
    let valid_id = !file.id.is_empty()
        && file.id.len() <= 64
        && file.id.as_bytes()[0].is_ascii_lowercase()
        && file
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if file.config_version != 1
        || !valid_id
        || file.name.trim().is_empty()
        || file.description.trim().is_empty()
    {
        return Err(config_error(
            "Versão, ID, nome ou descrição de conexão inválidos.",
        ));
    }
    let url = source(file.connection.url, file.connection.url_env, true)?
        .ok_or_else(|| config_error("URL obrigatória."))?;
    let network = file.engine != Engine::Sqlite;
    let username = source(
        file.connection.username,
        file.connection.username_env,
        network,
    )?;
    let password = source(
        file.connection.password,
        file.connection.password_env,
        network,
    )?;
    let address = Url::parse(url.expose()).map_err(|_| config_error("URL de conexão inválida."))?;
    if address.scheme() != file.engine.as_str()
        || !address.username().is_empty()
        || address.password().is_some()
        || address.fragment().is_some()
        || address.query().is_some()
        || address.port() == Some(0)
    {
        return Err(config_error(
            "URL incompatível: use o scheme do engine, credenciais separadas e nenhum parâmetro livre.",
        ));
    }
    if file.engine == Engine::Sqlite
        && file
            .default_schema
            .as_deref()
            .is_some_and(|schema| schema != "main")
    {
        return Err(config_error(
            "SQLite oferece somente o schema main nesta versão.",
        ));
    }
    if file.engine == Engine::Mysql
        && let Some(schema) = &file.default_schema
    {
        let database_name =
            percent_encoding::percent_decode_str(address.path().trim_start_matches('/'))
                .decode_utf8()
                .map_err(|_| config_error("Database MySQL não é UTF-8 válido."))?;
        if schema != database_name.as_ref() {
            return Err(config_error(
                "default_schema MySQL deve corresponder ao database da URL.",
            ));
        }
    }
    let sqlite_path = if network {
        if address.host_str().is_none() || address.path().trim_matches('/').is_empty() {
            return Err(config_error("URL exige host e database/service name."));
        }
        if file.engine == Engine::Oracle
            && !address
                .path()
                .trim_start_matches('/')
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(config_error(
                "Service name Oracle inválido para a conexão simples.",
            ));
        }
        None
    } else {
        if username.is_some() || password.is_some() || address.host_str().is_some() {
            return Err(config_error(
                "SQLite exige URL de arquivo sem usuário, senha ou host.",
            ));
        }
        let raw = url
            .expose()
            .strip_prefix("sqlite:")
            .ok_or_else(|| config_error("URL SQLite inválida."))?;
        let decoded = percent_encoding::percent_decode_str(raw)
            .decode_utf8()
            .map_err(|_| config_error("Caminho SQLite não é UTF-8 válido."))?;
        let candidate = Path::new(decoded.as_ref());
        let candidate = if candidate.is_absolute() {
            candidate.to_owned()
        } else {
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(candidate)
        };
        let canonical = candidate
            .canonicalize()
            .map_err(|_| config_error("O arquivo SQLite configurado precisa existir."))?;
        if !canonical.is_file() {
            return Err(config_error(
                "A URL SQLite precisa indicar um arquivo regular.",
            ));
        }
        Some(canonical)
    };
    let mut limits = defaults.clone();
    if let Some(overrides) = file.execution {
        if let Some(value) = overrides.query_timeout_seconds {
            if value == 0 || value > defaults.query_timeout_seconds {
                return Err(config_error("Override de timeout inválido."));
            }
            limits.query_timeout_seconds = value;
        }
        if let Some(value) = overrides.max_rows {
            if value == 0 || value > defaults.max_rows {
                return Err(config_error("Override de linhas inválido."));
            }
            limits.max_rows = value;
        }
        if let Some(value) = overrides.max_result_bytes {
            if value < 1024 || value > defaults.max_result_bytes {
                return Err(config_error("Override de bytes inválido."));
            }
            limits.max_result_bytes = value;
        }
    }
    if file
        .default_schema
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.contains('\0'))
    {
        return Err(config_error("Schema padrão inválido."));
    }
    Ok(DatabaseConfig {
        id: file.id,
        name: file.name,
        aliases: file.aliases,
        description: file.description,
        engine: file.engine,
        access_mode: file.access_mode,
        default_schema: file.default_schema,
        url,
        username,
        password,
        sqlite_path,
        limits,
    })
}
