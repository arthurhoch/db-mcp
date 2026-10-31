use std::{collections::BTreeSet, ops::ControlFlow};

use sqlparser::{
    ast::{
        ContextModifier, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArguments,
        ObjectType, Query, Select, Set, Statement, Value, Visit, Visitor,
    },
    dialect::{Dialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
    parser::Parser,
};

use crate::{
    config::{AccessMode, DatabaseConfig, Engine},
    error::{AppError, ErrorCode, Result},
    model::{BoundValue, SqlStatement},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatementKind {
    Read,
    Routine,
    Write,
    Ddl,
}

pub struct PreparedStatement {
    pub sql: String,
    pub parameters: Vec<BoundValue>,
    pub kind: StatementKind,
    pub relations: BTreeSet<String>,
}

pub fn prepare(
    config: &DatabaseConfig,
    statements: Vec<SqlStatement>,
    batch: bool,
) -> Result<Vec<PreparedStatement>> {
    let dialect: Box<dyn Dialect> = match config.engine {
        Engine::Postgresql => Box::new(PostgreSqlDialect {}),
        Engine::Mysql => Box::new(MySqlDialect {}),
        Engine::Sqlite => Box::new(SQLiteDialect {}),
        Engine::Oracle => Box::new(GenericDialect {}),
    };
    let mut prepared = Vec::with_capacity(statements.len());
    for (index, statement) in statements.into_iter().enumerate() {
        let result = prepare_one(&*dialect, config, statement, batch);
        match result {
            Ok(statement) => prepared.push(statement),
            Err(mut error) => {
                error.failed_statement_index = Some(index);
                return Err(error);
            }
        }
    }
    Ok(prepared)
}

fn prepare_one(
    dialect: &dyn Dialect,
    config: &DatabaseConfig,
    input: SqlStatement,
    batch: bool,
) -> Result<PreparedStatement> {
    // MySQL executes versioned comments that a generic SQL parser may ignore.
    let upper = input.sql.to_uppercase();
    if upper.contains("/*!") || upper.contains("/*M!") {
        return Err(AppError::unsupported(
            "Comentários SQL executáveis não são permitidos.",
        ));
    }
    let statements = Parser::parse_sql(dialect, &input.sql).map_err(|_| {
        AppError::unsupported(
            "SQL não reconhecido pelo parser do dialeto; nenhuma instrução foi enviada.",
        )
    })?;
    if statements.len() != 1 {
        return Err(AppError::invalid(
            "Cada item precisa conter exatamente um statement SQL.",
        ));
    }
    let mut guard = SqlGuard {
        engine: config.engine,
        mode: config.access_mode,
        batch,
        kind: StatementKind::Read,
        relations: BTreeSet::new(),
    };
    if let ControlFlow::Break(error) = statements[0].visit(&mut guard) {
        return Err(error);
    }
    if batch && guard.kind == StatementKind::Ddl {
        return Err(AppError::new(
            ErrorCode::BatchNotAtomic,
            "DDL deve ser executado individualmente; lotes aceitam leitura e DML transacional.",
        ));
    }
    let parameters = input
        .parameters
        .iter()
        .map(|parameter| parameter.resolve())
        .collect::<Result<Vec<_>>>()?;
    Ok(PreparedStatement {
        sql: input.sql,
        parameters,
        kind: guard.kind,
        relations: guard.relations,
    })
}

struct SqlGuard {
    engine: Engine,
    mode: AccessMode,
    batch: bool,
    kind: StatementKind,
    relations: BTreeSet<String>,
}

impl Visitor for SqlGuard {
    type Break = AppError;

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
        let kind = match statement {
            Statement::Query(_) => StatementKind::Read,
            Statement::Call(function)
                if self.engine == Engine::Oracle
                    && (2..=3).contains(&function.name.0.len())
                    && !function.name.to_string().contains('@') =>
            {
                StatementKind::Routine
            }
            Statement::Set(Set::SingleAssignment {
                scope: Some(ContextModifier::Local),
                hivevar: false,
                variable,
                ..
            }) if self.engine == Engine::Postgresql && is_custom_setting(&variable.to_string()) => {
                StatementKind::Read
            }
            Statement::Insert(_) | Statement::Update(_) | Statement::Delete(_) => {
                StatementKind::Write
            }
            Statement::CreateTable(_)
            | Statement::CreateIndex(_)
            | Statement::CreateView(_)
            | Statement::AlterTable(_) => StatementKind::Ddl,
            Statement::Drop {
                object_type: ObjectType::Table | ObjectType::View | ObjectType::Index,
                ..
            } => StatementKind::Ddl,
            _ => {
                return ControlFlow::Break(AppError::unsupported(
                    "Comando não oferecido: use leitura, DML, DDL suportado, CALL de pacote Oracle ou contexto LOCAL PostgreSQL.",
                ));
            }
        };
        if matches!(kind, StatementKind::Write | StatementKind::Ddl)
            && self.mode == AccessMode::ReadOnly
        {
            return ControlFlow::Break(AppError::new(
                ErrorCode::ReadOnlyViolation,
                "A conexão permite somente leitura.",
            ));
        }
        if kind == StatementKind::Ddl && self.mode != AccessMode::ReadWriteDdl {
            return ControlFlow::Break(AppError::unsupported(
                "DDL exige access_mode=read_write_ddl.",
            ));
        }
        self.kind = self.kind.max(kind);
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if !query.locks.is_empty()
            || query.settings.is_some()
            || query.format_clause.is_some()
            || !query.pipe_operators.is_empty()
        {
            return ControlFlow::Break(AppError::unsupported(
                "Opções de sessão, lock ou saída externa não são oferecidas.",
            ));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<Self::Break> {
        if select.into.is_some() {
            return ControlFlow::Break(AppError::unsupported(
                "SELECT INTO/saída externa não é permitido; use DDL explícito no terceiro modo.",
            ));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(
        &mut self,
        relation: &sqlparser::ast::ObjectName,
    ) -> ControlFlow<Self::Break> {
        let name = relation.to_string();
        if name.contains('@') {
            return ControlFlow::Break(AppError::unsupported(
                "Links remotos de banco não são oferecidos.",
            ));
        }
        self.relations.insert(name);
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expression: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Function(function) = expression {
            let name = function
                .name
                .to_string()
                .to_lowercase()
                .replace(['"', '`'], "");
            let qualified = name.contains('.') && !name.starts_with("pg_catalog.");
            let name = name.rsplit('.').next().unwrap_or(&name);
            if name == "set_config" {
                return if self.engine == Engine::Postgresql && is_local_context(function) {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(AppError::unsupported(
                        "set_config exige um parâmetro personalizado literal e is_local=true.",
                    ))
                };
            }
            if self.batch
                && (qualified
                    || ![
                        "count",
                        "sum",
                        "avg",
                        "min",
                        "max",
                        "coalesce",
                        "nullif",
                        "ifnull",
                        "nvl",
                        "lower",
                        "upper",
                        "length",
                        "char_length",
                        "substr",
                        "substring",
                        "round",
                        "abs",
                        "ceil",
                        "ceiling",
                        "floor",
                        "trim",
                        "ltrim",
                        "rtrim",
                        "concat",
                        "cast",
                        "current_setting",
                        "sys_context",
                    ]
                    .contains(&name))
            {
                return ControlFlow::Break(AppError::new(
                    ErrorCode::BatchNotAtomic,
                    "Função fora da lista de funções puras verificadas para lote; execute individualmente se permitido.",
                ));
            }
            if [
                "dblink",
                "dblink_exec",
                "load_extension",
                "writefile",
                "readfile",
                "lo_export",
                "lo_import",
                "pg_read_file",
                "pg_read_binary_file",
                "pg_write_file",
                "get_lock",
                "release_lock",
                "nextval",
                "setval",
            ]
            .contains(&name)
            {
                return ControlFlow::Break(AppError::unsupported(
                    "Função com efeitos de sessão, arquivo, sequência ou conexão externa não é oferecida.",
                ));
            }
        }
        ControlFlow::Continue(())
    }
}

fn is_custom_setting(name: &str) -> bool {
    name.contains('.')
        && name.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

fn is_local_context(function: &Function) -> bool {
    if !matches!(
        function.name.to_string().to_lowercase().as_str(),
        "set_config" | "pg_catalog.set_config"
    ) {
        return false;
    }
    let FunctionArguments::List(arguments) = &function.args else {
        return false;
    };
    let [
        FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(name))),
        _,
        FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(local))),
    ] = arguments.args.as_slice()
    else {
        return false;
    };
    matches!(&name.value, Value::SingleQuotedString(name) if is_custom_setting(name))
        && matches!(local.value, Value::Boolean(true))
}
