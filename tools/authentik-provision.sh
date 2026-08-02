#!/bin/sh
# authentik-provision.sh — crea i provider OAuth2 SIGILED sull'IdP di stack
# (design §1.3-1.4): sigiled-device (device flow, gamba umana) e un provider
# confidential per driver (client_credentials, gamba machine).
#
#   AUTHENTIK_API_TOKEN=<token admin> tools/authentik-provision.sh
#   AUTHENTIK_URL=https://auth.example.com ... (default: IdP dello stack)
#   DRIVERS="sigiled-claude sigiled-kimi" ...   (default: questi due)
#
# Il token deve essere di un utente admin (Admin interface → Directory →
# Tokens, intent «API Token»). Il token dell'outpost embedded NON basta:
# legge filtrato e non può creare (verificato 2026-08-02, tutto 403/vuoto).
# Idempotente: i provider esistenti vengono lasciati stare.
#
# Lezioni pagate col debugging (2026-08-02, authentik 2026.5.6):
#   - `redirect_uris` è obbligatorio per schema anche dove non serve.
#   - `grant_types` è una ALLOWLIST: vuota = ogni grant rifiutato con un
#     invalid_grant SILENZIOSO (nessun evento). Va dichiarata per provider —
#     ed è esattamente il «grant ristretto» che il design §1.8 chiede.
#   - l'expression del mapping groups usa request.user.groups (ak_groups è
#     deprecato e logga configuration_warning).
#   - il service account ak-<provider>-client_credentials nasce al PRIMO
#     mint riuscito: lo script lo "prima" e lo mette in stack:drivers.
set -eu

BASE="${AUTHENTIK_URL:-https://auth.016180.xyz}"
TOKEN="${AUTHENTIK_API_TOKEN:?serve AUTHENTIK_API_TOKEN (admin)}"
DRIVERS="${DRIVERS:-sigiled-claude sigiled-kimi}"
API="$BASE/api/v3"
auth="Authorization: Bearer $TOKEN"

get() { curl -sSf -H "$auth" "$API$1"; }
post() { curl -sSf -H "$auth" -H "Content-Type: application/json" -d "$2" "$API$1"; }

first_pk() { python3 -c 'import json,sys; r=json.load(sys.stdin)["results"]; print(r[0]["pk"] if r else "")'; }

echo "== flows e certificato di firma"
AUTHZ_FLOW=$(get "/flows/instances/?designation=authorization" | first_pk)
INVAL_FLOW=$(get "/flows/instances/?designation=invalidation" | first_pk)
SIGN_KEY=$(get "/crypto/certificatekeypairs/?has_key=true" | first_pk)
echo "   authorization=$AUTHZ_FLOW invalidation=$INVAL_FLOW signing=$SIGN_KEY"

echo "== scope mapping groups (claim per la capability map §1.6)"
GROUPS_MAPPING=$(get "/propertymappings/provider/scope/?scope_name=sigiled-groups" | first_pk)
if [ -z "$GROUPS_MAPPING" ]; then
    GROUPS_MAPPING=$(post "/propertymappings/provider/scope/" '{
        "name": "sigiled: groups claim",
        "scope_name": "sigiled-groups",
        "expression": "return {\"groups\": [g.name for g in request.user.groups.all()]}"
    }' | python3 -c 'import json,sys; print(json.load(sys.stdin)["pk"])')
fi
PROFILE_MAPPING=$(get "/propertymappings/provider/scope/?managed__iexact=goauthentik.io%2Fproviders%2Foauth2%2Fscope-profile" | first_pk)
echo "   groups=$GROUPS_MAPPING profile=$PROFILE_MAPPING"

echo "== gruppi stack:admins / stack:drivers"
for g in "stack:admins" "stack:drivers"; do
    exists=$(get "/core/groups/?name=$(printf %s "$g" | sed s/:/%3A/)" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["results"]))')
    [ "$exists" = "0" ] && post "/core/groups/" "{\"name\": \"$g\"}" >/dev/null && echo "   creato $g" || echo "   $g ok"
done
DRIVERS_GROUP=$(get "/core/groups/?name=stack%3Adrivers" | first_pk)

mkprovider() { # $1=name $2=client_type $3=grant_types-json-array
    pk=$(get "/providers/oauth2/?name=$1" | first_pk)
    if [ -n "$pk" ]; then echo "   provider $1 già presente (pk=$pk)"; return; fi
    # redirect_uris: obbligatorio per schema, placeholder strict inoffensivo.
    post "/providers/oauth2/" "{
        \"name\": \"$1\", \"client_id\": \"$1\", \"client_type\": \"$2\",
        \"authorization_flow\": \"$AUTHZ_FLOW\", \"invalidation_flow\": \"$INVAL_FLOW\",
        \"signing_key\": \"$SIGN_KEY\",
        \"property_mappings\": [\"$GROUPS_MAPPING\", \"$PROFILE_MAPPING\"],
        \"redirect_uris\": [{\"matching_mode\": \"strict\", \"url\": \"https://api.016180.xyz/sigiled/auth/callback\"}],
        \"grant_types\": $3,
        \"sub_mode\": \"user_username\"
    }" >/dev/null
    pk=$(get "/providers/oauth2/?name=$1" | first_pk)
    post "/core/applications/" "{\"name\": \"$1\", \"slug\": \"$1\", \"provider\": $pk}" >/dev/null
    echo "   provider+app $1 creati (pk=$pk)"
}

echo "== provider device flow (gamba umana)"
mkprovider "sigiled-device" "public" '["urn:ietf:params:oauth:grant-type:device_code"]'

echo "== provider driver (gamba machine, client_credentials)"
for d in $DRIVERS; do
    mkprovider "$d" "confidential" '["client_credentials"]'
done

echo "== priming: primo mint per creare i service account ak-*"
for d in $DRIVERS; do
    pk=$(get "/providers/oauth2/?name=$d" | first_pk)
    get "/providers/oauth2/$pk/" | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["client_secret"])' > /tmp/.cs.$$
    curl -sS -X POST "$BASE/application/o/token/" \
        -d grant_type=client_credentials -d "client_id=$d" \
        --data-urlencode "client_secret@/tmp/.cs.$$" -d scope=profile -o /dev/null
    rm -f /tmp/.cs.$$
    sa=$(get "/core/users/?type=service_account&search=ak-$d" | python3 -c 'import json,sys; r=[u for u in json.load(sys.stdin)["results"] if u["username"]=="ak-'"$d"'-client_credentials"]; print(r[0]["pk"] if r else "")')
    if [ -n "$sa" ]; then
        post "/core/groups/$DRIVERS_GROUP/add_user/" "{\"pk\": $sa}" >/dev/null || true
        echo "   ak-$d-client_credentials (pk=$sa) → stack:drivers"
    else
        echo "   ATTENZIONE: service account di $d non trovato — mint fallito?"
    fi
done

cat <<'EOF'
== fatto. Passi manuali residui (una tantum, UI Authentik):
   1. Aggiungi il TUO utente umano a stack:admins (lo script non decide
      quale sia).
   2. Verifica che il brand abbia il device flow abilitato
      (Brands → default → Default flows → Device code flow).
   3. I client_secret dei driver: Applications → Providers → <driver> →
      Edit — vanno SOLO nelle skill dei rispettivi driver, mai in git.
   4. Env per sigiledd (stack env): SIGILED_OIDC_BASE=<url IdP>,
      SIGILED_BOOTSTRAP_BEARER=<bearer legacy finché dura la finestra>.
EOF
