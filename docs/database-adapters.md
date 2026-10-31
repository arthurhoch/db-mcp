# Bancos

| Banco | Driver | Placeholder | Leitura nativa |
| --- | --- | --- | --- |
| Oracle | oracledb 26.0.0-beta.5, Rust. | `:1`, `:2`. | SET TRANSACTION READ ONLY. |
| PostgreSQL | SQLx. | `$1`, `$2`. | BEGIN READ ONLY. |
| MySQL | SQLx. | `?`, em ordem. | START TRANSACTION READ ONLY. |
| SQLite | SQLx, biblioteca embutida. | `?1`, `?2` ou `?`. | Arquivo read-only e query_only=ON. |

SQL é enviado no dialeto nativo, após validação pelo parser. Não há tradução
entre bancos. Oracle usa o parser genérico; construções que ele não reconhece
são recusadas. Metadados vêm de consultas internas com filtros parametrizados.

PostgreSQL e Oracle aceitam schema padrão configurado. MySQL mantém o database
da URL. SQLite oferece somente `main` e abre apenas arquivos existentes.

## Tipos

NUMERIC/DECIMAL e Oracle NUMBER retornam strings decimais. Inteiros grandes
também usam strings para preservar precisão no cliente. PostgreSQL recebe tipos
de parâmetros durante a preparação, inclusive em `SELECT $1` sem cast.

SQLite usa o tipo real da célula. Sua afinidade NUMERIC pode converter um valor
para REAL antes da leitura; use TEXT para guardar um decimal arbitrário exato.
Booleanos representados como tipos numéricos, como BOOLEAN MySQL, podem retornar
0/1. Timestamps com offset são normalizados para UTC; no MySQL o binding é um
datetime sem offset.

Oracle vincula decimais como NUMBER, com até 38 dígitos significativos e magnitude
menor que 1e126. Valores não nulos menores que 1e-130 são recusados. Datas aceitam
anos de 1 a 9999. Parâmetros com offset são normalizados para UTC e vinculados
como TIMESTAMP em uma sessão UTC.

Bytes usam base64. LOBs Oracle, arrays/tipos personalizados PostgreSQL e outros
tipos sem decodificador retornam erro; não há conversão genérica. O parâmetro
decimal aceita expoente absoluto de até 10000.

## Lotes

Todos os itens passam pela política antes da execução. DDL é recusado em lote
nos quatro bancos. Funções de lote usam uma lista restrita de funções de leitura,
além do contexto LOCAL PostgreSQL. Oracle aceita CALL de procedimentos em pacotes.
Veja [contexto e consultas](mcp.md#contexto-e-consultas).

| Banco | Verificação antes dos statements de escrita |
| --- | --- |
| PostgreSQL | Lock na transação; tabelas locais não particionadas, sem triggers de usuário. |
| MySQL | Metadata lock; tabelas InnoDB sem triggers e privilégio TRIGGER verificável. Grants de schema respeitam escapes e partial_revokes; permissões apenas via roles podem ser recusadas. |
| Oracle | Tabelas locais do próprio usuário, sem triggers nem tabelas externas; locks até o fim da transação. |
| SQLite | Transação nativa no arquivo selecionado, sem ATTACH ou extensões carregadas pelo agente. |

Um recurso não verificável gera BATCH_NOT_ATOMIC antes do primeiro statement do
usuário. Consultas internas e locks podem ocorrer antes dessa recusa. O lote
confirma uma vez ao final; erros tentam rollback e preservam a incerteza se a
confirmação faltar.

As verificações de tabelas não inspecionam o corpo de pacotes Oracle. Um CALL
pode confirmar a transação ou produzir efeitos autônomos; depois de enviá-lo,
falhas reportam `effect=unknown`. Use pacotes que preservem a transação quando
precisar de rollback de todos os itens.

DDL individual segue os commits implícitos do
[Oracle](https://docs.oracle.com/en/database/oracle/oracle-database/26/sqlrf/COMMIT.html)
e do [MySQL](https://dev.mysql.com/doc/refman/8.4/en/implicit-commit.html).
MyISAM aceita DML individual e pode manter efeitos parciais após erro. Por isso,
erro de DML individual MySQL reporta `effect=unknown`, mesmo com ROLLBACK aceito.

## Oracle

O [oracledb](https://github.com/oracle/rust-oracledb) implementa o protocolo em
Rust e não requer Instant Client, OCI ou ODPI-C. A versão pré-release está fixada
no Cargo.toml. A URL continua usando host, porta e service name por TCP.

O driver é síncrono e executa fora do executor async. Cancelar o future não
interrompe imediatamente o I/O bloqueante. Escrita enviada sem
confirmação pode retornar unknown e nunca é repetida automaticamente.

`RSO_DEBUG_PACKETS` é recusado para conexões Oracle: essa opção do driver imprime
pacotes em stdout e conflita com o protocolo MCP.

Uma leitura logo após DDL pode retornar
[ORA-01466](https://docs.oracle.com/en/error-help/db/ora-01466/). O código é exposto
sanitizado. O teste aguarda o timestamp de DDL antes de abrir o snapshot read-only.

As [versões testadas](testing.md) não definem suporte universal a outras versões,
variantes como MariaDB ou funções/extensões instaladas pelo operador.
