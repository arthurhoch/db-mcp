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
            Engine::SqlServer => "dbo".into(),
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
    let table = if matches!(
        request.include,
        MetadataKind::Columns | MetadataKind::Indexes | MetadataKind::Constraints
    ) {
        Some(
            request
                .table
                .as_ref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::invalid("columns, indexes e constraints exigem table."))?,
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
        (Engine::SqlServer, MetadataKind::Summary) => (
            "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)) AS version, SCHEMA_NAME() AS default_schema",
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
        (Engine::SqlServer, MetadataKind::Schemas) => (
            "SELECT SCHEMA_NAME AS schema_name FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME",
            vec![],
        ),
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
        (Engine::SqlServer, MetadataKind::Tables) => (
            "SELECT TABLE_NAME AS table_name, TABLE_TYPE AS table_type FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA=@P1 ORDER BY TABLE_NAME",
            vec![text(schema)],
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
        (Engine::SqlServer, MetadataKind::Columns) => (
            "SELECT COLUMN_NAME AS column_name, DATA_TYPE AS data_type, IS_NULLABLE AS is_nullable, ORDINAL_POSITION AS ordinal_position FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA=@P1 AND TABLE_NAME=@P2 ORDER BY ORDINAL_POSITION",
            vec![text(schema), table_parameter(table)?],
        ),
        (engine, MetadataKind::Indexes | MetadataKind::Constraints) => (
            table_metadata(engine, request.include)?,
            if engine == Engine::Sqlite {
                vec![table_parameter(table)?]
            } else {
                vec![text(schema), table_parameter(table)?]
            },
        ),
    };
    Ok(PreparedStatement {
        sql: sql.into(),
        parameters,
        kind: StatementKind::Read,
        relations: Default::default(),
    })
}

fn table_parameter(table: Option<&String>) -> Result<BoundValue> {
    table
        .cloned()
        .map(BoundValue::String)
        .ok_or_else(|| AppError::invalid("Nome de tabela ausente."))
}

fn table_metadata(engine: Engine, kind: MetadataKind) -> Result<&'static str> {
    Ok(match (engine, kind) {
        (Engine::Postgresql, MetadataKind::Indexes) => {
            r"
            SELECT idx.relname AS index_name, pg_catalog.pg_get_indexdef(idx.oid, key.pos, true) AS column_name,
                   key.pos AS ordinal_position, i.indisunique AS is_unique
            FROM pg_catalog.pg_index i
            JOIN pg_catalog.pg_class t ON t.oid = i.indrelid
            JOIN pg_catalog.pg_class idx ON idx.oid = i.indexrelid
            JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace
            CROSS JOIN LATERAL pg_catalog.generate_series(1, i.indnkeyatts::int) AS key(pos)
            WHERE n.nspname = $1 AND t.relname = $2
              AND pg_catalog.has_table_privilege(t.oid, 'SELECT')
            ORDER BY index_name, ordinal_position"
        }
        (Engine::Mysql, MetadataKind::Indexes) => {
            r"
            SELECT INDEX_NAME AS index_name, COLUMN_NAME AS column_name,
                   SEQ_IN_INDEX AS ordinal_position, NON_UNIQUE = 0 AS is_unique
            FROM information_schema.STATISTICS
            WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
            ORDER BY INDEX_NAME, SEQ_IN_INDEX"
        }
        (Engine::Oracle, MetadataKind::Indexes) => {
            r"
            SELECT c.index_name, c.column_name, c.column_position AS ordinal_position,
                   CASE WHEN i.uniqueness = 'UNIQUE' THEN 1 ELSE 0 END AS is_unique
            FROM all_ind_columns c
            JOIN all_indexes i ON i.owner = c.index_owner AND i.index_name = c.index_name
            WHERE c.table_owner = :1 AND c.table_name = :2
            ORDER BY c.index_name, c.column_position"
        }
        (Engine::Sqlite, MetadataKind::Indexes) => {
            r#"
            SELECT i.name AS index_name, c.name AS column_name, c.seqno + 1 AS ordinal_position,
                   i."unique" AS is_unique
            FROM pragma_index_list(?1, 'main') i
            JOIN pragma_index_xinfo(i.name, 'main') c
            WHERE c."key" = 1
            ORDER BY index_name, ordinal_position"#
        }
        (Engine::SqlServer, MetadataKind::Indexes) => {
            r"
            SELECT i.name AS index_name, c.name AS column_name, ic.key_ordinal AS ordinal_position,
                   i.is_unique
            FROM sys.indexes i
            JOIN sys.tables t ON t.object_id = i.object_id
            JOIN sys.schemas s ON s.schema_id = t.schema_id
            JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id
            JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
            WHERE s.name = @P1 AND t.name = @P2 AND ic.key_ordinal > 0
            ORDER BY index_name, ordinal_position"
        }
        (Engine::Postgresql, MetadataKind::Constraints) => {
            r"
            SELECT k.conname AS constraint_name,
                   CASE k.contype WHEN 'p' THEN 'PRIMARY KEY' WHEN 'u' THEN 'UNIQUE'
                                  WHEN 'f' THEN 'FOREIGN KEY' WHEN 'c' THEN 'CHECK' END AS constraint_type,
                   a.attname AS column_name, key.pos::int AS ordinal_position,
                   rn.nspname AS referenced_schema, rt.relname AS referenced_table,
                   ra.attname AS referenced_column
            FROM pg_catalog.pg_constraint k
            JOIN pg_catalog.pg_class t ON t.oid = k.conrelid
            JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace
            LEFT JOIN LATERAL pg_catalog.unnest(k.conkey) WITH ORDINALITY AS key(attnum, pos) ON true
            LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = t.oid AND a.attnum = key.attnum
            LEFT JOIN pg_catalog.pg_class rt ON rt.oid = k.confrelid
            LEFT JOIN pg_catalog.pg_namespace rn ON rn.oid = rt.relnamespace
            LEFT JOIN pg_catalog.pg_attribute ra ON ra.attrelid = rt.oid AND ra.attnum = k.confkey[key.pos]
            WHERE n.nspname = $1 AND t.relname = $2 AND k.contype IN ('p','u','f','c')
              AND pg_catalog.has_table_privilege(t.oid, 'SELECT')
            ORDER BY constraint_name, ordinal_position"
        }
        (Engine::Mysql, MetadataKind::Constraints) => {
            r"
            SELECT t.CONSTRAINT_NAME AS constraint_name, t.CONSTRAINT_TYPE AS constraint_type,
                   k.COLUMN_NAME AS column_name, k.ORDINAL_POSITION AS ordinal_position,
                   k.REFERENCED_TABLE_SCHEMA AS referenced_schema, k.REFERENCED_TABLE_NAME AS referenced_table,
                   k.REFERENCED_COLUMN_NAME AS referenced_column
            FROM information_schema.TABLE_CONSTRAINTS t
            LEFT JOIN information_schema.KEY_COLUMN_USAGE k
              ON k.CONSTRAINT_SCHEMA = t.CONSTRAINT_SCHEMA AND k.TABLE_SCHEMA = t.TABLE_SCHEMA
             AND k.TABLE_NAME = t.TABLE_NAME AND k.CONSTRAINT_NAME = t.CONSTRAINT_NAME
            WHERE t.TABLE_SCHEMA = ? AND t.TABLE_NAME = ?
            ORDER BY t.CONSTRAINT_NAME, k.ORDINAL_POSITION"
        }
        (Engine::Oracle, MetadataKind::Constraints) => {
            r"
            SELECT k.constraint_name,
                   CASE k.constraint_type WHEN 'P' THEN 'PRIMARY KEY' WHEN 'U' THEN 'UNIQUE'
                                          WHEN 'R' THEN 'FOREIGN KEY' WHEN 'C' THEN 'CHECK' END AS constraint_type,
                   c.column_name, c.position AS ordinal_position,
                   r.owner AS referenced_schema, r.table_name AS referenced_table, r.column_name AS referenced_column
            FROM all_constraints k
            LEFT JOIN all_cons_columns c ON c.owner = k.owner AND c.constraint_name = k.constraint_name
            LEFT JOIN all_cons_columns r ON r.owner = k.r_owner AND r.constraint_name = k.r_constraint_name
                                        AND r.position = c.position
            WHERE k.owner = :1 AND k.table_name = :2 AND k.constraint_type IN ('P','U','R','C')
            ORDER BY k.constraint_name, c.position"
        }
        (Engine::Sqlite, MetadataKind::Constraints) => {
            r#"
            SELECT 'PRIMARY KEY' AS constraint_name, 'PRIMARY KEY' AS constraint_type,
                   name AS column_name, pk AS ordinal_position,
                   NULL AS referenced_schema, NULL AS referenced_table, NULL AS referenced_column
            FROM pragma_table_info(?1, 'main') WHERE pk > 0
            UNION ALL
            SELECT i.name, 'UNIQUE', c.name, c.seqno + 1, NULL, NULL, NULL
            FROM pragma_index_list(?1, 'main') i
            JOIN pragma_index_xinfo(i.name, 'main') c
            WHERE i.origin = 'u' AND c."key" = 1
            UNION ALL
            SELECT 'FK ' || id, 'FOREIGN KEY', "from", seq + 1, 'main', "table", "to"
            FROM pragma_foreign_key_list(?1, 'main')
            ORDER BY constraint_name, ordinal_position"#
        }
        (Engine::SqlServer, MetadataKind::Constraints) => {
            r"
            WITH target AS (
                SELECT t.object_id FROM sys.tables t JOIN sys.schemas s ON s.schema_id = t.schema_id
                WHERE s.name = @P1 AND t.name = @P2
            )
            SELECT k.name AS constraint_name,
                   CASE k.type WHEN 'PK' THEN 'PRIMARY KEY' ELSE 'UNIQUE' END AS constraint_type,
                   c.name AS column_name, CAST(ic.key_ordinal AS int) AS ordinal_position,
                   CAST(NULL AS nvarchar(128)) AS referenced_schema,
                   CAST(NULL AS nvarchar(128)) AS referenced_table,
                   CAST(NULL AS nvarchar(128)) AS referenced_column
            FROM target t
            JOIN sys.key_constraints k ON k.parent_object_id = t.object_id
            JOIN sys.index_columns ic ON ic.object_id = t.object_id AND ic.index_id = k.unique_index_id
            JOIN sys.columns c ON c.object_id = t.object_id AND c.column_id = ic.column_id
            WHERE ic.key_ordinal > 0
            UNION ALL
            SELECT f.name, 'FOREIGN KEY', c.name, fk.constraint_column_id,
                   OBJECT_SCHEMA_NAME(fk.referenced_object_id), OBJECT_NAME(fk.referenced_object_id), r.name
            FROM target t
            JOIN sys.foreign_keys f ON f.parent_object_id = t.object_id
            JOIN sys.foreign_key_columns fk ON fk.constraint_object_id = f.object_id
            JOIN sys.columns c ON c.object_id = t.object_id AND c.column_id = fk.parent_column_id
            LEFT JOIN sys.columns r ON r.object_id = fk.referenced_object_id AND r.column_id = fk.referenced_column_id
            UNION ALL
            SELECT k.name, 'CHECK', c.name, NULL, NULL, NULL, NULL
            FROM target t
            JOIN sys.check_constraints k ON k.parent_object_id = t.object_id
            LEFT JOIN sys.columns c ON c.object_id = t.object_id AND c.column_id = k.parent_column_id
            ORDER BY constraint_name, ordinal_position"
        }
        _ => return Err(AppError::invalid("Tipo de metadado de tabela inválido.")),
    })
}
