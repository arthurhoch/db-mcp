# Configuração

O arquivo global aponta para um diretório com um TOML por conexão:

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

Os arquivos `.toml` são carregados sem recursão, em ordem de nome. Um arquivo
inválido ou ID duplicado impede a inicialização. O catálogo é carregado sem
abrir conexões; mudanças exigem reiniciar o processo. Symlinks de arquivos de
conexão são recusados.

```sh
db-mcp --config /caminho/config/global.toml --check-config
```

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
| `config_version` | Obrigatório; valor 1. |
| `id` | Obrigatório; único, no formato `[a-z][a-z0-9-]{0,63}`. |
| `name`, `description` | Obrigatórios, não vazios; publicados no catálogo MCP. |
| `aliases` | Lista opcional. Aliases repetidos deixam a escolha para o agente. |
| `engine` | `oracle`, `postgresql`, `mysql` ou `sqlite`. |
| `access_mode` | `read_only`, `read_write` ou `read_write_ddl`. |
| `default_schema` | Opcional; não restringe os grants do banco. |
| `connection` | URL e credenciais. |
| `execution` | Overrides de timeout, linhas e bytes; só podem reduzir os limites globais. |

| Engine | URL | Exemplo completo |
| --- | --- | --- |
| Oracle | `oracle://host:1521/service_name` | [Oracle](examples/config/databases/erp-oracle-homologacao.toml) |
| PostgreSQL | `postgresql://host:5432/database` | [PostgreSQL](examples/config/databases/vendas-postgres-homologacao.toml) |
| MySQL | `mysql://host:3306/database` | [MySQL](examples/config/databases/legado-mysql-homologacao.toml) |
| SQLite | `sqlite:./arquivo.db` | [SQLite](examples/config/databases/local-sqlite.toml) |

Cada campo `url`, `username` e `password` aceita valor direto ou a alternativa
`url_env`, `username_env` e `password_env`. Usar as duas fontes para o mesmo campo
é erro. Variável ausente ou vazia também é erro. Não versionar credenciais reais.

URLs de rede exigem host e database/serviço. Credenciais embutidas, query string,
fragmento e porta zero são recusados. Opções de TLS e certificados personalizados
não têm campos nesta versão; valem os padrões do driver.

SQLite usa somente URL, sem usuário ou senha. O arquivo precisa existir; caminhos
relativos partem do TOML da conexão, e escapes como `%20` são decodificados.
`default_schema` aceita apenas `main`. No MySQL, ele deve coincidir com o database
da URL. Oracle usa service name; SID, TNS e wallet não são suportados.

## Limites globais

Exemplo com todos os campos: [global.toml](examples/config/global.toml).

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
