use crate::{
    config::{DatabaseConfig, Engine},
    error::{AppError, Result},
    model::{BoundValue, DescribeRequest, MetadataKind},
    policy::{PreparedStatement, StatementKind},
};

pub fn statement(config: &DatabaseConfig, request: &DescribeRequest) -> Result<PreparedStatement> {
    let schema = request
        .schema
        .clone()
        .or_else(|| config.default_schema.clone())
        .unwrap_or_else(|| match config.engine {
            Engine::Postgresql => "public".into(),
            Engine::Oracle => config
                .username
                .as_ref()
                .map_or(String::new(), |s| s.expose().to_uppercase()),
            Engine::Mysql => url::Url::parse(config.url.expose())
                .map(|u| {
                    percent_encoding::percent_decode_str(u.path().trim_start_matches('/'))
                        .decode_utf8_lossy()
                        .into_owned()
                })
                .unwrap_or_default(),
            Engine::Sqlite => "main".into(),
        });
    if schema.len() > 256
        || request
            .table
            .as_ref()
            .is_some_and(|table| table.len() > 256)
    {
        return Err(AppError::invalid(
            "Identificador de metadado excede o limite.",
        ));
    }
    let table = if request.include == MetadataKind::Columns {
        Some(
            request
                .table
                .as_ref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::invalid("columns exige table."))?,
        )
    } else {
        None
    };
    if config.engine == Engine::Sqlite && schema != "main" {
        return Err(AppError::unsupported(
            "Esta conexão SQLite oferece somente o schema main.",
        ));
    }
    let text = |value: String| BoundValue::String(value);
    let (sql, parameters) = match (config.engine, request.include) {
        (Engine::Postgresql, MetadataKind::Summary) => (
            "SELECT current_setting('server_version') AS version, current_schema() AS default_schema",
            vec![],
        ),
        (Engine::Mysql, MetadataKind::Summary) => (
            "SELECT VERSION() AS version, DATABASE() AS default_schema",
            vec![],
        ),
        (Engine::Oracle, MetadataKind::Summary) => (
            "SELECT version, SYS_CONTEXT('USERENV','CURRENT_SCHEMA') AS default_schema FROM product_component_version WHERE product LIKE 'Oracle%Database%'",
            vec![],
        ),
        (Engine::Sqlite, MetadataKind::Summary) => (
            "SELECT sqlite_version() AS version, 'main' AS default_schema",
            vec![],
        ),
        (Engine::Postgresql, MetadataKind::Schemas) => (
            "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name",
            vec![],
        ),
        (Engine::Mysql, MetadataKind::Schemas) => (
            "SELECT SCHEMA_NAME AS schema_name FROM information_schema.SCHEMATA ORDER BY SCHEMA_NAME",
            vec![],
        ),
        (Engine::Oracle, MetadataKind::Schemas) => (
            "SELECT username AS schema_name FROM all_users ORDER BY username",
            vec![],
        ),
        (Engine::Sqlite, MetadataKind::Schemas) => ("SELECT 'main' AS schema_name", vec![]),
        (Engine::Postgresql, MetadataKind::Tables) => (
            "SELECT table_name, table_type FROM information_schema.tables WHERE table_schema=$1 ORDER BY table_name",
            vec![text(schema)],
        ),
        (Engine::Mysql, MetadataKind::Tables) => (
            "SELECT TABLE_NAME AS table_name, TABLE_TYPE AS table_type FROM information_schema.TABLES WHERE TABLE_SCHEMA=? ORDER BY TABLE_NAME",
            vec![text(schema)],
        ),
        (Engine::Oracle, MetadataKind::Tables) => (
            "SELECT object_name AS table_name, object_type AS table_type FROM all_objects WHERE owner=:1 AND object_type IN ('TABLE','VIEW') ORDER BY object_name",
            vec![text(schema)],
        ),
        (Engine::Sqlite, MetadataKind::Tables) => (
            "SELECT name AS table_name, type AS table_type FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
            vec![],
        ),
        (Engine::Postgresql, MetadataKind::Columns) => (
            "SELECT column_name, data_type, is_nullable, ordinal_position FROM information_schema.columns WHERE table_schema=$1 AND table_name=$2 ORDER BY ordinal_position",
            vec![text(schema), text(table.cloned().unwrap_or_default())],
        ),
        (Engine::Mysql, MetadataKind::Columns) => (
            "SELECT COLUMN_NAME AS column_name, COLUMN_TYPE AS data_type, IS_NULLABLE AS is_nullable, ORDINAL_POSITION AS ordinal_position FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION",
            vec![text(schema), text(table.cloned().unwrap_or_default())],
        ),
        (Engine::Oracle, MetadataKind::Columns) => (
            "SELECT column_name, data_type, nullable AS is_nullable, column_id AS ordinal_position FROM all_tab_columns WHERE owner=:1 AND table_name=:2 ORDER BY column_id",
            vec![text(schema), text(table.cloned().unwrap_or_default())],
        ),
        (Engine::Sqlite, MetadataKind::Columns) => (
            "SELECT name AS column_name, type AS data_type, \"notnull\" AS not_null, cid AS ordinal_position FROM pragma_table_info(?1) ORDER BY cid",
            vec![text(table.cloned().unwrap_or_default())],
        ),
    };
    Ok(PreparedStatement {
        sql: sql.into(),
        parameters,
        kind: StatementKind::Read,
        relations: Default::default(),
    })
}
