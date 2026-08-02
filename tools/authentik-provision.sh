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
set -eu

BASE="${AUTHENTIK_URL:-https://auth.016180.xyz}"
TOKEN="${AUTHENTIK_API_TOKEN:?serve AUTHENTIK_API_TOKEN (admin)}"
DRIVERS="${DRIVERS:-sigiled-claude sigiled-kimi}"
API="$BASE/api/v3"
auth="Authorization: Bearer $TOKEN"

get() { curl -sSf -H "$auth" "$API$1"; }
post() { curl -sSf -H "$auth" -H "Content-Type: application/json" -d "$2" "$API$1"; }

echo "== flows e certificato di firma"
AUTHZ_FLOW=$(get "/flows/instances/?designation=authorization" | python3 -c 'import json,sys; print(json.load(sys.stdin)["results"][0]["pk"])')
INVAL_FLOW=$(get "/flows/instances/?designation=invalidation" | python3 -c 'import json,sys; r=json.load(sys.stdin)["results"]; print(r[0]["pk"])')
SIGN_KEY=$(get "/crypto/certificatekeypairs/?has_key=true" | python3 -c 'import json,sys; print(json.load(sys.stdin)["results"][0]["pk"])')
echo "   authorization=$AUTHZ_FLOW invalidation=$INVAL_FLOW signing=$SIGN_KEY"

echo "== scope mapping groups (claim per la capability map §1.6)"
GROUPS_MAPPING=$(get "/propertymappings/provider/scope/?scope_name=sigiled-groups" | python3 -c 'import json,sys; r=json.load(sys.stdin)["results"]; print(r[0]["pk"] if r else "")')
if [ -z "$GROUPS_MAPPING" ]; then
    GROUPS_MAPPING=$(post "/propertymappings/provider/scope/" '{
        "name": "sigiled: groups claim",
        "scope_name": "sigiled-groups",
        "expression": "return {\"groups\": [g.name for g in request.user.ak_groups.all()]}"
    }' | python3 -c 'import json,sys; print(json.load(sys.stdin)["pk"])')
fi
echo "   mapping=$GROUPS_MAPPING"

echo "== gruppi stack:admins / stack:drivers"
for g in "stack:admins" "stack:drivers"; do
    exists=$(get "/core/groups/?name=$(printf %s "$g" | sed s/:/%3A/)" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["results"]))')
    [ "$exists" = "0" ] && post "/core/groups/" "{\"name\": \"$g\"}" >/dev/null && echo "   creato $g" || echo "   $g ok"
done

mkprovider() { # $1=name $2=client_type $3=extra-json-fields
    pk=$(get "/providers/oauth2/?name=$1" | python3 -c 'import json,sys; r=json.load(sys.stdin)["results"]; print(r[0]["pk"] if r else "")')
    if [ -n "$pk" ]; then echo "   provider $1 già presente (pk=$pk)"; return; fi
    post "/providers/oauth2/" "{
        \"name\": \"$1\", \"client_id\": \"$1\", \"client_type\": \"$2\",
        \"authorization_flow\": \"$AUTHZ_FLOW\", \"invalidation_flow\": \"$INVAL_FLOW\",
        \"signing_key\": \"$SIGN_KEY\",
        \"property_mappings\": [\"$GROUPS_MAPPING\"],
        \"sub_mode\": \"user_username\" $3
    }" >/dev/null
    pk=$(get "/providers/oauth2/?name=$1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["results"][0]["pk"])')
    post "/core/applications/" "{\"name\": \"$1\", \"slug\": \"$1\", \"provider\": $pk}" >/dev/null
    echo "   provider+app $1 creati (pk=$pk)"
}

echo "== provider device flow (gamba umana)"
mkprovider "sigiled-device" "public" ""

echo "== provider driver (gamba machine, client_credentials)"
for d in $DRIVERS; do
    mkprovider "$d" "confidential" ""
done

cat <<'EOF'
== fatto. Passi manuali residui (una tantum, UI Authentik):
   1. Aggiungi il tuo utente a stack:admins; i service account dei driver
      (creati da Authentik al primo client_credentials) a stack:drivers.
   2. Verifica che il brand abbia il device flow abilitato
      (Brands → default → Default flows → Device code flow).
   3. I client_secret dei driver: Applications → Providers → <driver> →
      Edit — vanno SOLO nelle skill dei rispettivi driver, mai in git.
   4. Env per sigiledd (stack env): SIGILED_OIDC_BASE=<url IdP>,
      SIGILED_BOOTSTRAP_BEARER=<bearer legacy finché dura la finestra>.
EOF
