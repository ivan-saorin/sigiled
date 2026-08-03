#!/bin/sh
# authentik-provision.sh — creates the SIGILED OAuth2 providers on the stack
# IdP (design §1.3-1.4): sigiled-device (device flow, the human leg) and one
# confidential provider per driver (client_credentials, the machine leg).
#
#   DOMAIN=example.com AUTHENTIK_API_TOKEN=<admin token> tools/authentik-provision.sh
#   AUTHENTIK_URL=https://auth.example.com ...  (default: https://auth.$DOMAIN)
#   SIGILED_API_BASE=https://api.example.com ... (default: https://api.$DOMAIN)
#   DRIVERS="sigiled-claude sigiled-kimi" ...    (default: these two)
#
# The token must belong to an admin user (Admin interface → Directory →
# Tokens, intent "API Token"). The embedded outpost's token is NOT enough:
# it reads filtered and cannot create (verified 2026-08-02, all 403/empty).
# Idempotent: existing providers are left alone.
#
# Lessons paid for in debugging (2026-08-02, authentik 2026.5.6):
#   - `redirect_uris` is schema-required even where unused.
#   - `grant_types` is an ALLOWLIST: empty = every grant refused with a
#     SILENT invalid_grant (no event). Declare it per provider — which is
#     exactly the "narrow grant" design §1.8 asks for.
#   - the groups mapping expression uses request.user.groups (ak_groups is
#     deprecated and logs configuration_warning).
#   - the service account ak-<provider>-client_credentials is born on the
#     FIRST successful mint: the script "primes" it and adds it to
#     stack:drivers.
set -eu

# Instance identity: DOMAIN drives both bases unless overridden explicitly.
[ -n "${DOMAIN:-}" ] || [ -n "${AUTHENTIK_URL:-}" ] || {
    echo "set DOMAIN (e.g. example.com) or AUTHENTIK_URL + SIGILED_API_BASE" >&2; exit 1; }
BASE="${AUTHENTIK_URL:-https://auth.${DOMAIN}}"
API_PUBLIC="${SIGILED_API_BASE:-https://api.${DOMAIN:?SIGILED_API_BASE needs DOMAIN when unset}}"
TOKEN="${AUTHENTIK_API_TOKEN:?AUTHENTIK_API_TOKEN (admin) is required}"
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
        \"redirect_uris\": [{\"matching_mode\": \"strict\", \"url\": \"$API_PUBLIC/sigiled/auth/callback\"}],
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
