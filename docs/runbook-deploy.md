# Runbook — deploy e immagini (bozza, sessione 1b)

**Stato:** bozza. La versione completa (deploy via supervisor, rollback via
sha, recovery a stack morto) arriva con la sessione 4 del build plan.

## Immagine base `vm-base` (DEC-17/20)

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

Nota: se il package è legato al repo (`org.opencontainers.image.source`),
la visibilità del package resta indipendente da quella del repo — il flip
del repo (DEC-19) non tocca questa impostazione.

### Bump di versione (quando una sessione modifica l'agent)

1. La sessione alza `version` in `vm-base/Cargo.toml` e aggiorna il pin
   `FROM` in `template/Dockerfile` (+ il pin `template = "vm-tmpl@x.y.z"`
   dove ricepito) — stesso commit.
2. L'operatore esegue `PUSH=1 images/build-vm-base.sh` sul box.
3. I progetti adottano il tag nuovo on demand (DEC-05): mai automaticamente.

## Self-host (chi non è lo stack di riferimento)

- Registry proprio: `VM_BASE_REGISTRY=reg.example.com PUSH=1
  images/build-vm-base.sh` e stesso valore nel `FROM` del template.
- L'identità git dei commit di sessione è env-driven
  (`GIT_AUTHOR_NAME/EMAIL`, `GIT_COMMITTER_NAME/EMAIL`, iniettate dal
  control plane alla creazione del container); i default compilati in
  vm-base sono neutri (`sigiled-session` / `session@sigiled.dev`).

## Deploy del control plane (sigiledd)

Rimandato: fino al cutover (build plan §6) il control plane in produzione
resta MGR v1; sigiledd si esercita in run locale dentro le sessioni. Il
deploy reale passa da `sigiled-supervisor` (sessione 4) e sarà documentato
qui.
