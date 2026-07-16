# pg_lens — Plano: Distribuição via gerenciadores de pacote

> Levar o pg_lens até os usuários por canais de instalação idiomáticos:
> Homebrew (macOS/Linux), Docker/GHCR (Web Lens), deb/rpm, crates.io +
> cargo-binstall — resolvendo de quebra o atrito do Gatekeeper no macOS
> (brew e curl não aplicam o atributo de quarentena).

Segue as convenções do [PLAN.md](PLAN.md): fases autocontidas executáveis
via `/claude-mem:do`, checklist e anti-padrões por fase. Pré-requisito
geral: release v0.1.0 publicada (feita) e workflow release.yml funcionando.

---

## Fase D0 — Descoberta e decisões (CONCLUÍDA — resultados abaixo)

### Fatos verificados (fontes)

**Homebrew taps** ([docs.brew.sh/Taps](https://docs.brew.sh/Taps)):
- Repo DEVE chamar `homebrew-<nome>` (ex.: `dog-hero/homebrew-tap`);
  usuários usam `brew tap dog-hero/tap` + `brew install pg_lens`, ou
  direto `brew install dog-hero/tap/pg_lens`.
- Fórmulas ficam em `Formula/*.rb` no repo do tap (convenção padrão).
- Fórmula de binário pré-compilado: `url` apontando para o tarball da
  release + `sha256` por plataforma (blocos `on_macos`/`on_linux` +
  `Hardware::CPU.arm?`), `bin.install "pg_lens_tui"`.

**cargo-binstall** ([github.com/cargo-bins/cargo-binstall](https://github.com/cargo-bins/cargo-binstall)):
- **Exige o crate publicado no crates.io** (é de lá que ele lê o repo).
- `[package.metadata.binstall]` com `pkg-url` template
  (`{ repo }/releases/download/v{ version }/{ name }-v{ version }-{ target }.{ archive-format }`),
  `pkg-fmt = "tgz"`, `bin-dir` — nosso padrão de nome de artefato já é
  compatível, só mapear.

**dist (ex-cargo-dist)** ([github.com/axodotdev/cargo-dist](https://github.com/axodotdev/cargo-dist)):
- Ativamente mantido (v0.32.0, mai/2026; rebrand para "dist").
- Automatiza: tarballs, installers shell, tap homebrew, GitHub Releases.
- **Decisão: NÃO migrar agora.** Nosso release.yml hand-rolled está
  verificado (musl via zigbuild + frontend build). Adotar dist é uma
  consolidação futura (Fase D5) — avaliar quando os canais manuais
  virarem manutenção repetitiva.

### Decisões de design

1. **Ordem dos canais por custo/benefício**: D1 brew tap (resolve o
   Gatekeeper, público macOS/Linux dev) → D2 Docker/GHCR (deploy do
   Web Lens; usa GITHUB_TOKEN nativo, zero segredos novos) → D3 deb/rpm
   (servidores Linux) → D4 crates.io + binstall (público Rust) →
   D5 consolidação (dist) / notarização Apple.
2. **Nome do binário**: hoje é `pg_lens_tui`, mas o produto chama-se
   `pg_lens` e o binário contém TUI **e** web (`serve`). **Renomear o
   binário para `pg_lens` na Fase D1** (via `[[bin]] name = "pg_lens"`),
   antes de espalhar o nome antigo pelos canais. Fazer isso numa release
   nova (v0.2.0) — os canais nascem já com o nome certo.
3. **Segredos/ações que dependem do dono do repo** (bloqueiam fases):
   - D1: criar o repo `dog-hero/homebrew-tap` (vazio) + PAT fine-grained
     com write no tap, salvo como secret `TAP_GITHUB_TOKEN` no pg_lens.
   - D4: token do crates.io como secret `CARGO_REGISTRY_TOKEN`.
   - D5 (notarização): conta Apple Developer (US$ 99/ano) + certificado
     Developer ID como secrets. Opcional — brew/curl já contornam o
     Gatekeeper.

### Anti-padrões (NÃO FAZER)
- ❌ Fórmula brew buildando da fonte (exigiria Rust+Node do usuário) —
  usar os binários da release.
- ❌ sha256 desatualizado/na mão: a atualização da fórmula é AUTOMÁTICA
  no workflow de release, nunca manual.
- ❌ Docker rodando como root ou com imagem gorda — musl estático em
  `FROM scratch`/`alpine`, USER não-root, só o binário.
- ❌ Publicar no crates.io sem o `dist/` do frontend embutido no pacote
  (o build quebraria) — usar `include` no Cargo.toml e publicar via CI
  que builda o frontend antes.
- ❌ Duplicar lógica de empacotamento entre canais — os tarballs da
  release são a fonte única; canais só referenciam/reempacotam.

---

## Fase D1 — Homebrew tap com atualização automática

**Objetivo:** `brew install dog-hero/tap/pg_lens` funcionando, com a
fórmula atualizada automaticamente a cada release.

**Pré-requisito (ação do dono):** criar `dog-hero/homebrew-tap` (repo
público — taps precisam ser clonáveis sem auth) e o secret
`TAP_GITHUB_TOKEN` (PAT fine-grained, contents:write no tap) no pg_lens.

**O que implementar:**
1. **Renomear o binário para `pg_lens`** (`[[bin]]` em pg_lens_tui) —
   atualizar README, harnesses de e2e (`scripts/*.py` referenciam o path
   do binário), workflow release (`env.BIN`), CLAUDE.md.
2. No repo do tap: `Formula/pg_lens.rb` — classe `PgLens`, `desc`,
   `homepage`, `license "MIT"`, blocos por plataforma
   (`on_macos`/`on_linux` × arm/intel) com `url` do tarball da release e
   `sha256` (dos arquivos `.sha256` que a release já gera),
   `bin.install "pg_lens"`, bloco `test do` (`pg_lens --help`).
3. No release.yml do pg_lens: job `update-tap` (needs: release) que
   clona o tap com `TAP_GITHUB_TOKEN`, regenera a fórmula a partir de um
   template com as novas URLs/sha256s (baixa os `.sha256` dos artefatos)
   e commita/pusha no tap.
4. README: seção Homebrew no topo de Installation.

**Verificação:**
- [ ] Tag nova (v0.2.0, com o rename) → release publica e o job
      update-tap commita a fórmula no tap automaticamente.
- [ ] Em um Mac: `brew install dog-hero/tap/pg_lens` instala; `pg_lens
      --mock` abre a TUI **sem prompt do Gatekeeper**; `brew test
      pg_lens` passa.
- [ ] `brew audit --strict dog-hero/tap/pg_lens` sem erros relevantes.
- [ ] Gate do repo continua verde (rename não quebrou e2e/CI).

---

## Fase D2 — Imagem Docker no GHCR (foco: Web Lens)

**Objetivo:** `docker run ghcr.io/dog-hero/pg_lens serve --listen
0.0.0.0:8080` (com token) monitorando um Postgres — o caminho natural de
deploy do Web Lens.

**O que implementar:**
1. `Dockerfile` multi-stage: stage 1 reutiliza a receita musl já testada
   (rust:1-alpine + node para o frontend); stage final `FROM scratch`
   (binário estático) ou `alpine` (se precisar de sh para password_cmd —
   **decisão**: alpine, porque `password_cmd` executa via `sh -c` e é
   feature central), `USER 65534`, `ENTRYPOINT ["/pg_lens"]`.
2. release.yml: job `docker` com docker/build-push-action, tags
   `ghcr.io/dog-hero/pg_lens:{versão}` e `:latest`, plataformas
   linux/amd64 + linux/arm64 (buildx QEMU) — autenticação via
   `GITHUB_TOKEN` (packages:write), sem segredos novos.
3. README: seção Docker com exemplo compose (pg_lens + variáveis
   PG*/PG_LENS_AUTH_TOKEN).

**Verificação:**
- [ ] Imagem local builda e roda: serve --mock responde /api/snapshot.
- [ ] Contra Postgres real em rede docker: dashboard funciona; tamanho
      da imagem < 30 MB; roda como não-root; `password_cmd` funciona
      dentro do container (sh presente).
- [ ] Push na release: `docker pull ghcr.io/dog-hero/pg_lens:latest` de
      outra máquina funciona (repo/packages público).

---

## Fase D3 — Pacotes deb/rpm anexados à release

**Objetivo:** `.deb` e `.rpm` (amd64+arm64) como artefatos da release,
para instalação direta em servidores.

**O que implementar:**
1. [nfpm](https://nfpm.goreleaser.com) (binário único, config YAML,
   gera deb+rpm+apk do mesmo input — verificar docs atuais na
   implementação) com `nfpm.yaml`: nome, arch, binário, LICENSE,
   README, seção `contents`.
2. release.yml: job `packages` (needs: build-linux) que baixa os
   artefatos musl, roda nfpm para cada formato×arch e anexa à release
   (softprops já aceita mais files).
3. README: instruções `dpkg -i` / `rpm -i`.

**Verificação:**
- [ ] `dpkg -i` num container debian:stable-slim instala e `pg_lens
      --mock` roda; idem `rpm -i`/`dnf install` em fedora.
- [ ] `dpkg -c` mostra layout correto (/usr/bin/pg_lens, docs).
- [ ] Lint: `lintian` sem erros graves (warnings aceitáveis).

---

## Fase D4 — crates.io + cargo-binstall

**Objetivo:** `cargo install pg_lens` e `cargo binstall pg_lens`
funcionando.

**Pré-requisito (ação do dono):** conta crates.io + secret
`CARGO_REGISTRY_TOKEN`. Verificar disponibilidade dos nomes `pg_lens`,
`pg_lens_core`, `pg_lens_web` ANTES (crates.io não permite reuso).

**O que implementar:**
1. Metadados de publicação nos 3 crates (description, license,
   repository, keywords, categories); versões sincronizadas; path deps
   ganham `version =`.
2. O problema do frontend: `cargo install pg_lens` na máquina do usuário
   não pode exigir Node → o pacote publicado do pg_lens_web DEVE conter
   o `dist/` buildado: `include = [...]` no Cargo.toml do pg_lens_web
   (o `include` do cargo package ignora o .gitignore — verificar na doc
   do cargo) e publicação SEMPRE via CI (workflow `publish.yml` em tag,
   precedido do build do frontend).
3. `[package.metadata.binstall]` no crate do binário mapeando nosso
   padrão de artefato (`pkg-url` com `v{ version }`, `pkg-fmt = "tgz"`,
   `bin-dir` refletindo o diretório interno do tarball).
4. Ordem de publish: core → web → bin (dependências primeiro), com
   `cargo publish --dry-run` no CI de PR.

**Verificação:**
- [ ] `cargo publish --dry-run` verde para os 3 crates no CI.
- [ ] Após publicar: `cargo install pg_lens` numa máquina SEM Node
      compila e roda com o dashboard web embutido.
- [ ] `cargo binstall pg_lens` baixa o binário da release (sem
      compilar) e roda.

---

## Fase D5 (opcional/futuro) — Consolidação e assinatura

1. **Avaliar migração para `dist`** (ex-cargo-dist): se D1–D4 virarem
   manutenção repetitiva, dist gera release.yml + installers + tap de
   forma declarativa. Custo: refazer o pipeline verificado; ganho:
   menos YAML nosso. Reavaliar com o projeto estável.
2. **Notarização Apple** (requer conta Developer): codesign com
   Developer ID + `xcrun notarytool submit --wait` + `stapler` no job
   macOS da release; secrets: certificado .p12 + credenciais da conta.
   Elimina o passo `xattr`/curl para downloads via browser.
3. **Canais comunitários** (quando houver tração): AUR (PKGBUILD),
   nixpkgs, Winget/Scoop se um dia houver build Windows.

---

## Sequência recomendada

D1 (brew + rename para v0.2.0) → D2 (docker) → D3 (deb/rpm) → D4
(crates.io) → D5 sob demanda. Cada fase: implementar → gate → verificação
com instalação real → commit/push → (quando fizer sentido) tag de release.
Ações do dono antes de começar: criar `dog-hero/homebrew-tap` + secret
`TAP_GITHUB_TOKEN` (D1); token crates.io (D4).
