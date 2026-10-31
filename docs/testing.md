# Testes

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

A suíte local usa arquivos SQLite temporários e um processo MCP real. Cobre
configuração, modos, bindings, limites, isolamento e transações. O exemplo
`create_demo` testa os dados criados e a recusa de sobrescrita.

## Bancos de rede

Os testes ignorados exigem os bancos descartáveis de
[compose.yaml](../tests/compose.yaml), nas portas locais 55432 (PostgreSQL),
53306 (MySQL) e 51521 (Oracle). Não usam configurações do operador.

```sh
docker compose -p db-mcp-validation -f tests/compose.yaml up -d --wait
DB_MCP_INTEGRATION=1 cargo test --all-targets --locked -- --include-ignored --test-threads=1
docker compose -p db-mcp-validation -f tests/compose.yaml down -v
```

Em PowerShell, defina `$env:DB_MCP_INTEGRATION = "1"` antes de executar Cargo.

O contrato comum verifica versão, parâmetros, precisão, DML, CREATE/ALTER/DROP,
modos, lote confirmado/revertido e truncamento. Testes MySQL verificam MyISAM,
triggers e efeitos parciais. Testes PostgreSQL cortam a resposta após um commit
real e cancelam uma consulta observada em pg_stat_activity. Oracle verifica NULL,
RAW, NUMBER, inteiros unsigned, timestamps com timezone, timeout e rejeição de
bindings inválidos antes do lote. O processo MCP é exercitado nos quatro bancos.

Os testes de contexto usam RLS real no PostgreSQL, com usuário sem bypass, e
VPD com pacote Oracle. Verificam contexto compartilhado entre statements,
isolamento entre chamadas, escrita filtrada, rollback, falha na preparação e
efeito desconhecido quando um pacote confirma a transação internamente.

## Ambientes verificados em 2026-10-31

| Cliente / banco | Resultado registrado |
| --- | --- |
| macOS 26.6.2 ARM64, Rust 1.99.1 | 49 testes aprovados: 31 locais e 18 de rede. Fmt, Clippy e build release aprovados. |
| PostgreSQL 17.11 | Servidor Linux ARM64 em container. |
| MySQL 8.4.13 | Servidor Linux ARM64 em container. |
| Oracle 23.0.0.0.0, VERSION_FULL 23.26.3.0.0 | Servidor Linux ARM64; cliente macOS com oracledb 26.0.0-beta.5. |
| SQLite 3.51.3 embutido | Arquivos reais temporários. |

Os testes do processo MCP cobrem catálogo, versão real, SELECT parametrizado,
bloqueio de escrita e a recusa de RSO_DEBUG_PACKETS. Rodam sem ORACLE_HOME,
LD_LIBRARY_PATH ou DYLD_LIBRARY_PATH. O cliente é JSON-RPC dedicado, com negociação
MCP 2025-11-25.

`DB_MCP_TEST_BINARY` permite executar esses testes contra um binário externo.
A CI usa essa variável para validar o executável release antes de empacotá-lo.

O [workflow](../.github/workflows/ci.yaml) cobre macOS, Windows e Linux em x64 e
ARM64, com build, fmt, Clippy e testes locais. Um job Linux ARM64 executa a suíte
completa nos bancos descartáveis. A CI limita jobs e debuginfo para reduzir
memória de linkedição. Execuções e pacotes ficam disponíveis no GitHub Actions;
veja [releases](releases.md).
