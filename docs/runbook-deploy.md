# Runbook — deploy, rollback, recovery (sessione 4)

Il percorso di vita di sigiledd sul box: deploy via supervisor, rollback via
sha, recovery a stack morto. Le immagini base sono nella seconda metà.

## 1. Il supervisor (la resurrezione come servizio)

Codice: repo `sigiled-supervisor`, `supervisor/src/main.rs` (un file, un
dovere — suoi requisiti DEC-01..07). Deploy **indipendente da SIGILED**:
mai `[app]`, unit systemd o compose a mano sul box.

### Prima installazione (operatore, sul box)

```sh
git clone git@github.com:ivan-saorin/sigiled-supervisor.git /opt/sigiled-supervisor
cd /opt/sigiled-supervisor/supervisor && cargo build --release
install -m 755 target/release/sigiled-supervisor /usr/local/bin/
```

Unit systemd (`/etc/systemd/system/sigiled-supervisor.service`):

```ini
[Unit]
Description=SIGILED resurrection service
After=network.target

[Service]
Environment=SUPERVISOR_TOKEN=<bearer statico, generato lungo>
Environment=SIGILED_REPO_DIR=/opt/sigiled
Environment=SIGILED_HEALTH_URL=http://localhost:8080/healthz
Environment=SUPERVISOR_LOG=/var/log/sigiled-supervisor.log
# default: docker compose up -d --build sigiled — override se serve:
# Environment=SUPERVISOR_RESTART_CMD=systemctl restart sigiledd
ExecStart=/usr/local/bin/sigiled-supervisor
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

`systemctl enable --now sigiled-supervisor`. Vhost edge
`supervisor.016180.xyz` → `localhost:9090` in Caddy (questione aperta §6.2
del supervisor: esposto con auth, così i driver lo chiamano a sigiled morto).

### Deploy di sigiledd

```sh
curl -X POST -H "Authorization: Bearer $SUPERVISOR_TOKEN" \
  https://supervisor.016180.xyz/sigiled/restart
```

Senza body: checkout di `origin/master` → `SUPERVISOR_RESTART_CMD` → attesa
health (fino a 60 s) → report `{previous_sha, new_sha, healthy,
duration_secs, log_tail}`. Un secondo restart mentre uno è in corso → 409.

### Rollback

```sh
curl -X POST -H "Authorization: Bearer $SUPERVISOR_TOKEN" \
  -H "Content-Type: application/json" -d '{"sha": "<sha buono>"}' \
  https://supervisor.016180.xyz/sigiled/restart
```

Lo sha esplicito è il rollback (supervisor DEC-04): il `previous_sha` di un
report andato male è il candidato naturale. Stato corrente:
`GET /sigiled/status` → `{deployed_sha, healthy, last_restart}`.

### Recovery a stack morto

In ordine di gravità:

1. **sigiledd morto, supervisor vivo**: `POST /sigiled/restart` (sopra).
   È il caso per cui il supervisor esiste.
2. **Anche il supervisor morto**: SSH sul box →
   `systemctl restart sigiled-supervisor` → caso 1. Il suo log è
   `/var/log/sigiled-supervisor.log`, append-only, leggibile anche a tutto
   fermo.
3. **Box irraggiungibile**: console fisica/provider. Dopo il boot: docker e
   il supervisor partono da systemd; i workload SIGILED si riaprono con
   sessioni normali — il repo è l'unica memoria, i container sono bestiame
   (contratto, regole 3/5): non c'è stato da recuperare.

Nessuna auto-remediation (supervisor DEC-05): il restart lo decide un
umano — o un driver su suo ordine.

## 2. Immagine base `vm-base` (DEC-17/20)

L'immagine base dei workspace è pubblica su ghcr:
`ghcr.io/ivan-saorin/vm-base:x.y.z`. Il pull non richiede credenziali — né
dal box, né da chi self-hosta. Il PAT serve solo per il push.

### Build + push (operatore, sul box o qualunque docker host)

```sh
# una tantum: login con PAT classic, scope write:packages
docker login ghcr.io -u ivan-saorin

# build (tag = ghcr.io/ivan-saorin/vm-base:<version da vm-base/Cargo.toml>)
images/build-vm-base.sh

# build + push
PUSH=1 images/build-vm-base.sh
```

### Rendere il package pubblico (una tantum per package)

Il primo push crea il package **privato**. Per DEC-20 va reso pubblico:

1. GitHub → profilo `ivan-saorin` → **Packages** → `vm-base`.
2. **Package settings** (colonna destra).
3. **Danger Zone → Change visibility → Public** — digitare il nome del
   package per confermare.
4. Verifica da un host qualunque, senza login:
   `docker pull ghcr.io/ivan-saorin/vm-base:0.1.0`.

### Bump di versione (quando una sessione modifica l'agent)

1. La sessione alza `version` in `vm-base/Cargo.toml` e aggiorna il pin
   `FROM` in `template/Dockerfile` (+ il pin `template = "vm-tmpl@x.y.z"`
   dove ricepito) — stesso commit.
2. L'operatore esegue `PUSH=1 images/build-vm-base.sh` sul box.
3. I progetti adottano il tag nuovo on demand (DEC-05): mai automaticamente.

## 3. Self-host (chi non è lo stack di riferimento)

- Registry proprio: `VM_BASE_REGISTRY=reg.example.com PUSH=1
  images/build-vm-base.sh` e stesso valore nel `FROM` del template.
- Identità git dei commit di sessione: env-driven (`GIT_AUTHOR_*` /
  `GIT_COMMITTER_*`), default neutri compilati in vm-base.
- Env di sigiledd, tutte con default documentati nel codice:
  `SIGILED_BOOTSTRAP_BEARER`, `SIGILED_OIDC_BASE` (+ vedi
  `authentik-setup.md`), `SIGILED_TEMPLATE_LATEST`, `SIGILED_REPOS_DIR`.
- Supervisor: `SUPERVISOR_RESTART_CMD` adatta il restart a compose, systemd
  o qualunque altra cosa (`{sha}` viene sostituito).
