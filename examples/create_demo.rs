use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use clap::Parser;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

#[derive(Parser)]
#[command(about = "Cria um banco SQLite de exemplo e três conexões MCP")]
struct Arguments {
    #[arg(default_value = ".runtime/demo")]
    directory: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    create_demo(&arguments.directory).await?;
    println!(
        "Configuração: {}",
        arguments.directory.join("config/global.toml").display()
    );
    Ok(())
}

async fn create_demo(directory: &Path) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = directory
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    // create_dir fails if the destination exists, before any file is changed.
    fs::create_dir(directory)?;
    let connections = directory.join("config/databases");
    fs::create_dir_all(&connections)?;

    let options = SqliteConnectOptions::new()
        .filename(directory.join("demo.db"))
        .create_if_missing(true);
    let mut database = SqliteConnection::connect_with(&options).await?;
    sqlx::query(
        "CREATE TABLE pedidos(id INTEGER PRIMARY KEY, cliente TEXT, status TEXT, valor TEXT)",
    )
    .execute(&mut database)
    .await?;
    for (id, client, status, amount) in [
        (101, "Cliente A", "PENDENTE", "149.90"),
        (102, "Cliente B", "PROCESSADO", "85.00"),
        (103, "Cliente C", "PENDENTE", "230.50"),
    ] {
        sqlx::query("INSERT INTO pedidos VALUES(?, ?, ?, ?)")
            .bind(id)
            .bind(client)
            .bind(status)
            .bind(amount)
            .execute(&mut database)
            .await?;
    }
    database.close().await?;

    fs::write(
        directory.join("config/global.toml"),
        "\
config_version = 1
databases_dir = \"databases\"

[server]
transport = \"stdio\"
",
    )?;
    for (suffix, mode) in [
        ("ro", "read_only"),
        ("rw", "read_write"),
        ("ddl", "read_write_ddl"),
    ] {
        fs::write(
            connections.join(format!("demo-{suffix}.toml")),
            format!(
                "\
config_version = 1
id = \"demo-{suffix}\"
name = \"Demo SQLite ({mode})\"
description = \"Pedidos de exemplo\"
engine = \"sqlite\"
access_mode = \"{mode}\"

[connection]
url = \"sqlite:../../demo.db\"
"
            ),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_mcp::config::{AccessMode, Catalog};

    #[tokio::test]
    async fn creates_data_and_connections_with_distinct_access_modes() -> Result<(), Box<dyn Error>>
    {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("demo");
        create_demo(&directory).await?;

        let catalog = Catalog::load(&directory.join("config/global.toml"))?;
        assert_eq!(catalog.databases.len(), 3);
        for (id, mode) in [
            ("demo-ro", AccessMode::ReadOnly),
            ("demo-rw", AccessMode::ReadWrite),
            ("demo-ddl", AccessMode::ReadWriteDdl),
        ] {
            assert_eq!(catalog.database(id)?.access_mode, mode);
        }
        let options = SqliteConnectOptions::new()
            .filename(directory.join("demo.db"))
            .read_only(true);
        let mut database = SqliteConnection::connect_with(&options).await?;
        let pending: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM pedidos WHERE status='PENDENTE' ORDER BY id")
                .fetch_all(&mut database)
                .await?;
        assert_eq!(pending, [101, 103]);
        database.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn refuses_existing_destination_without_changing_its_files() -> Result<(), Box<dyn Error>>
    {
        let temporary = tempfile::tempdir()?;
        let marker = temporary.path().join("keep.txt");
        fs::write(&marker, "existing content")?;

        assert!(create_demo(temporary.path()).await.is_err());
        assert_eq!(fs::read_to_string(marker)?, "existing content");
        assert!(!temporary.path().join("demo.db").exists());
        Ok(())
    }
}
