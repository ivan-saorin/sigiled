use super::*;
#[derive(Clone)]
pub(super) struct Binding {
    pub target: Option<targets::FileTarget>,
    pub port: Option<u16>,
    pub parent: String,
    pub actor: String,
    pub project: String,
    pub session: String,
    pub generation: u64,
    pub origin: String,
    pub destination: String,
}
#[derive(Clone)]
pub(super) struct Ticket {
    pub binding: Binding,
    pub expires: u64,
}
#[derive(Clone)]
pub(super) struct Grant {
    pub binding: Binding,
    pub csrf: String,
}
pub(super) fn issue(b: &Inner, binding: Binding) -> Result<String, Error> {
    let id = random()?;
    let mut tickets = b.gateway.tickets.lock().unwrap();
    tickets.retain(|_, t| t.expires > auth::now_epoch());
    if tickets.len() >= 4096 {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "ide_ticket_capacity",
        ));
    }
    tickets.insert(
        id.clone(),
        Ticket {
            binding,
            expires: auth::now_epoch() + 60,
        },
    );
    Ok(id)
}
pub(super) fn live(state: &AppState, binding: &Binding) -> bool {
    let Ok(b) = state.browser.inner() else {
        return false;
    };
    let parent_live = {
        let store = b.store.lock().unwrap();
        store
            .sessions
            .get(&binding.parent)
            .is_some_and(|s| b.live(s, auth::now_epoch()))
    };
    parent_live && record(state, binding).is_ok()
}
pub(super) fn record(state: &AppState, binding: &Binding) -> Result<SessionRecord, Error> {
    let b = state.browser.inner()?;
    let _domain = b
        .config
        .ide_domain
        .as_deref()
        .ok_or(Error(StatusCode::CONFLICT, "ide_gateway_not_configured"))?;
    let r = state
        .sessions
        .record(&binding.session)
        .ok_or(Error(StatusCode::CONFLICT, "workspace_closed"))?;
    if r.project != binding.project
        || r.actor.driver != binding.actor
        || r.generation != binding.generation
        || r.lifecycle != Lifecycle::Active
        || !r.runtime_owned
        || r.token.is_none()
        || !state.sessions.binding_safe(&r)
        || r.container()
            != crate::runtime::Runtime::session_container(&r.project, &r.session_id, r.generation)
        || binding_origin(&b, &r, binding.port)? != binding.origin
        || !r.binding.as_ref().is_some_and(|v| {
            v.generation == r.generation
                && v.ide.as_ref().is_some_and(|i| {
                    i.generation == r.generation
                        && i.started
                        && i.state == "ready"
                        && i.provider == crate::ide::PROVIDER
                })
        })
    {
        return Err(Error(
            StatusCode::CONFLICT,
            "workspace_generation_unavailable",
        ));
    }
    if !state
        .registry
        .descriptor(&r.project)
        .declaration
        .ide
        .enabled
        || binding.port.is_some_and(|p| {
            !state
                .registry
                .descriptor(&r.project)
                .declaration
                .ide
                .preview_ports
                .contains(&p)
        })
    {
        return Err(Error::forbidden("preview_port_not_declared"));
    }
    Ok(r)
}
fn binding_origin(b: &Inner, r: &SessionRecord, port: Option<u16>) -> Result<String, Error> {
    match port {
        Some(p) => targets::preview_origin(
            b.config
                .preview_domain
                .as_deref()
                .ok_or(Error(StatusCode::CONFLICT, "preview_domain_not_configured"))?,
            &r.session_id,
            r.generation,
            p,
        ),
        None => targets::origin(
            b.config
                .ide_domain
                .as_deref()
                .ok_or(Error(StatusCode::CONFLICT, "ide_gateway_not_configured"))?,
            &r.session_id,
            r.generation,
        ),
    }
}
fn cookie_name(binding: &Binding) -> &'static str {
    if binding.port.is_some() {
        "__Host-sigil_preview"
    } else {
        "__Host-sigil_ide"
    }
}
pub(super) async fn authorize(
    state: &AppState,
    binding: &Binding,
    activity: bool,
) -> Result<BrowserContext, Error> {
    let c = resolve_session(state, &binding.parent, activity).await?;
    if c.actor.driver != binding.actor {
        return Err(Error::forbidden("workspace_owner_changed"));
    }
    let r = record(state, binding)?;
    crate::sessions::authorized(&c.actor, &r, state, crate::auth::Action::OpenSession)
        .map_err(|_| Error::forbidden("workspace_approval_required"))?;
    Ok(c)
}
pub(super) fn exact_origin(headers: &HeaderMap, origin: &str, required: bool) -> Result<(), Error> {
    if single(headers, "host") != Some(&origin[8..])
        || ((required || headers.contains_key("origin"))
            && single(headers, "origin") != Some(origin))
    {
        return Err(Error::forbidden("invalid_origin"));
    }
    // Requests initiated by sibling preview content are rejected even for GET.
    if single(headers, "sec-fetch-site").is_some_and(|s| s != "same-origin" && s != "none") {
        return Err(Error::forbidden("cross_origin_editor_request"));
    }
    Ok(())
}
pub(super) async fn consume(
    state: &AppState,
    headers: &HeaderMap,
    ticket: &str,
) -> Result<Response, Error> {
    let b = state.browser.inner()?;
    let t = {
        let mut tickets = b.gateway.tickets.lock().unwrap();
        let t = tickets.get(ticket).ok_or_else(Error::login)?;
        exact_origin(headers, &t.binding.origin, true)?;
        tickets.remove(ticket).unwrap()
    };
    if t.expires <= auth::now_epoch() {
        return Err(Error::login());
    }
    let c = authorize(state, &t.binding, t.binding.port.is_none()).await?;
    if let Some(target) = &t.binding.target {
        let r = record(state, &t.binding)?;
        targets::probe(state, &r, target).await?;
    }
    let id = random()?;
    let csrf = random()?;
    let mut grants = b.gateway.grants.lock().unwrap();
    grants.retain(|_, g| live(state, &g.binding));
    if grants.len() >= 4096 {
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "ide_grant_capacity"));
    }
    let mut data = serde_json::json!({"generation":t.binding.generation.to_string(),"destination":if t.binding.port.is_some(){"/"}else{"/_sigil/workbench"}});
    if t.binding.port.is_none() {
        data["csrf_token"] = serde_json::json!(csrf);
    }
    let mut r = Json(data).into_response();
    let value = format!(
        "{}={id}; Path=/; Max-Age={}; Secure; HttpOnly; SameSite=Strict",
        cookie_name(&t.binding),
        c.absolute.saturating_sub(auth::now_epoch())
    );
    r.headers_mut().insert("set-cookie", value.parse().unwrap());
    grants.insert(
        id,
        Grant {
            binding: t.binding,
            csrf,
        },
    );
    Ok(r)
}
pub(super) async fn grant(
    state: &AppState,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<Grant, Error> {
    let b = state.browser.inner()?;
    let name = if b
        .config
        .preview_domain
        .as_ref()
        .is_some_and(|d| single(headers, "host").is_some_and(|h| h.ends_with(&format!(".{d}"))))
    {
        "__Host-sigil_preview"
    } else {
        "__Host-sigil_ide"
    };
    let id = cookie(headers, name).ok_or_else(Error::login)?;
    let g = b
        .gateway
        .grants
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(Error::login)?;
    if name != cookie_name(&g.binding) {
        return Err(Error::login());
    }
    exact_origin(headers, &g.binding.origin, mutation)?;
    authorize(state, &g.binding, false).await?;
    Ok(g)
}
pub(super) fn csrf(headers: &HeaderMap, grant: &Grant) -> Result<(), Error> {
    if !single(headers, "x-sigil-csrf").is_some_and(|v| auth::constant_time_eq(v, &grant.csrf)) {
        return Err(Error::forbidden("invalid_csrf"));
    }
    Ok(())
}
