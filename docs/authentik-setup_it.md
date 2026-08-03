# Setup Authentik per SIGILED (sessione 3)

Il design è §1 di `sigiled-v2.md`: due gambe — machine (`client_credentials`
per driver) e umana (device flow via `sigiled-device`). Questo documento è il
runbook dell'operatore.

## Prerequisito: token API admin

Il provisioning richiede un token API di un utente **admin**:

1. Admin interface → **Directory → Tokens** → Create.
2. Utente: il tuo (o un service account con permessi admin);
   Intent: **API Token**; scadenza a piacere.

**Nota di esperienza (2026-08-02):** il token dell'**outpost embedded**
(`ak-outpost-…`) non basta — le liste tornano vuote (object-level permissions)
e ogni create risponde 403. Se `tools/authentik-provision.sh` stampa liste
vuote o 403, stai usando il token sbagliato.

## Provisioning automatico

```sh
AUTHENTIK_API_TOKEN=<token admin> tools/authentik-provision.sh
```

Idempotente. Crea: scope mapping `sigiled-groups` (claim `groups` per la
capability map), gruppi `stack:admins`/`stack:drivers`, provider+application
`sigiled-device` (public, gamba umana) e un provider **confidential per
driver** (`sigiled-claude`, `sigiled-kimi`; override con `DRIVERS="…"`),
poi «prima» ogni driver (primo mint → nasce il service account
`ak-<driver>-client_credentials`) e lo mette in `stack:drivers`.
Lo script chiude stampando i passi manuali residui (il TUO utente in
stack:admins, device flow sul brand, dove vivono i client_secret).

**Eseguito con successo il 2026-08-02** sull'IdP dello stack (2026.5.6):
provider pk 2/3/4, e2e completo — JWT mintato con client_credentials,
claim `groups: ["stack:drivers"]`, validato da sigiledd via JWKS live.

### Trappola: `grant_types` è una allowlist (versioni recenti)

Un provider creato via API con `grant_types: []` **rifiuta ogni grant** con
`invalid_grant` *silenzioso* (nessun evento nel log di Authentik — il
rifiuto avviene in `TokenParams.__post_init__` prima di tutto il resto).
Diagnostica: se il mint fallisce invalid_grant, il secret combacia e gli
eventi tacciono, controlla `grant_types` sul provider. Lo script li imposta
espliciti: `client_credentials` per i driver, `device_code` per
sigiled-device — che è poi il «grant ristretto» prescritto dal design §1.8.

## Provisioning manuale (fallback, UI)

1. **Scope mapping**: Customization → Property mappings → Create → Scope
   mapping. Nome `sigiled: groups claim`, scope `sigiled-groups`, expression:
   `return {"groups": [g.name for g in request.user.ak_groups.all()]}`.
2. **Gruppi**: Directory → Groups → `stack:admins`, `stack:drivers`.
   Te stesso in admins.
3. **Provider driver** (uno per driver, §1.3): Applications → Providers →
   Create → OAuth2/OpenID. Nome/client_id `sigiled-<driver>`, Confidential,
   signing key RS256 (il self-signed di Authentik va bene), property mapping
   `sigiled: groups claim`. Poi Applications → Create con lo stesso slug,
   legata al provider.
4. **Provider device** (§1.4): come sopra ma **Public**, client_id
   `sigiled-device`. Brand → Default flows → Device code flow attivo.
5. **Service account driver**: al primo `client_credentials` Authentik crea
   il service account dell'application — aggiungilo a `stack:drivers`.

## Wiring di sigiledd (stack env)

| Env | Valore | Note |
|---|---|---|
| `SIGILED_OIDC_BASE` | URL dell'IdP | gamba OIDC; assente = gamba spenta |
| `SIGILED_BOOTSTRAP_BEARER` | bearer legacy | finestra dual-auth §1.7; a fine migrazione si toglie |
| `SIGILED_DEVICE_CLIENT_ID` | `sigiled-device` | default già giusto |
| `SIGILED_ADMIN_GROUP` / `SIGILED_DRIVER_GROUP` | `stack:admins` / `stack:drivers` | default già giusti |
| `AUTHENTIK_API_TOKEN` | token admin | serve solo al provisioning/manutenzione, non a sigiledd |

## Verifica

```sh
# discovery pubblica del provider driver
curl -s $IDP/application/o/sigiled-claude/.well-known/openid-configuration | head

# la gamba machine minta un token (il client_secret sta nella skill del driver)
curl -s -X POST $IDP/application/o/token/ \
  -d grant_type=client_credentials -d client_id=sigiled-claude \
  -d client_secret=$SECRET -d scope="openid profile sigiled-groups"
```

Il JWT risultante deve avere `iss` sotto l'IdP, firma RS256 e claim `groups`
con `stack:drivers`: è esattamente ciò che `sigiledd` valida (auth.rs).

## Dove vanno i segreti

- `client_secret` di ogni driver → **solo nella skill di quel driver**
  (regole di handling della skill: mai echo, mai commit).
- Token API admin → stack env dell'operatore, mai nei repo.
- I token del device flow non escono mai da SIGILED (custodia §1.4).
