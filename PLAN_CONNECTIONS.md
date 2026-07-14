# pg_lens — Plano: Conexão avançada (env vars, services file, password_cmd)

> Melhoria da forma de conexão ao banco: variáveis de ambiente padrão do
> libpq, arquivo de serviços com N hosts cadastrados (inspirado no
> `pg_service.conf`), e resolução de senha por comando externo
> (ex.: `password_cmd = "vault kv get -field=password secret/pg/prod"`)
> para que o arquivo nunca precise conter senhas.

Segue as convenções do [PLAN.md](PLAN.md): fases autocontidas executáveis via
`/claude-mem:do`, checklist de verificação e anti-padrões por fase, e os
invariantes do CLAUDE.md (gate = `cargo clippy --workspace --all-targets --
-D warnings`; core livre de UI; sem `unwrap()` no core).

---

## Fase C0 — Descoberta de Documentação (CONCLUÍDA — resultados abaixo)

### Fatos verificados (fontes)

**tokio-postgres `Config`** ([docs.rs/tokio-postgres/.../Config](https://docs.rs/tokio-postgres/latest/tokio_postgres/config/struct.Config.html)):
- Builder completo: `host()`, `port()`, `user()`, `password()`, `dbname()`,
  `application_name()`, `connect_timeout()`, `ssl_mode()`, `options()`.
- `FromStr` aceita DSN `key=value` E URLs `postgres://`.
- **NÃO lê env vars (PGHOST etc.) nem service files** — nada disso é
  mencionado na doc; a resolução é 100% nossa. É por isso que este plano
  existe.

**Env vars do libpq** ([postgresql.org/docs/current/libpq-envars.html](https://www.postgresql.org/docs/current/libpq-envars.html)):
- Nomes oficiais: `PGHOST`, `PGPORT`, `PGDATABASE`, `PGUSER`, `PGPASSWORD`,
  `PGAPPNAME`, `PGCONNECT_TIMEOUT`, `PGSSLMODE`, `PGSERVICE`,
  `PGSERVICEFILE`, `PGPASSFILE` (e outras de SSL/GSS fora do escopo MVP).
- **Atenção: é `PGUSER`, não `PGUSERNAME`** — seguimos os nomes oficiais.
- Precedência documentada: env vars são apenas *defaults* — parâmetro
  explícito sempre vence.

**pg_service.conf** ([postgresql.org/docs/current/libpq-pgservice.html](https://www.postgresql.org/docs/current/libpq-pgservice.html)):
- Formato INI: `[nome_do_servico]` + `key=value`, comentários com `#`.
- Local padrão `~/.pg_service.conf`, sobreposto por `PGSERVICEFILE`.
- Precedência oficial de merge (maior → menor): **string de conexão →
  service file → env vars → defaults**. Copiamos essa semântica.
- A doc NÃO impõe permissões ao arquivo (diferente do `.pgpass`, que exige
  0600) — como nosso arquivo pode executar comandos, seremos MAIS rígidos.

### Decisões de design

1. **Onde vive**: novo módulo `pg_lens_core/src/settings.rs` — a resolução
   de conexão é frontend-agnóstica (a Web Lens da Fase 6 usará igual). O
   `clap` continua só na TUI; o core recebe uma struct `ConnSpec` (dsn
   opcional, service opcional, overrides) e devolve
   `(tokio_postgres::Config, ConnLabel)` onde `ConnLabel` é o rótulo seguro
   para header/logs (host/serviço, JAMAIS senha).
2. **Formato do arquivo próprio**: TOML (`serde` + crate `toml`), em
   `~/.config/pg_lens/services.toml` (XDG; sobreposto por
   `--services-file` / env `PG_LENS_SERVICES_FILE`):

   ```toml
   [services.prod]
   host = "db.prod.internal"
   port = 5432
   user = "pg_monitor_ro"
   dbname = "app"
   application_name = "pg_lens"
   connect_timeout_secs = 5
   password_cmd = "vault kv get -field=password secret/pg/prod"

   [services.staging]
   host = "db.staging.internal"
   user = "postgres"
   # açúcar sintático: password com $(...) é tratado como password_cmd
   password = "$(op read op://infra/pg-staging/password)"
   ```

   Por que TOML e não INI compatível com pg_service.conf: nosso arquivo tem
   semântica a mais (`password_cmd`) e queremos parse tipado com serde sem
   ambiguidade. Compatibilidade de leitura com o `pg_service.conf` real fica
   como fase opcional (C3).
3. **`password_cmd`**: executado via `tokio::process::Command` como
   `sh -c "<cmd>"` (mesma confiança do shellrc do usuário — o arquivo é
   dele), com timeout (10s), stdout com trailing newline aparado vira a
   senha, exit != 0 vira erro claro (stderr incluído na mensagem, stdout
   NUNCA). Açúcar: `password = "$(...)"` (regex `^\$\(.*\)$`) é convertido
   para `password_cmd` no parse.
4. **Permissões**: como o arquivo executa comandos, na leitura: se o modo
   for group/world-writable → **recusar** com erro explicando; se contiver
   `password` em texto puro e for group/world-readable → recusar (espírito
   do `.pgpass`); caso contrário apenas warning se != 0600. (Unix only;
   no-op em outras plataformas.)
5. **Precedência final** (maior → menor), espelhando libpq:
   1. `--dsn` explícito (campos presentes nele);
   2. entrada do services file (`--service` / env `PG_LENS_SERVICE`, com
      fallback para `PGSERVICE`);
   3. env vars libpq (`PGHOST`, `PGPORT`, `PGDATABASE`, `PGUSER`,
      `PGPASSWORD`, `PGAPPNAME`, `PGCONNECT_TIMEOUT`);
   4. defaults atuais (`host=localhost user=postgres`).
   `--dsn` e `--service` simultâneos = erro de CLI (mantém o modelo mental
   simples; o merge parcial do libpq confunde mais do que ajuda aqui).
6. **Segurança transversal**: a senha resolvida entra via
   `Config::password()` — nunca é reinterpolada em string de DSN; nenhum
   `Debug`/log de `Config` ou da senha; o `ConnLabel` do header continua
   mostrando só host (comportamento já existente do `dsn_host`).

### Anti-padrões (NÃO FAZER)
- ❌ Reinterpolar a senha resolvida numa string DSN (usar o builder).
- ❌ Logar/exibir `Config` via Debug, a senha, ou o stdout do password_cmd.
- ❌ Parsear shell por conta própria — delegar a `sh -c`.
- ❌ Executar password_cmd a cada tick do poller — resolver UMA vez por
  (re)conexão; no reconnect com backoff, re-executar é correto (tokens
  expiram), mas nunca dentro do loop de render.
- ❌ `clap`/CLI no core; `unwrap()` no core; bloquear a UI esperando o
  comando (resolução acontece na task do poller, antes do connect).
- ❌ Inventar env vars não-libpq para o que o libpq já nomeia (exceção:
  prefixo `PG_LENS_*` para o que é nosso: arquivo/serviço).

---

## Fase C1 — Resolução por env vars + refactor para `Config` builder

**Objetivo:** `pg_lens_tui` conecta usando `PGHOST`/`PGPORT`/`PGDATABASE`/
`PGUSER`/`PGPASSWORD`/`PGAPPNAME`/`PGCONNECT_TIMEOUT` quando `--dsn` não os
especifica, com a precedência da Fase C0. Sem services file ainda.

**O que implementar:**
1. `pg_lens_core/src/settings.rs`: struct `ConnSpec { dsn: Option<String>,
   env: HashMap<String,String>, .. }` e
   `fn resolve(spec) -> Result<(tokio_postgres::Config, ConnLabel), SettingsError>`.
   O ambiente entra **injetado** (parâmetro), não lido via `std::env` dentro
   da função — é o que torna a matriz de precedência testável sem
   `set_var`/flaky. A TUI captura `std::env::vars()` uma vez no main.
2. Parse do `--dsn` continua via `Config::from_str` (aceita `key=value` e
   URL de graça); campos ausentes no DSN são preenchidos pelos env vars via
   builder (`get_hosts().is_empty()` etc. para detectar ausência).
3. `db::connect` passa a receber `&tokio_postgres::Config` em vez de `&str`
   (o poller idem). `ConnLabel` substitui o `dsn_host()` atual da TUI
   (mover/adaptar a lógica para o core, mantendo a garantia de não vazar
   credencial — os testes existentes de `dsn_host` migram juntos).
4. `--interval` da CLI ganha fallback `PGCONNECT_TIMEOUT`? NÃO — connect
   timeout e poll interval são coisas distintas; `PGCONNECT_TIMEOUT` mapeia
   para `Config::connect_timeout` apenas.
5. README: seção "Connecting" com a tabela de env vars suportadas e a
   precedência.

**Referências:** Fase C0 (Config builder, libpq envars); os getters de
`Config` em docs.rs para detecção de campo ausente.

**Verificação:**
- [ ] Gate verde (build/clippy/test) + testes novos: matriz de precedência
      (dsn vence env; env vence default; PGPASSWORD aplicado; PGUSER — não
      PGUSERNAME) como testes puros com env injetado.
- [ ] Live (Docker PG 16, porta 54316): `PGHOST=localhost PGPORT=54316
      PGUSER=postgres PGPASSWORD=pg cargo run -p pg_lens_tui` conecta SEM
      `--dsn`; harness `scripts/e2e_pty_live.py` passa.
- [ ] `--dsn` com host explícito ignora `PGHOST` conflitante (teste unitário
      + prova live com PGHOST=lixo).
- [ ] `grep -rn "std::env" crates/pg_lens_core/` → apenas fora de
      `settings::resolve` (injeção respeitada) — idealmente vazio no core.
- [ ] Header continua sem exibir senha em nenhum cenário (capture PTY).

**Anti-padrões:** os da Fase C0; não usar `std::env::set_var` em testes.

---

## Fase C2 — Services file com `password_cmd`

**Objetivo:** `pg_lens_tui --service prod` conecta usando
`~/.config/pg_lens/services.toml`, resolvendo a senha por comando externo;
o arquivo nunca precisa conter senha em texto puro.

**O que implementar:**
1. `settings.rs`: `ServicesFile` (serde/TOML — adicionar crate `toml` ao
   core), localização XDG (`dirs` crate ou `$XDG_CONFIG_HOME` manual —
   decidir no momento, citar doc do crate escolhido), override por
   `--services-file`/`PG_LENS_SERVICES_FILE`. Campos por serviço: `host`,
   `port`, `user`, `dbname`, `application_name`, `connect_timeout_secs`,
   `password` (aceito mas desencorajado; suporta açúcar `$(...)`),
   `password_cmd`. `password` + `password_cmd` juntos = erro de validação.
2. Checagem de permissões conforme Fase C0 (recusar writable por
   grupo/mundo; recusar password puro legível por grupo/mundo; warning
   != 0600). Unix only via `std::os::unix::fs::PermissionsExt`.
3. `resolve_password_cmd(cmd) -> Result<SecretString-like, _>`:
   `tokio::process::Command::new("sh").arg("-c").arg(cmd)`, timeout 10s
   (`tokio::time::timeout`), trim de `\n`/`\r\n` final, exit != 0 → erro com
   stderr (stdout jamais na mensagem). A resolução roda **na task do
   poller** antes de cada tentativa de conexão (inclusive reconexões — o
   comando é re-executado, tokens rotativos funcionam).
4. Integração da precedência completa da Fase C0 no `resolve()`
   (service < env vars? NÃO — atenção: precedência libpq é **dsn → service
   → env → default**; o service file VENCE env vars).
5. CLI: `--service <name>` (env `PG_LENS_SERVICE`, fallback `PGSERVICE`),
   `--services-file <path>`, `--list-services` (imprime nomes + host/user —
   sem segredos — e sai, exit 0). `--dsn` e `--service` mutuamente
   exclusivos (clap `conflicts_with`).
6. Erros de UX: serviço inexistente lista os disponíveis; password_cmd
   falhando aparece como `PollerStatus::Error` no banner (UI viva, backoff
   re-tenta e re-executa o comando) — mesmo caminho de resiliência da
   Fase 3.
7. README: seção "Services file" com exemplo completo (vault, 1password
   `op read`, `security find-generic-password` do macOS), nota de segurança
   (o arquivo executa comandos → trate como código; 0600) e a precedência.

**Referências:** Fase C0; docs.rs do crate `toml` e do crate de dirs
escolhido (verificar API atual antes de codar); `tokio::process` em
docs.rs/tokio.

**Verificação:**
- [ ] Gate verde + testes: parse TOML (incl. açúcar `$(...)` → password_cmd;
      password+password_cmd → erro), permissões (arquivos temp com modos
      0600/0644/0666 — 0644 com password puro recusa, 0644 sem password
      warning, 0666 sempre recusa), precedência dsn→service→env→default,
      `resolve_password_cmd` com `echo`/`false`/comando lento (timeout).
- [ ] Live (Docker PG 16): services.toml com
      `password_cmd = "echo pg"` → `--service local16` conecta; harness
      passa. Com `password_cmd = "false"` → banner de erro claro, UI
      responsiva, e trocar o arquivo + aguardar backoff reconecta.
- [ ] `--list-services` imprime e sai; serviço inexistente → erro com lista.
- [ ] Nenhuma senha em capture/stderr/log em nenhum cenário (inspecionar
      captures do harness + saída de erro do password_cmd).
- [ ] `grep -rn "clap" crates/pg_lens_core/` → vazio.

**Anti-padrões:** os da Fase C0, principalmente: nunca ecoar stdout do
comando em erro/log; não executar o comando no loop de render; não fazer
cache eterno da senha (re-resolver a cada reconexão).

---

## Fase C3 (opcional/backlog) — Compat e conveniências

Somente após C1+C2 verificadas. Itens independentes, priorizar sob demanda:

1. **Leitura do `pg_service.conf` real** (INI): se `--service`/`PGSERVICE`
   não achar o nome no nosso TOML, tentar `PGSERVICEFILE` /
   `~/.pg_service.conf` (formato da Fase C0). Sem password_cmd nesse formato
   (é só compat).
2. **`.pgpass`/`PGPASSFILE`**: resolução de senha padrão libpq (match
   host:port:db:user, exigir 0600) quando nenhuma outra fonte de senha
   existir.
3. **Picker de serviço na TUI**: sem `--dsn`/`--service` e com services.toml
   presente → tela inicial de seleção (lista navegável, Enter conecta).
   Reaproveita o padrão Table+TableState existente.
4. **Fase 6 (web)**: `pg_lens serve --service prod` — o `resolve()` do core
   já serve; garantir que o payload JSON jamais inclua ConnLabel além de
   host (auditar Serialize).

**Verificação (quando implementados):** compat testada contra um
`pg_service.conf` gerado à mão; picker coberto pelo harness PTY (navegar +
Enter conecta no mock); greps de segredo nos payloads da web.

---

## Sequência recomendada de execução

C1 → C2 (C3 sob demanda). Cada fase: implementar → gate → verificação live →
commit (`feat: connection env vars (C1)` / `feat: services file with
password_cmd (C2)`) — mesmo protocolo do PLAN.md.
