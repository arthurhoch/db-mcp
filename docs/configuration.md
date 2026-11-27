# Configuração

`global.toml` aponta para a pasta que contém um arquivo TOML por conexão.

```text
config/
  global.toml
  databases/
    erp.toml
    vendas.toml
```

```toml
config_version = 1
databases_dir = "./databases"

[server]
transport = "stdio"
```

O servidor lê os arquivos `.toml` diretamente em `databases_dir`, em ordem de
nome. Um TOML inválido, um ID duplicado ou um link simbólico no lugar de um
arquivo de conexão impede a inicialização.

Para adicionar um banco, copie apenas o [exemplo do seu banco](#conexões) para
esse diretório. Ajuste o ID, o modo de acesso e os dados de conexão. Nos bancos
de rede, defina a variável indicada por `password_env` tanto no ambiente de
validação quanto no processo iniciado pelo cliente MCP.

```sh
db-mcp --config /caminho/config/global.toml --check-config
db-mcp --config /caminho/config/global.toml --test-connection erp-hml
```

`--check-config` valida todo o catálogo sem abrir conexões. `--test-connection`
consulta a versão do banco com o ID informado (no exemplo, `erp-hml`). Reinicie
o servidor MCP no cliente para carregar as alterações.

## Conexões

```toml
config_version = 1
id = "erp-hml"
name = "ERP homologação"
aliases = ["erp hml"]
description = "Pedidos e faturamento"
engine = "oracle"
access_mode = "read_only"

[connection]
url = "oracle://oracle.example.invalid:1521/ERP_HML"
username = "app_reader"
password_env = "ERP_HML_PASSWORD"
```

| Campo | Regra |
| --- | --- |
| `config_version` | Use `1`. |
| `id` | Identificador único no formato `[a-z][a-z0-9-]{0,63}`. |
| `name`, `description` | Textos obrigatórios exibidos no catálogo MCP. |
| `aliases` | Lista opcional. Se um alias identificar mais de um banco, o agente deve perguntar qual usar. |
| `engine` | `oracle`, `postgresql`, `mysql`, `sqlite` ou `sql_server`. |
| `access_mode` | `read_only`, `read_write` ou `read_write_ddl`. SQL Server aceita apenas `read_only`. |
| `default_schema` | Schema padrão opcional. As permissões vêm do banco. |
| `connection` | URL e credenciais. |
| `execution` | Permite reduzir o tempo de execução e os limites de linhas e bytes desta conexão. |

| Engine | URL | Exemplo completo |
| --- | --- | --- |
| Oracle | `oracle://host:1521/service_name` | [Oracle](examples/config/databases/erp-oracle-homologacao.toml) |
| PostgreSQL | `postgresql://host:5432/database` | [PostgreSQL](examples/config/databases/vendas-postgres-homologacao.toml) |
| MySQL | `mysql://host:3306/database` | [MySQL](examples/config/databases/legado-mysql-homologacao.toml) |
| SQLite | `sqlite:./arquivo.db` | [SQLite](examples/config/databases/local-sqlite.toml) |
| SQL Server | `sqlserver://host:1433/database` | [SQL Server](examples/config/databases/relatorios-sql-server.toml) |

Para cada campo (`url`, `username` e `password`), escolha uma fonte. Use o valor
direto ou indique o nome da variável em `url_env`, `username_env` ou `password_env`,
respectivamente. O servidor recusa variáveis ausentes ou vazias. Mantenha
credenciais reais fora do repositório.

URLs de rede exigem host e database/serviço. Credenciais embutidas, query string,
fragmento e porta zero são recusados. No SQL Server, use login SQL e o host e a
porta TCP da instância. O certificado TLS precisa corresponder ao host da URL.
Para uma autoridade interna, defina `connection.tls_ca_file` com o caminho ao
certificado PEM da CA, relativo ao TOML da conexão ou absoluto. Os demais
drivers usam seus padrões de TLS.

SQLite usa somente URL, sem usuário ou senha. O arquivo precisa existir; caminhos
relativos partem do TOML da conexão, e escapes como `%20` são decodificados.
`default_schema` aceita apenas `main`. No MySQL, ele deve coincidir com o database
da URL. Oracle usa service name; SID, TNS e wallet não são suportados. No SQL
Server, os metadados usam `dbo` por padrão; as consultas sem schema seguem o
schema padrão do login.

## Limites globais

O [global.toml de exemplo](examples/config/global.toml) usa os limites padrão.

| Seção/campo | Padrão | Aplicação |
| --- | ---: | --- |
| `execution.query_timeout_seconds` | 30 | Conexão e execução, após admissão. |
| `execution.max_rows` | 500 | Linhas retornadas no total da chamada. |
| `execution.max_result_bytes` | 1048576 | Resposta JSON; mínimo 1024. |
| `execution.max_sql_bytes` | 262144 | SQL UTF-8, somado no lote. |
| `execution.max_parameter_bytes` | 262144 | Parâmetros serializados, somados no lote. |
| `execution.max_batch_statements` | 50 | Itens por lote. |
| `execution.max_concurrent_operations` | 4 | Operações simultâneas. |
| `connections.max_open_total` | 16 | Conexões abertas. |
| `connections.max_open_per_database` | 4 | Conexões por ID. |
| `connections.acquire_timeout_seconds` | 10 | Espera por vagas nos semáforos. |
| `connections.idle_timeout_seconds` | 300 | Aceito pelo formato; sem efeito, pois não há pool. |
| `logging.level` | `info` | `error`, `warn`, `info` ou `debug`; stderr. |

Limites numéricos devem ser positivos; zero não significa ilimitado. Arquivos de
configuração têm limite de 1 MiB e campos desconhecidos são recusados. Limites de
linhas não limitam quantos registros um UPDATE ou DELETE afeta.
