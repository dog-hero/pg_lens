# pg_lens — Plano de Desenvolvimento MVP (Fase 1)

> **pg_lens**: A blazing-fast, modern TUI for PostgreSQL observability.
> *"A microscopic view into your PostgreSQL performance."*

Este plano é dividido em fases autocontidas, projetadas para serem executadas
consecutivamente em contextos de chat novos (compatível com `/claude-mem:do`).
Cada fase inclui referências de documentação, checklist de verificação e
guardas contra anti-padrões.

---

## Decisão de Stack: Rust + Ratatui (decidido)

Avaliado em 2026-07-13 contra Deno + TS:

| Critério | Rust + Ratatui | Deno + TUI |
|---|---|---|
| Ecossistema TUI | ratatui: ativo, templates oficiais, usado por btop-likes | `deno_tui` sem release há ~3 anos; alternativas (OpenTUI, Melker) imaturas |
| Binário estático | ~3–8 MB, musl cross-compile trivial | Piso de ~60–80 MB (`denort` embute V8) |
| Overhead de memória | Nativo, sem GC/runtime | V8 runtime sempre residente |
| Async DB client | tokio-postgres 0.7.18 (pipelining nativo) | `deno-postgres` (ok, mas menos maduro) |
| Proficiência do dev | Menor (aprendizado) | Alta (TS) |

**Veredito:** as prioridades absolutas do projeto são performance, memória e
binários estáticos para instâncias Linux — Rust vence em todas. A curva de
aprendizado é mitigada pelos templates oficiais do Ratatui e pelo padrão TEA
(que é conceitualmente idêntico ao Elm/Redux que um dev TS já conhece).

