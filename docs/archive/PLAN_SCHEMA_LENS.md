# pg_lens — Plano: Schema Lens (tabelas, pg_stat_user_tables e bloat estimado)

> Terceira lente do pg_lens: visão de schema/tabelas do banco conectado —
> estatísticas do `pg_stat_user_tables` (scans, tuplas vivas/mortas,
> vacuum/analyze) e **bloat estimado** de tabelas e índices btree, com as
> queries do [ioguix/pgsql-bloat-estimation](https://github.com/ioguix/pgsql-bloat-estimation)
> como base.

Segue as convenções do [PLAN.md](PLAN.md): fases autocontidas executáveis
via `/claude-mem:do`, checklist e anti-padrões por fase, invariantes do
CLAUDE.md (core sem UI, sem unwrap, gate clippy -D warnings).

---

## Fase S0 — Descoberta e decisões (CONCLUÍDA — resultados abaixo)

### Fatos verificados (fontes)

**ioguix/pgsql-bloat-estimation** ([repo](https://github.com/ioguix/pgsql-bloat-estimation)):
- **Licença BSD-2-Clause** — compatível com nosso MIT; ao adaptar os SQL,
  manter o aviso de copyright no cabeçalho dos arquivos derivados.
- Estrutura: `table/table_bloat.sql` (bloat de heap) e
  `btree/btree_bloat.sql` (+ variante superuser, mais rápida — NÃO usar,
  pg_lens não assume superuser).
- Saída: real size, extra size/%, fillfactor, bloat size/%, e flag
  **`is_na`** marcando estimativas não confiáveis (ex.: colunas `name`,
  stats ausentes) — a UI DEVE exibir essa flag, não esconder.
- Ressalvas documentadas: são **estimativas estatísticas** — dependem de
  `ANALYZE` recente; campos TOASTed causam subestimação; alignment
  padding (~10%) aparece no bloat sem ser isolável. A UI deve se rotular
  "estimated bloat", nunca "bloat".

**pg_stat_user_tables** ([postgresql.org/docs/current/monitoring-stats.html](https://www.postgresql.org/docs/current/monitoring-stats.html)):
- Colunas estáveis PG13+: `seq_scan`, `seq_tup_read`, `idx_scan`,
  `idx_tup_fetch`, `n_tup_ins/upd/del/hot_upd`, `n_live_tup`,
  `n_dead_tup`, `n_mod_since_analyze`, `n_ins_since_vacuum`,
  `last_[auto]vacuum`, `last_[auto]analyze`, `[auto]vacuum_count`,
  `[auto]analyze_count`.
- Gates de versão a CONFERIR na implementação (a doc "current" é PG18):
  `last_seq_scan`/`last_idx_scan` são **PG16+**; `n_tup_newpage_upd`
  também é recente. MVP da lente usa só o conjunto PG13+; colunas 16+
  entram como variante `post_160000` (convenção já existente).
- **A view é por banco conectado** — a Schema Lens mostra o database do
  DSN, não o cluster inteiro. Deixar explícito no header da lente.

**Verificação de precisão**: a extensão `pgstattuple` (contrib) mede
bloat EXATO — usar nos testes de verificação para validar que nossa
estimativa fica na ordem de grandeza certa (não em produção; é cara).

### Decisões de design

1. **Cadência separada (o ponto crítico)**: as queries de bloat e
   `pg_total_relation_size` são caras — NUNCA no tick de 2s. O snapshot
   ganha um campo `schema: Option<SchemaSnapshot>` com `collected_at`
   próprio, coletado a cada N segundos (default 60s, configurável) E
   sob demanda (tecla `R` na lente). O poller intercala: tick normal
   (atividade) + tick lento (schema). Erros da coleta lenta não derrubam
   a rápida (status separado dentro de SchemaSnapshot).
2. **Modelos** (core, Serialize): `SchemaSnapshot { collected_at,
   tables: Vec<TableStatRow>, table_bloat: Vec<BloatRow>,
   index_bloat: Vec<BloatRow>, status }`. `TableStatRow` espelha o
   conjunto PG13+ acima + `total_bytes`/`table_bytes`/`index_bytes`
   (via `pg_total_relation_size` etc. na mesma query). `BloatRow {
   schema, name (tabela ou índice), real_bytes, bloat_bytes, bloat_pct,
   fillfactor, is_na }`.
3. **TUI**: `Tab` passa a ciclar 3 lentes (Macro → Micro → Schema).
   Schema Lens: tabela ordenável (`s`) por total size / dead tuples /
   bloat% / seq scans; colunas `Table | Size | Live | Dead | Bloat% |
   Bloat | Last AV | Seq/Idx`; linhas com bloat% alto (>30% e >1MB) em
   amarelo, (>50% e >10MB) em vermelho; `is_na` renderiza `~?` no lugar
   do número; `Enter` abre detalhe (todas as colunas de vacuum/analyze
   + bloat de índices daquela tabela); `R` força re-coleta; staleness
   da coleta no rodapé da lente.
4. **Web Lens** ganha a mesma aba (mesmo SchemaSnapshot já viaja no
   watch/SSE de graça — só falta a UI TS).
5. **Version gating**: mesmo esquema de sufixos
   (`table_stats_post_130000.sql`, `post_160000` se/quando usar colunas
   16+; bloat do ioguix funciona em 13+ sem variantes — conferir).

### Anti-padrões (NÃO FAZER)
- ❌ Rodar bloat/size no tick rápido (é o anti-padrão nº 1 desta feature).
- ❌ Apresentar estimativa como medida ("bloat" seco) — sempre
  "estimated", sempre respeitando `is_na`.
- ❌ Reescrever as queries do ioguix "de cabeça" — copiar dos arquivos
  do repo, adaptar o mínimo (remover parâmetros de filtro, ajustar
  aliases), manter atribuição BSD no cabeçalho.
- ❌ Usar a variante superuser do btree bloat.
- ❌ `pgstattuple` em runtime (só em teste de verificação).
- ❌ Quebrar o contrato existente: DbSnapshot continua Serialize,
  core sem UI, watch único.

---

## Fase S1 — Data layer: table stats + coleta em cadência lenta

**Objetivo:** `SchemaSnapshot` real (sem bloat ainda) viajando no
DbSnapshot, coletado a cada 60s sem afetar o tick de 2s.

**O que implementar:**
1. `queries/table_stats_post_130000.sql`: `pg_stat_user_tables` JOIN
   sizes (`pg_total_relation_size(relid)`, `pg_table_size`,
   `pg_indexes_size`) — colunas do conjunto PG13+ da Fase S0; LIMIT
   configurável (default 200 por total_bytes desc — proteger contra
   bancos com dezenas de milhares de tabelas).
2. Models: `SchemaSnapshot`, `TableStatRow`, `SchemaStatus` (Serialize;
   mock() para a lente funcionar em `--mock`).
3. Poller: agendamento de segunda cadência (`schema_interval`, default
   60s, flag `--schema-interval`) + comando de coleta imediata (o canal
   de comandos pode ser um `watch<Instant>`/notify simples — decidir na
   implementação, sem Mutex). Falha da coleta lenta → `SchemaStatus::
   Error` no envelope, atividade segue intacta.
4. Statement preparado uma vez, como os demais.

**Verificação:**
- [ ] Gate verde; testes: mock com schema; serialização; agendador
      (tokio::test com time paused se viável).
- [ ] Live PG16 + pgbench: /api/snapshot (ou log de teste) mostra
      tables com n_live_tup/n_dead_tup reais das pgbench_*; coleta
      ocorre a cada 60s (timestamps) e o tick de 2s não muda de
      latência (medir before/after).
- [ ] PG13: colunas do conjunto 13+ funcionam.

---

## Fase S2 — Bloat estimado (ioguix)

**Objetivo:** `table_bloat`/`index_bloat` no SchemaSnapshot.

**O que implementar:**
1. Buscar os SQL ATUAIS do repo (raw): `table/table_bloat.sql` e
   `btree/btree_bloat.sql` (variante não-superuser). Adaptar:
   filtrar ao database corrente, remover placeholders, alias para os
   campos do `BloatRow`, manter cabeçalho de copyright BSD-2-Clause +
   URL de origem. Salvar como `queries/bloat_tables.sql` e
   `queries/bloat_indexes.sql` (verificar se precisam de gate de versão
   para 13–16; o repo suporta versões antigas, provavelmente um arquivo
   serve).
2. Executar na cadência lenta junto com table_stats (mesmo ciclo).
3. `is_na = true` → `bloat_pct = None` nos models (Option, não 0.0).
4. README: nota metodológica curta (estimativa, dependência de ANALYZE,
   crédito ao ioguix).

**Verificação:**
- [ ] Live PG16: criar bloat de verdade (tabela com fillfactor 100,
      UPDATE em massa repetido com autovacuum desligado na tabela,
      `ANALYZE` ao final) → bloat% reportado alto; rodar `VACUUM FULL`
      + `ANALYZE` → bloat% cai drasticamente.
- [ ] Validação cruzada: `CREATE EXTENSION pgstattuple` no container e
      comparar ordem de grandeza (dead_tuple_percent + free_percent vs
      nossa estimativa) para 2–3 tabelas — documentar o delta no
      relatório da fase (estimativa dentro de ±15pp é aceitável).
- [ ] Tabela com coluna `name` (ex.: criar uma) → `is_na` true e UI
      não mostra número inventado.
- [ ] Queries lentas rodam < 500ms no pgbench scale 10 (EXPLAIN ANALYZE
      manual; se passar disso, documentar e considerar LIMIT/filtro).

---

## Fase S3 — Schema Lens na TUI

**Objetivo:** terceira aba completa e utilizável.

**O que implementar:**
1. `Tab` cicla 3 lentes; `ui/schema_lens.rs` com a tabela da Fase S0
   (decisão 3): sort por `s`, cores por severidade de bloat, `~?` para
   is_na, `Enter` detalhe (stats completas + índices da tabela com seus
   bloats), `R` re-coleta, staleness da coleta lenta no rodapé.
2. Header da lente: `db: <database> · N tables · coleta há Xs`.
3. Formatação reutiliza `ui/format.rs` (bytes/durações).

**Verificação:**
- [ ] Harness PTY: mock mostra a aba; sort cicla; detalhe abre/fecha;
      `R` atualiza timestamp; 80x24 não quebra.
- [ ] Live com o cenário de bloat da S2: tabela inchada aparece no topo
      ordenando por bloat, com cor; após VACUUM FULL some do topo.
- [ ] Testes de update() para as teclas novas; gate verde.

---

## Fase S4 — Schema Lens no Web Lens

**Objetivo:** paridade no browser (o dado já chega via SSE desde a S1).

**O que implementar:**
1. Aba "Schema" no frontend TS: tabela ordenável (mesmo padrão da
   activity), badges de bloat com as mesmas cores, tooltip com a nota
   "estimated (needs fresh ANALYZE)", indicador de staleness da coleta.
2. Botão de re-coleta? Exige endpoint de escrita — **NÃO** no primeiro
   corte (web é read-only); apenas exibir. Anotar como backlog.

**Verificação:**
- [ ] Browser real: aba renderiza dados reais, sort funciona, is_na e
      staleness visíveis; SSE atualiza quando a coleta lenta roda.
- [ ] `git diff --stat crates/pg_lens_core/` ≈ zero nesta fase.
- [ ] Binário release continua < 15 MB.

---

## Sequência recomendada

S1 → S2 → S3 → S4. Depende apenas de fases já concluídas (MVP + Web
Lens). Sem ações externas do dono. Sugestão de release ao final: v0.3.0
(ou v0.2.0 se sair antes da Fase D1 de distribuição — coordenar com
[PLAN_DISTRIBUTION.md](PLAN_DISTRIBUTION.md), que renomeia o binário).
