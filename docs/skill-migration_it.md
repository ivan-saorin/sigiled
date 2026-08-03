# Migrazione delle skill driver al nuovo auth (sessione 3)

Testo pronto da incollare nelle skill dei driver quando la finestra dual-auth
(§1.7) si apre sul serio. Fino ad allora il bearer legacy continua a
funzionare: la migrazione è per-driver e non ha un big bang.

## Cosa cambia per una skill

Prima: un bearer di stack monolitico. Dopo: una coppia `client_id` /
`client_secret` **propria del driver** (provider Authentik `sigiled-<driver>`)
con cui la skill si minta da sola token brevi. Un leak compra minuti di UN
driver, revocabile in un click, non le chiavi dello stack.

## Blocco da incollare nella skill

> ### Credenziali e token
>
> ```
> CLIENT_ID     = sigiled-<driver>
> CLIENT_SECRET = <dal provider Authentik, mai in chiaro fuori dalla skill>
> IDP           = https://auth.016180.xyz
> ```
>
> Prima di chiamare l'API, minta un access token:
>
> ```
> POST {IDP}/application/o/token/
>   grant_type=client_credentials
>   client_id={CLIENT_ID}
>   client_secret={CLIENT_SECRET}
>   scope=openid profile sigiled-groups
> ```
>
> Usa `access_token` come `Authorization: Bearer …` verso l'API della
> piattaforma. Il token dura poco per design: su **401 si minta un token
> nuovo e si riprova una volta**; se il 401 persiste, il provider è stato
> revocato o ruotato — chiedi all'operatore, non insistere.
>
> Handling: mai echo del secret in chat, log o commit; vive solo in questa
> skill. Rotazione = l'operatore rigenera il secret sul provider e aggiorna
> questa riga.
>
> ### Approval umana (elevate)
>
> Alcuni verbi rispondono `403 capability requires approval` (nuovi progetti,
> verbi app, sessioni sui progetti piattaforma). Procedura:
>
> 1. `POST /sigiled/auth/elevate` → `{verification_uri, user_code, expires}`.
> 2. Riporta in chat: «vai su {verification_uri}, codice {user_code}» —
>    stampare il codice è sicuro: autorizza soltanto, i token restano
>    server-side.
> 3. L'operatore approva dal browser; verifica con
>    `GET /sigiled/auth/approvals` e riprova il verbo negato.
>
> L'approval dura ore/giorni (configurazione provider): non richiederla
> se `approvals` ne mostra già una viva per il tuo driver.

## Sequenza di migrazione (per driver)

1. L'operatore crea il provider `sigiled-<driver>` (vedi
   `authentik-setup.md`) e mette il service account in `stack:drivers`.
2. Il blocco sopra entra nella skill, col secret di quel driver.
3. Si prova un giro completo: mint → verbo normale → verbo con approval.
4. Quando tutti i driver sono migrati, l'operatore toglie
   `SIGILED_BOOTSTRAP_BEARER` dallo stack env: fine della finestra, il
   bearer legacy muore.
