# Arquitetura

Um pacote Cargo com Tokio, rmcp, SQLx e oracledb. Versões exatas estão em
Cargo.lock; o toolchain fica em rust-toolchain.toml.

```text
main → mcp → execution → policy
                │
                ├── config / catálogo
                └── database → SQLx ou oracledb
```

| Módulo | Responsabilidade |
| --- | --- |
| `config` | Carregamento TOML, validação, secrets e catálogo privado. |
| `model` | Entradas MCP, valores de parâmetros e orçamento de saída. |
| `policy` | Parser por dialeto, classificação e autorização do SQL. |
| `execution` | Catálogo público, metadados, admissão, timeout e respostas. |
| `database/sqlx` | PostgreSQL, MySQL e SQLite: conexão, bindings e transação. |
| `database/oracle` | Driver Oracle síncrono, bindings, timeout e confirmação. |
| `database/metadata` | Consultas internas de metadados. |
| `error` | Códigos sanitizados e estado dos efeitos. |
| `mcp`, `main` | Ferramentas, schemas, transporte e inicialização. |

## Uma chamada

1. Resolver o ID no catálogo e validar entrada, modo e todos os statements.
2. Adquirir a vaga por banco, seguida das vagas globais de operação/conexão.
3. Abrir uma conexão, configurar schema e proteção nativa de leitura.
4. Verificar recursos transacionais quando houver lote.
5. Executar os itens em ordem com bindings e coletar a saída limitada; o contexto
   configurado por um item fica disponível aos seguintes na mesma conexão.
6. Confirmar escrita ou reverter; informar efeitos conhecidos e fechar a conexão.

Não há banco ativo global, pool ou transação entre chamadas. Uma falha não muda
o destino para outra conexão. Cada chamada tem seus próprios parâmetros,
transação e resultado.

Oracle roda em `spawn_blocking`. Os permits ficam dentro da tarefa nativa, mesmo
quando o chamador deixa de esperar. Timeout não prova que o I/O terminou nem que uma
escrita foi revertida.

## Acesso e dados

O acesso efetivo depende do modo configurado e dos grants da credencial. O parser
recusa comandos fora do contrato, inclusive troca de destino, PL/SQL livre,
COPY e ATTACH. Contexto LOCAL PostgreSQL e CALL de pacote Oracle fazem parte do
lote, sem uma etapa separada de configuração. Transações read-only e flags do
arquivo SQLite complementam a política. O corpo de rotinas instaladas, inclusive
commits internos e efeitos externos, continua sob administração do banco; grants
restritos são necessários para limitar esses efeitos.

STDIO usa a fronteira do processo local: quem o inicia acessa seu catálogo.
Não há autenticação MCP, HTTP ou serviço compartilhado. O cliente MCP pode
aplicar sua própria política de aprovação de escrita.

Secrets não são serializados nem revelados por Debug. Logs usam stderr e
registram correlação, ID, duração e estado, sem URL, SQL, parâmetros ou linhas.
Dados e descrições retornados pelo banco não alteram a política de execução.

Os limites de saída são aplicados após o driver receber a linha. Eles não são
um teto de memória para um pacote/célula individual nem um limite de registros
afetados por DML.