Fontes: [deno_tui](https://github.com/Im-Beast/deno_tui),
[deno compile binary size](https://deno.com/blog/v1.41),
[ratatui templates](https://github.com/ratatui/templates).

### Decisão adicional (2026-07-13): UI Web futura ("Web Lens")

pg_lens terá também uma UI web hospedável para acesso remoto (pós-MVP,
Fase 6). Isso **não** muda o escopo das fases 1–5, mas impõe 3 decisões de
fundação desde o dia 1:

1. **Cargo workspace** com o crate `pg_lens_core` (db + models + poller)
   separado do crate `pg_lens_tui`. A TUI e a web são consumidores iguais
   do core.
2. **Models serializáveis**: `#[derive(serde::Serialize)]` em `DbSnapshot` e
   structs filhas desde a criação — a web vai transmiti-los como JSON.
3. **Canal multi-consumidor**: o poller publica em
   `tokio::sync::watch::Sender<Arc<DbSnapshot>>` (semântica "último valor
   vence", N consumidores) em vez de enviar direto no mpsc da TUI. A TUI
   observa o `watch` e converte para seu próprio `Action`; a web (futura)
   observa o mesmo `watch` e converte para eventos SSE.

O enum `Action` é detalhe interno da TUI — o core **nunca** o conhece.
Frontend web será em **TypeScript** (proficiência do dev), servido pelo
próprio binário Rust (axum + assets embutidos).

---

## Fase 0 — Descoberta de Documentação (CONCLUÍDA — resultados abaixo)

### APIs Permitidas (verificadas nas fontes)

**tokio-postgres 0.7.18** ([docs.rs](https://docs.rs/tokio-postgres/latest/tokio_postgres/)):
- `tokio_postgres::connect(config_str, NoTls) -> (Client, Connection)`
- A `Connection` **DEVE** ser movida para `tokio::spawn` — ela executa o I/O
  real; queries não completam sem ela rodando concorrentemente.
- `Client::query(&stmt, &params) -> Vec<Row>`, `Client::query_one(...)`
- `Row::get::<_, T>(idx_ou_nome)` para extração tipada.
- Queries são lazy: só enviadas quando o future é polled.
- TLS: `NoTls` no MVP; `postgres-native-tls` como feature futura.

**ratatui + crossterm + tokio** ([tutorial async](https://ratatui.rs/tutorials/counter-async-app/), [templates](https://github.com/ratatui/templates)):
- Template base: `cargo generate ratatui/templates` → escolher **event-driven-async**.
- crossterm com feature `event-stream` → `crossterm::event::EventStream` (async).
- Loop: `tokio::select!` sobre (a) `EventStream.next()`, (b) `watch::Receiver`
  de snapshots, (c) tick de render.
- `Terminal::draw(|frame| ...)` é síncrono e barato; NUNCA fazer I/O dentro dele.
- Versões do tutorial: ratatui 0.28 / crossterm 0.28 / tokio 1.x `features=["full"]` —
  **verificar versões atuais em docs.rs no momento da implementação** (usar a
  versão que o template gerar).

**Queries de referência do pg_activity** ([dalibo/pg_activity/pgactivity/queries](https://github.com/dalibo/pg_activity/tree/master/pgactivity/queries)):
- Convenção de versionamento: sufixo `post_140000` = PG >= 14.0. **Copiar essa convenção.**
- Arquivos-chave para copiar/adaptar:
  - `get_pg_activity_post_140000.sql` — atividade (Micro Lens)
  - `get_blocking_post_140000.sql` — locks bloqueantes
  - `get_server_info_post_110000.sql` — vitais do servidor (Macro Lens)
  - `get_wal_senders_post_090100.sql` — replicação (pós-MVP)
  - `do_pg_cancel_backend.sql` / `do_pg_terminate_backend.sql` — ações (pós-MVP)
- Colunas produzidas pela query de atividade (verificado no fonte):
  `pid, xmin, application_name, database, client, duration, wait, user, state,
  query, encoding, query_leader_pid (coalesce(leader_pid, pid)),
  is_parallel_worker, query_id`.
- Expressões notáveis: `EXTRACT(epoch FROM (NOW() - a.<duration_column>))` para
  duração; exclui `idle` e o próprio backend (`pg_backend_pid()`).

**Web (Fase 6, referências a validar na implementação):**
- [axum](https://docs.rs/axum) — servidor HTTP do mesmo runtime tokio;
  exemplo oficial de SSE em [axum/examples/sse](https://github.com/tokio-rs/axum/tree/main/examples/sse).
- [rust-embed](https://docs.rs/rust-embed) ou `include_dir` — embutir os
  assets do frontend TS no binário (mantém single-binary).
- Frontend: Vite + TypeScript + [uPlot](https://github.com/leeoniya/uPlot)
  (charts de série temporal minúsculos e rápidos — coerente com a filosofia
  do projeto).

### Anti-padrões conhecidos (NÃO FAZER)
- ❌ Query ao Postgres dentro de `Terminal::draw` ou na mesma task do input.
- ❌ Esquecer de `tokio::spawn` a `Connection` do tokio-postgres (deadlock silencioso).
- ❌ Usar o crate síncrono `postgres` misturado com tokio.
- ❌ Assumir colunas sem gate de versão: `leader_pid` só existe em PG 13+,
  `query_id` em PG 14+, `wait_event` em PG 9.6+. MVP suporta **PG 13+**; queries
  versionadas como no pg_activity.
- ❌ Inventar widgets/métodos do ratatui — usar apenas os documentados
  (`Table`, `Gauge`, `Sparkline`, `Paragraph`, `Block`, `Tabs`).
- ❌ Core (`pg_lens_core`) importar qualquer coisa de ratatui/crossterm ou
  conhecer o enum `Action` da TUI.

---

## Arquitetura (referência para todas as fases)

### Padrão: The Elm Architecture (TEA) adaptado + core multi-frontend

```
┌────────────────────────────────────────────────────────────────┐
│ pg_lens_core                                                   │
│  ┌───────────────────────┐      ┌──────────────────────────┐   │
│  │ Poller task           │      │ tokio::spawn(Connection) │   │
│  │  loop {               │      └──────────────────────────┘   │
│  │    query pg views     │                                     │
│  │    watch_tx.send(Arc<DbSnapshot>)   ← "último valor vence"  │
│  │    sleep(interval)    │                                     │
│  │  }                    │                                     │
│  └───────────┬───────────┘                                     │
└──────────────┼─────────────────────────────────────────────────┘
               │ watch::Receiver<Arc<DbSnapshot>>  (N consumidores)
       ┌───────┴────────────────────┐
       ▼                            ▼
┌──────────────────┐        ┌──────────────────────┐
│ pg_lens_tui      │        │ pg_lens_web (Fase 6) │
│ tokio::select!   │        │ axum + SSE           │
│  ├ EventStream   │        │  snapshot → JSON     │
│  ├ watch.changed()│       │  assets TS embutidos │
│  └ tick (render) │        └──────────────────────┘
│ Model→update→view│
└──────────────────┘
```

- **Model** (`app.rs`, na TUI): estado puro — snapshot atual, aba ativa,
  seleção da tabela, ordenação, flag de conexão. Sem I/O.
- **Update**: `fn update(&mut App, Action)` — única função que muta o Model.
- **View** (`ui/`): `fn draw(&App, &mut Frame)` — funções puras de renderização.
- **Action enum** (interno da TUI): `Key(KeyEvent) | Snapshot(Arc<DbSnapshot>)
  | DbError(String) | Tick | Quit`.
- **Canais**: poller → `watch<Arc<DbSnapshot>>` (multi-consumidor, sem
  backlog); dentro da TUI, um `mpsc<Action>` único agrega teclado + snapshots
  convertidos. UI nunca espera o DB: se não chegou snapshot novo, redesenha o
  antigo (com indicador de staleness). Erros do poller viajam num campo
  `status: PollerStatus` dentro do envelope do snapshot (assim a web também
  os vê).

### Estrutura de diretórios alvo (Cargo workspace)

```
pg_lens/
├── Cargo.toml                        # [workspace] members = crates/*
├── PLAN.md
└── crates/
    ├── pg_lens_core/
    │   ├── Cargo.toml                # tokio, tokio-postgres, serde
    │   ├── queries/                  # SQL como arquivos (include_str!)
    │   │   ├── activity_post_140000.sql
    │   │   ├── activity_post_130000.sql
    │   │   ├── blocking_post_140000.sql
    │   │   └── server_info_post_130000.sql
    │   └── src/
    │       ├── lib.rs
    │       ├── db.rs                 # connect + spawn Connection + versão
    │       ├── poller.rs             # loop de coleta → watch_tx.send(...)
    │       ├── queries.rs            # seleção de SQL por versão
    │       └── models.rs             # DbSnapshot, ActivityRow, ServerVitals,
    │                                 # LockRow, PollerStatus (todos Serialize)
    ├── pg_lens_tui/
    │   ├── Cargo.toml                # ratatui, crossterm, pg_lens_core
    │   └── src/
    │       ├── main.rs               # setup terminal, spawn tasks, select!
    │       ├── app.rs                # Model + update() + Action enum
    │       ├── event.rs              # EventStream → Action
    │       └── ui/
    │           ├── mod.rs            # layout raiz, tabs, statusbar
    │           ├── macro_lens.rs     # dashboard de vitais
    │           ├── micro_lens.rs     # tabela de conexões/queries
    │           └── format.rs         # duração/bytes humanos
    └── pg_lens_web/                  # criado só na Fase 6
        ├── Cargo.toml                # axum, rust-embed, pg_lens_core
        ├── frontend/                 # Vite + TS + uPlot
        └── src/main.rs (ou lib.rs)   # rotas: /, /api/snapshot, /api/stream
```

---

## Fase 1 — Scaffold do workspace + loop de render com dados mock

**Objetivo:** binário que abre a TUI, renderiza layout completo com dados
falsos, responde a `q` (sair) e `Tab` (trocar de aba). Zero código de DB.

**O que implementar:**
1. Criar o workspace: `Cargo.toml` raiz com `[workspace] members =
   ["crates/*"]`; `cargo new --lib crates/pg_lens_core`; para a TUI, gerar
   `cargo generate ratatui/templates` (template **event-driven-async**) e
   acomodar o resultado em `crates/pg_lens_tui`. Deps da TUI: `ratatui`,
   `crossterm` (feature `event-stream`), `tokio` (features `full`),
   `color-eyre`, `pg_lens_core = { path = "../pg_lens_core" }`.
2. Em `pg_lens_core/src/models.rs`: structs `DbSnapshot`, `ActivityRow` (com
   os campos da query do pg_activity listados na Fase 0), `ServerVitals`,
   `PollerStatus` — **todas com `#[derive(Clone, Debug, serde::Serialize)]`**
   — e um `DbSnapshot::mock()` gerando dados plausíveis. Nenhum código de DB
   ainda.
3. Em `pg_lens_tui/src/app.rs`: o Model (`App { active_tab, snapshot,
   table_state, should_quit, .. }`), o enum `Action` e `update()`.
4. `pg_lens_tui/src/ui/`: layout com header (nome/versão/uptime), tabs
   (Macro | Micro), corpo, statusbar com keybindings. Macro Lens: `Gauge` para
   conexões, `Sparkline` para TPS (mock), `Paragraph` para vitais. Micro Lens:
   `Table` com colunas `PID | DB | User | Client | State | Wait | Duration | Query`.
5. `main.rs`: terminal setup/restore (com panic hook restaurando o terminal —
   copiar do template), loop `tokio::select!` só com EventStream + tick.

**Referências:** template `event-driven-async` em
[ratatui/templates](https://github.com/ratatui/templates); widgets em
[docs.rs/ratatui](https://docs.rs/ratatui) (copiar exemplos de `Table` e
`Gauge` da doc, não inventar assinaturas).

**Verificação:**
- [ ] `cargo build` sem warnings; `cargo clippy --workspace -- -D warnings` limpo.
- [ ] `cargo run -p pg_lens_tui` abre a TUI, mock visível nas duas abas,
      `Tab` alterna, `q` sai.
- [ ] Ctrl+C ou panic restauram o terminal (sem terminal quebrado).
- [ ] `grep -r "tokio_postgres" crates/` → vazio (nenhum código de DB nesta fase).
- [ ] `grep -rn "ratatui\|crossterm" crates/pg_lens_core/` → vazio
      (core não conhece terminal).

**Anti-padrões:** não adicionar tokio-postgres ainda; não bloquear o loop com
`std::thread::sleep`; usar `tokio::time::interval` para o tick.

---

## Fase 2 — Pipeline de eventos e ações completo

**Objetivo:** todos os inputs viram `Action` num canal mpsc único; navegação
da tabela funciona; o contrato `watch<Arc<DbSnapshot>>` do core está de pé
(ainda com mock).

**O que implementar:**
1. `pg_lens_tui/src/event.rs`: task que consome
   `crossterm::event::EventStream` e envia `Action::Key`/`Action::Resize`
   pelo `mpsc::Sender<Action>`.
2. Keybindings no `update()`: `q`/`Esc` sair, `Tab` alternar lens, `↑/↓` ou
   `j/k` navegam a `Table` (via `TableState::select`), `s` cicla ordenação
   (duration/state/pid), `+`/`-` ajustam intervalo de refresh (só estado, por ora).
3. **Poller fake no core**: `pg_lens_core::poller::spawn_mock(interval) ->
   watch::Receiver<Arc<DbSnapshot>>` — task que a cada 2s publica
   `DbSnapshot::mock()` no `watch`. Na TUI, uma task ponte observa
   `watch.changed()` e envia `Action::Snapshot(rx.borrow().clone())` no mpsc.
   Isso valida o contrato core→frontends antes de existir DB **e** antes de
   existir web.
4. Indicador de staleness na statusbar: tempo desde o último snapshot.

**Referências:** [tutorial async do ratatui](https://ratatui.rs/tutorials/counter-async-app/)
(EventStream + select!); `tokio::sync::mpsc` e `tokio::sync::watch` em
docs.rs/tokio (copiar o exemplo de `watch` da doc).

**Verificação:**
- [ ] Mock atualiza sozinho a cada 2s (dados mudam na tela) sem travar teclado.
- [ ] Digitar rápido durante refresh não perde inputs nem trava render.
- [ ] `grep -rn "\.await" crates/pg_lens_tui/src/ui/` → vazio (view é 100% síncrona).
- [ ] `grep -rn "Action" crates/pg_lens_core/` → vazio (Action é da TUI).
- [ ] Teste unitário de `update()`: dado `Action::Key(Tab)`, `active_tab` muda.

**Anti-padrões:** não vazar tipos do ratatui para o core; não usar
`std::sync::Mutex` compartilhando estado entre tasks (o estado da UI vive só
na UI task; dados cruzam por mensagem/watch).

---

## Fase 3 — Data layer real (tokio-postgres + queries versionadas)

**Objetivo:** substituir o poller mock por coleta real no PostgreSQL,
tudo dentro de `pg_lens_core`.

**O que implementar:**
1. `pg_lens_core/src/db.rs`: `connect(dsn) -> (Client, JoinHandle)` — chama
   `tokio_postgres::connect(dsn, NoTls)` e **`tokio::spawn` a Connection**
   (regra da Fase 0). DSN vem de arg CLI/env `PG_LENS_DSN` (padrão
   `host=localhost user=postgres`), via `clap` no crate da TUI (o core recebe
   a string pronta).
2. Detectar versão: `SELECT current_setting('server_version_num')::int` →
   guardar no estado do poller.
3. `pg_lens_core/queries/*.sql`: **copiar e adaptar** de
   [pg_activity/queries](https://github.com/dalibo/pg_activity/tree/master/pgactivity/queries):
   - `activity_post_140000.sql` ← base em `get_pg_activity_post_140000.sql`
     (colunas: pid, application_name, database, client, duration em epoch,
     wait_event, usename, state, query, query_leader_pid, is_parallel_worker,
     query_id; excluir `pg_backend_pid()`).
   - `activity_post_130000.sql` ← variante sem `query_id` (PG 13).
   - `blocking_post_140000.sql` ← base em `get_blocking_post_140000.sql`
     (usa `pg_locks` + `pg_blocking_pids()`).
   - `server_info_post_130000.sql` ← vitais Macro Lens (ver mapeamento abaixo).
   Carregar com `include_str!` em `queries.rs`, selecionando por
   `server_version_num`.
4. `poller.rs` real: loop `interval.tick()` → executa as queries → monta
   `DbSnapshot` → `watch_tx.send(Arc::new(snapshot))`. Erros de query/conexão
   viram `PollerStatus::Error(msg)` no envelope (UI mostra banner e mantém
   últimos dados); reconectar com backoff simples.
5. Métricas derivadas por delta no poller (não no SQL): TPS =
   Δ(xact_commit+xact_rollback)/Δt; cache hit ratio = blks_hit/(blks_hit+blks_read).

**Mapeamento de views (Data Layer):**

| Lens | Fonte | Métricas |
|---|---|---|
| Macro | `pg_stat_database` (agregado) | TPS (delta), cache hit ratio, tup_returned/fetched, temp_files/temp_bytes, deadlocks |
| Macro | `pg_stat_activity` (count por state) | total/active/idle/idle_in_tx, waiting (wait_event not null) |
| Macro | `current_setting('max_connections')`, `pg_postmaster_start_time()`, `version()` | saturação de conexões, uptime, versão |
| Macro | `pg_database_size(datname)` | tamanho por DB (a cada N ciclos, é mais cara) |
| Micro | `pg_stat_activity` (query versionada) | lista de sessões/queries com duração |
| Micro | `pg_locks` + `pg_blocking_pids()` | quem bloqueia quem |

**Referências:** [docs.rs/tokio-postgres](https://docs.rs/tokio-postgres/latest/tokio_postgres/)
(exemplo de connect no topo da doc — copiar); views em
[postgresql.org/docs/current/monitoring-stats.html](https://www.postgresql.org/docs/current/monitoring-stats.html).

**Verificação:**
- [ ] Contra Postgres local (ex.: `docker run -e POSTGRES_PASSWORD=pg -p 5432:5432 postgres:16`):
      TUI mostra sessões reais; `pgbench -T 30` faz TPS e conexões se moverem.
- [ ] Derrubar o Postgres com a TUI aberta → banner de erro, UI segue
      responsiva, reconecta quando o DB volta.
- [ ] Testar também contra PG 13 (variante de query correta escolhida).
- [ ] `grep -rn "tokio_postgres\|query(" crates/pg_lens_tui/` → vazio
      (frontend não conhece o DB).
- [ ] `serde_json::to_string(&snapshot)` funciona num teste unitário do core
      (contrato da futura web).
- [ ] Overhead: queries do poller executam em < 50ms em DB local
      (`EXPLAIN ANALYZE` manual nas queries).

**Anti-padrões:** não usar `unwrap()` em resultados de DB (erro → status);
não preparar statements a cada tick (usar `Client::prepare` uma vez); não
rodar `pg_database_size` a cada ciclo.

---

## Fase 4 — Macro Lens e Micro Lens com dados reais + UX

**Objetivo:** dashboards completos e utilizáveis no dia a dia.

**O que implementar:**
1. **Macro Lens**: sparklines com histórico em anel (`VecDeque<f64>`, cap ~120
   amostras) para TPS e sessões ativas; gauges de conexões e cache hit;
   contadores de deadlocks/temp files. O anel de histórico vive no **core**
   (`SnapshotHistory`), não na TUI — a web vai precisar dele também.
2. **Micro Lens**: ordenação real por coluna (`s`), truncamento inteligente da
   query (largura do terminal), highlight de linhas com `wait_event` não-nulo
   e de sessões bloqueadas (cruzar com resultado do blocking query),
   `Enter` abre painel de detalhe com a query completa (`Paragraph` com wrap).
3. Formatação: duração humana (`4m32s`), bytes humanos (`1.2 GB`) — funções
   puras em `ui/format.rs` com testes unitários.
4. Header definitivo: `pg_lens v0.1.0 │ PG 16.3 @ host │ up 3d 4h │ 42/100 conns`.

**Referências:** exemplos de `Table`, `Sparkline`, `Gauge` em
[ratatui.rs/examples](https://ratatui.rs/examples/) — copiar o padrão de
`TableState` + highlight do exemplo oficial de Table.

**Verificação:**
- [ ] Com `pgbench` rodando: sparkline de TPS se move; sessão com
      `SELECT pg_sleep(60)` aparece com duração crescendo.
- [ ] Lock test: em duas sessões `psql`, `BEGIN; UPDATE t ...` sem commit +
      mesma linha na outra → segunda sessão aparece destacada como bloqueada.
- [ ] Resize do terminal não quebra layout (testar largura mínima ~80 cols).
- [ ] `cargo test --workspace` verde (update + format + serialização).

**Anti-padrões:** não recalcular histórico inteiro por frame (anel incremental);
não clonar `DbSnapshot` inteiro por frame — a view recebe `&App` (snapshot já
é `Arc`).

---

## Fase 5 — Verificação final e release engineering (fecha o MVP)

**Objetivo:** provar que tudo bate com a documentação e gerar binários.

**O que implementar/executar:**
1. **Auditoria de anti-padrões (grep):**
   - `grep -rn "\.await" crates/pg_lens_tui/src/ui/` → vazio.
   - `grep -rn "block_on\|std::thread::sleep" crates/` → vazio.
   - `grep -rn "unwrap()" crates/pg_lens_core/src/` → apenas em testes.
   - `grep -rn "ratatui\|crossterm\|Action" crates/pg_lens_core/` → vazio.
   - Confirmar `tokio::spawn` da `Connection` existe em `db.rs`.
2. **Conferência de APIs**: `cargo doc --open` / docs.rs — cada API externa
   usada existe na versão travada no `Cargo.lock` (o compilador já garante,
   mas revisar deprecations com `cargo clippy`).
3. **Matriz de teste manual**: PG 13, 14, 16 via Docker; DB vazio; DB sob
   pgbench; queda e volta do servidor.
4. **Build de release**: `cargo build --release -p pg_lens_tui` no macOS +
   cross-compile Linux: targets `x86_64-unknown-linux-musl` e
   `aarch64-unknown-linux-musl` via [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)
   ou [cross](https://github.com/cross-rs/cross) (verificar docs atuais antes
   de escolher). Meta: binário < 10 MB.
5. README com pitch, screenshot/gif (`vhs` da Charm é opcional), instruções
   de uso e keybindings.

**Verificação:**
- [ ] Todos os greps acima limpos.
- [ ] `cargo test --workspace && cargo clippy --workspace -- -D warnings` verdes.
- [ ] Binário musl roda num container `debian:stable-slim`/`alpine` contra PG 16.
- [ ] Uso de memória residente da TUI < 20 MB monitorando um DB ativo
      (verificar com `ps`/Activity Monitor).

---

## Fase 6 — Web Lens (pós-MVP): UI web hospedável

**Objetivo:** `pg_lens serve --listen 127.0.0.1:8080` sobe um servidor web
que mostra os mesmos Macro/Micro Lens no browser, em tempo real, para acesso
remoto. Read-only no primeiro corte (sem cancel/terminate via web).

**Pré-requisito:** fases 1–5 concluídas. Se as decisões de fundação foram
respeitadas, esta fase **não toca** em `pg_lens_core` além de adicionar
endpoints consumidores.

**O que implementar:**
1. Crate `pg_lens_web`: axum montado no mesmo runtime tokio, consumindo o
   mesmo `watch::Receiver<Arc<DbSnapshot>>` do poller. Rotas:
   - `GET /api/snapshot` → JSON do snapshot atual (`serde_json`).
   - `GET /api/stream` → SSE: a cada `watch.changed()`, envia o snapshot
     serializado (copiar o padrão de [axum/examples/sse](https://github.com/tokio-rs/axum/tree/main/examples/sse)).
   - `GET /` → assets estáticos embutidos via `rust-embed`.
2. Frontend em `pg_lens_web/frontend/`: Vite + **TypeScript**, sem framework
   pesado (vanilla TS ou Preact) + uPlot para TPS/histórico; tabela de
   atividade com sort no cliente. `EventSource` consome `/api/stream`.
   Build do Vite (`dist/`) é embutido no binário no `cargo build` (build.rs
   ou passo de CI).
3. CLI unificado: subcomandos `pg_lens tui` (padrão) e `pg_lens serve
   [--listen addr]` — um binário só, com o crate web atrás de uma
   feature flag `web` para não inflar quem só quer a TUI.
4. **Segurança (obrigatório antes de qualquer deploy remoto):**
   - Auth por bearer token (`PG_LENS_AUTH_TOKEN`); sem token definido, o
     servidor **recusa** bind fora de localhost.
   - Default bind `127.0.0.1` — expor é decisão explícita do operador.
   - Documentar no README: usar role com `pg_monitor` (read-only) no DSN,
     e TLS via reverse proxy (Caddy/nginx) — o binário não termina TLS no
     primeiro corte.
   - O DSN/senha jamais aparece em endpoint, log ou payload JSON.

**Referências:** [docs.rs/axum](https://docs.rs/axum) (router + SSE),
[docs.rs/rust-embed](https://docs.rs/rust-embed), [uPlot](https://github.com/leeoniya/uPlot).
Verificar versões atuais no momento da implementação; copiar os exemplos
oficiais do axum, não inventar extractors.

**Verificação:**
- [ ] `pg_lens serve` + browser: dashboard atualiza em tempo real via SSE
      enquanto `pgbench` roda; TUI aberta em paralelo mostra os mesmos dados
      (dois consumidores do mesmo watch).
- [ ] `curl /api/snapshot` sem token em bind não-local → 401.
- [ ] `grep -rn "ratatui" crates/pg_lens_web/` → vazio.
- [ ] Diff de `pg_lens_core` nesta fase ≈ zero (prova de que a fundação
      estava certa).
- [ ] Binário com feature `web` continua < 15 MB.

**Anti-padrões:** não criar um segundo poller para a web (mesmo `watch`);
não usar WebSocket onde SSE basta (fluxo é unidirecional); não servir o
frontend de disco em produção (embutir); nunca logar o DSN.

---

## Planos complementares
- **Conexão avançada** (env vars libpq, services file com `password_cmd`):
  plano detalhado em [PLAN_CONNECTIONS.md](PLAN_CONNECTIONS.md). ✅ C1+C2
- **Distribuição** (Homebrew tap, Docker/GHCR, deb/rpm, crates.io +
  binstall): plano detalhado em [PLAN_DISTRIBUTION.md](PLAN_DISTRIBUTION.md).
- **Schema Lens** (pg_stat_user_tables + bloat estimado via
  ioguix/pgsql-bloat-estimation): plano detalhado em
  [PLAN_SCHEMA_LENS.md](PLAN_SCHEMA_LENS.md).

## Fora de escopo (backlog Fase 7+)
- Ações administrativas (`pg_cancel_backend`/`pg_terminate_backend` — SQL já
  mapeado no pg_activity; na web exigiria auth mais forte + auditoria),
  `pg_stat_statements`, replicação/WAL senders, métricas de SO da máquina do
  servidor, TLS nativo no servidor web, export Prometheus, temas,
  multi-instância (monitorar N clusters numa UI só).
